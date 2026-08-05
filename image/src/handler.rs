use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::{
    extract::{ConnectInfo, Query, State},
    http::{HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use tokio::sync::Semaphore;
use tracing::{info, warn};

use crate::cache::ImageCache;
use crate::config::Config;
use crate::processor::{OutputFormat, ProcessError, ProcessOptions, process_image};
use crate::security::SecurityError;

#[derive(Debug, Deserialize)]
pub struct ImageQuery {
    pub src: String,
    pub w: Option<u32>,
    pub f: Option<String>,
}

pub struct AppState {
    pub cache: ImageCache,
    pub config: Config,
    pub rate_limiter: RateLimiter,
    /// SECURITY (CPU exhaustion): global cap on simultaneous decode/resize/encode work.
    ///
    /// `Arc`, not a bare `Semaphore`, so a permit can be taken as an `OwnedSemaphorePermit` and
    /// MOVED into the blocking closure that does the work. See `processor::with_transform_permit`
    /// for why the permit's lifetime has to follow the work rather than the request.
    pub transform_permits: Arc<Semaphore>,
}

/// How a client is bucketed for rate limiting.
///
/// An IPv6 client is bucketed by its /64, not its address. A routine residential or cloud
/// allocation is a whole /64, so per-address buckets let one attacker mint unlimited distinct keys,
/// which both defeats the limit and grows the map without bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ClientKey {
    V4(std::net::Ipv4Addr),
    V6Net64([u8; 8]),
}

impl From<IpAddr> for ClientKey {
    fn from(ip: IpAddr) -> Self {
        match ip {
            IpAddr::V4(v4) => ClientKey::V4(v4),
            IpAddr::V6(v6) => {
                // An IPv4-mapped address is really an IPv4 client; bucket it as one so a
                // dual-stack listener cannot be used to get a second allowance.
                if let Some(v4) = v6.to_ipv4_mapped() {
                    return ClientKey::V4(v4);
                }
                let o = v6.octets();
                let mut prefix = [0u8; 8];
                prefix.copy_from_slice(&o[..8]);
                ClientKey::V6Net64(prefix)
            }
        }
    }
}

const RATE_LIMIT_SHARDS: usize = 16;

/// Last-line-of-defence rate limiter; not a substitute for reverse-proxy limits.
///
/// SECURITY (lock contention): sharded, and pruned OFF the request path. Taking one global mutex
/// and running a full `HashMap::retain` on every request before any other work would let an
/// attacker supplying many distinct source addresses turn each request into an O(n) scan under a
/// lock every other request has to queue behind.
pub struct RateLimiter {
    shards: Vec<Mutex<HashMap<ClientKey, RateWindow>>>,
    max_requests: u64,
    window_secs: u64,
    /// Hard ceiling on tracked buckets IN ONE SHARD, so the map cannot grow without bound between
    /// prunes. The name is the accurate one: `with_capacity` divides the caller's all-shards
    /// figure by `RATE_LIMIT_SHARDS`, so the whole-limiter ceiling is this number times 16.
    max_entries_per_shard: usize,
}

struct RateWindow {
    count: u64,
    window_start: Instant,
}

impl RateLimiter {
    #[must_use]
    pub fn new(max_requests: u64, window_secs: u64) -> Self {
        Self::with_capacity(max_requests, window_secs, 65_536)
    }

    #[must_use]
    pub fn with_capacity(max_requests: u64, window_secs: u64, max_entries: usize) -> Self {
        let shards = (0..RATE_LIMIT_SHARDS)
            .map(|_| Mutex::new(HashMap::new()))
            .collect();
        Self {
            shards,
            max_requests,
            window_secs,
            max_entries_per_shard: (max_entries / RATE_LIMIT_SHARDS).max(1),
        }
    }

    fn shard_for(&self, key: &ClientKey) -> &Mutex<HashMap<ClientKey, RateWindow>> {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut hasher);
        &self.shards[(hasher.finish() as usize) % self.shards.len()]
    }

    /// Returns false when the request should be refused. Takes `&self`: the locks are per-shard and
    /// no await happens while one is held, so this stays a plain, short critical section.
    pub fn check_and_record(&self, ip: IpAddr) -> bool {
        let key = ClientKey::from(ip);
        let now = Instant::now();
        let window_duration = std::time::Duration::from_secs(self.window_secs);

        let mut map = match self.shard_for(&key).lock() {
            Ok(m) => m,
            // A poisoned lock means another thread panicked mid-update. Failing closed on a rate
            // limiter is the safe direction.
            Err(_) => return false,
        };

        if !map.contains_key(&key) && map.len() >= self.max_entries_per_shard {
            // The shard is full of live buckets. Drop the expired ones first...
            map.retain(|_, w| now.duration_since(w.window_start) < window_duration * 2);
            if map.len() >= self.max_entries_per_shard {
                // ...and if it is still full, refuse rather than grow. Memory is bounded by
                // construction, not by hoping the sweeper keeps up.
                return false;
            }
        }

        let entry = map.entry(key).or_insert(RateWindow {
            count: 0,
            window_start: now,
        });

        if now.duration_since(entry.window_start) >= window_duration {
            entry.count = 0;
            entry.window_start = now;
        }

        if entry.count >= self.max_requests {
            false
        } else {
            entry.count += 1;
            true
        }
    }

    /// Drop buckets whose window is long past. Called from a background timer, never on the request
    /// path. Each shard is locked separately, so pruning never blocks the whole service.
    pub fn prune(&self) {
        let now = Instant::now();
        let window_duration = std::time::Duration::from_secs(self.window_secs);
        for shard in &self.shards {
            if let Ok(mut map) = shard.lock() {
                map.retain(|_, w| now.duration_since(w.window_start) < window_duration * 2);
            }
        }
    }

    /// Total tracked buckets. Test and diagnostic use only.
    #[must_use]
    pub fn tracked_entries(&self) -> usize {
        self.shards
            .iter()
            .map(|s| s.lock().map(|m| m.len()).unwrap_or(0))
            .sum()
    }
}

pub async fn image_handler(
    ConnectInfo(addr): ConnectInfo<std::net::SocketAddr>,
    State(state): State<Arc<AppState>>,
    Query(query): Query<ImageQuery>,
) -> Response {
    let client_ip = addr.ip();

    // No prune here: it runs on a background timer (see `spawn_rate_limit_pruner`).
    if !state.rate_limiter.check_and_record(client_ip) {
        warn!(ip = %client_ip, "rate limit exceeded");
        return error_response_with_retry(
            StatusCode::TOO_MANY_REQUESTS,
            "rate limit exceeded - try again later",
            retry_after_for(StatusCode::TOO_MANY_REQUESTS),
        );
    }

    let width = match query.w {
        None => {
            return error_response(StatusCode::BAD_REQUEST, "missing required parameter: w");
        }
        Some(0) => {
            return error_response(StatusCode::BAD_REQUEST, "w must be greater than 0");
        }
        Some(w) if w > state.config.max_width => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!(
                    "w exceeds maximum allowed value of {}",
                    state.config.max_width
                ),
            );
        }
        Some(w) => w,
    };

    let format = match query.f.as_deref().unwrap_or("webp") {
        "avif" => OutputFormat::Avif,
        "webp" => OutputFormat::Webp,
        "jpeg" | "jpg" => OutputFormat::Jpeg,
        other => {
            return error_response(
                StatusCode::BAD_REQUEST,
                &format!("unsupported format '{other}'; use avif, webp, or jpeg"),
            );
        }
    };

    if query.src.trim().is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "missing required parameter: src");
    }

    let cache_key = ImageCache::cache_key(&query.src, width, format.as_str());

    if let Some(cached) = state.cache.get(&cache_key).await {
        info!(
            ip = %client_ip,
            src = %query.src,
            width,
            format = format.as_str(),
            "cache hit"
        );
        return image_response(cached, format.mime_type(), &state.config);
    }

    let opts = ProcessOptions { width, format };

    match process_image(&query.src, opts, &state.config, &state.transform_permits).await {
        Ok((data, mime)) => {
            info!(
                ip = %client_ip,
                src = %query.src,
                width,
                format = format.as_str(),
                bytes = data.len(),
                "processed"
            );
            state.cache.set(&cache_key, data.clone()).await;
            image_response(data, mime, &state.config)
        }
        Err(e) => {
            let (status, msg) = classify_error(&e);
            warn!(
                ip = %client_ip,
                src = %query.src,
                error = %e,
                "processing failed"
            );
            error_response_with_retry(status, &msg, retry_after_for(status))
        }
    }
}

fn image_response(data: Vec<u8>, mime: &'static str, config: &Config) -> Response {
    let cache_control = format!("public, max-age={}", config.cache_ttl);
    let mut resp = (StatusCode::OK, data).into_response();
    let headers = resp.headers_mut();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static(mime),
    );
    headers.insert(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_str(&cache_control)
            .unwrap_or_else(|_| HeaderValue::from_static("public, max-age=3600")),
    );
    resp
}

fn error_response(status: StatusCode, message: &str) -> Response {
    error_response_with_retry(status, message, None)
}

/// As [`error_response`], optionally carrying `Retry-After`.
///
/// A 503 that does not say when to come back leaves every client guessing, and the usual guess is
/// "immediately", which is the worst possible answer for a service refusing work because it has
/// none to spare. Any status that means "come back later" has to carry the header.
fn error_response_with_retry(
    status: StatusCode,
    message: &str,
    retry_after_secs: Option<u64>,
) -> Response {
    let body = serde_json::json!({ "error": message });
    let mut resp = (
        status,
        [(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        )],
        body.to_string(),
    )
        .into_response();
    if let Some(secs) = retry_after_secs
        && let Ok(v) = HeaderValue::from_str(&secs.to_string())
    {
        resp.headers_mut()
            .insert(axum::http::header::RETRY_AFTER, v);
    }
    resp
}

/// How long a client should wait before retrying, when we can say something useful.
///
/// Only the two "come back later" statuses get one. A 4xx caused by the request itself will fail
/// again at any time, and telling a client to retry it is an invitation to hammer.
fn retry_after_for(status: StatusCode) -> Option<u64> {
    match status {
        StatusCode::SERVICE_UNAVAILABLE => Some(5),
        StatusCode::TOO_MANY_REQUESTS => Some(60),
        _ => None,
    }
}

// Deliberately vague for security errors to avoid leaking internals.
fn classify_error(err: &ProcessError) -> (StatusCode, String) {
    match err {
        ProcessError::Security(SecurityError::SsrfBlocked) => {
            (StatusCode::FORBIDDEN, "request blocked".into())
        }
        ProcessError::Security(SecurityError::InvalidUrl)
        | ProcessError::Security(SecurityError::DisallowedScheme(_))
        | ProcessError::InvalidUrl => (StatusCode::BAD_REQUEST, "invalid source URL".into()),
        ProcessError::Security(SecurityError::SourceTooLarge) => (
            StatusCode::PAYLOAD_TOO_LARGE,
            "source image too large".into(),
        ),
        ProcessError::Security(SecurityError::DecompressionBomb) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "image dimensions too large".into(),
        ),
        ProcessError::Security(SecurityError::InvalidMagicBytes)
        | ProcessError::Security(SecurityError::UnsupportedSourceFormat) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "not a supported image format".into(),
        ),
        ProcessError::Security(SecurityError::OutputTooLarge) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "requested output image is too large".into(),
        ),
        ProcessError::DnsError(_) => (
            StatusCode::BAD_GATEWAY,
            "could not resolve source host".into(),
        ),
        ProcessError::Fetch(_) => (
            StatusCode::BAD_GATEWAY,
            "could not fetch source image".into(),
        ),
        ProcessError::Decode(_) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "could not decode image".into(),
        ),
        ProcessError::Encode(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not encode image".into(),
        ),
        // The permit could not be acquired, so no CPU work was started.
        //
        // This is NOT the saturation path, despite how it reads. `Semaphore::acquire` only
        // errors when the semaphore has been CLOSED, and nothing closes this one; saturation
        // simply makes the request wait, bounded by the whole-request `TimeoutLayer`. Queueing
        // is the intended behaviour - the alternative is refusing a request the service could
        // have served a moment later - so the branch really does mean "shutting down", which is
        // what the variant's own message says.
        //
        // Kept rather than removed because a future shutdown path (closing the semaphore to
        // drain in-flight work) lands here, and a 503 is the right answer for it.
        ProcessError::Unavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "server unavailable, retry shortly".into(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    /// A "come back later" status has to say WHEN, and nothing else may carry the header.
    #[test]
    fn retry_after_is_sent_only_on_come_back_later_statuses() {
        for status in [
            StatusCode::SERVICE_UNAVAILABLE,
            StatusCode::TOO_MANY_REQUESTS,
        ] {
            let secs = retry_after_for(status).expect("must advise a retry delay");
            let resp = error_response_with_retry(status, "x", Some(secs));
            let header = resp
                .headers()
                .get(axum::http::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            assert_eq!(
                header,
                Some(secs.to_string()),
                "{status} must carry Retry-After"
            );
        }

        // A request that failed because of the REQUEST will fail again at any time; advising a
        // retry there is an invitation to hammer.
        for status in [
            StatusCode::BAD_REQUEST,
            StatusCode::FORBIDDEN,
            StatusCode::UNPROCESSABLE_ENTITY,
            StatusCode::PAYLOAD_TOO_LARGE,
            StatusCode::BAD_GATEWAY,
            StatusCode::INTERNAL_SERVER_ERROR,
        ] {
            assert!(
                retry_after_for(status).is_none(),
                "{status} must not advise a retry"
            );
            let resp = error_response(status, "x");
            assert!(
                resp.headers()
                    .get(axum::http::header::RETRY_AFTER)
                    .is_none(),
                "{status} must not carry Retry-After"
            );
        }
    }

    /// The 503 branch is reached by `ProcessError::Unavailable`, and its classification must stay
    /// paired with the header - the two drifted apart once already.
    #[test]
    fn an_unavailable_error_classifies_as_a_retryable_503() {
        let (status, _msg) = classify_error(&ProcessError::Unavailable);
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(
            retry_after_for(status).is_some(),
            "the 503 this error produces must advise a retry delay"
        );
    }

    #[test]
    fn rate_limiter_allows_up_to_limit() {
        let rl = RateLimiter::new(5, 60);
        let ip = IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4));
        for _ in 0..5 {
            assert!(rl.check_and_record(ip));
        }
        assert!(!rl.check_and_record(ip));
    }

    #[test]
    fn rate_limiter_separate_ips_are_independent() {
        let rl = RateLimiter::new(2, 60);
        let ip1 = IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1));
        let ip2 = IpAddr::V4(Ipv4Addr::new(2, 2, 2, 2));
        assert!(rl.check_and_record(ip1));
        assert!(rl.check_and_record(ip1));
        assert!(!rl.check_and_record(ip1));
        assert!(rl.check_and_record(ip2));
    }

    #[test]
    fn rate_limiter_prune_removes_expired_windows() {
        // Window of 0 seconds, so `window_duration * 2` is zero and every entry is already expired
        // from prune's point of view. This exercises prune without sleeping.
        let rl = RateLimiter::new(100, 0);
        rl.check_and_record(IpAddr::V4(Ipv4Addr::new(5, 6, 7, 8)));
        assert_eq!(rl.tracked_entries(), 1);
        rl.prune();
        assert_eq!(rl.tracked_entries(), 0);
    }

    #[test]
    fn rate_limiter_buckets_ipv6_by_slash_64() {
        // An attacker with one /64 must not get a fresh allowance per address.
        let rl = RateLimiter::new(2, 60);
        let a: IpAddr = "2001:4860:4860:1234::1".parse().unwrap();
        let b: IpAddr = "2001:4860:4860:1234::dead:beef".parse().unwrap();
        let other_net: IpAddr = "2001:4860:4860:9999::1".parse().unwrap();
        assert!(rl.check_and_record(a));
        assert!(rl.check_and_record(b));
        assert!(!rl.check_and_record(b), "same /64 must share one bucket");
        assert!(!rl.check_and_record(a), "same /64 must share one bucket");
        assert!(
            rl.check_and_record(other_net),
            "a different /64 is a different bucket"
        );
        assert_eq!(
            rl.tracked_entries(),
            2,
            "one bucket per /64, not per address"
        );
    }

    #[test]
    fn rate_limiter_ipv4_mapped_shares_the_ipv4_bucket() {
        let rl = RateLimiter::new(1, 60);
        let v4 = IpAddr::V4(Ipv4Addr::new(9, 9, 9, 9));
        let mapped = IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0x0909, 0x0909));
        assert!(rl.check_and_record(v4));
        assert!(
            !rl.check_and_record(mapped),
            "::ffff:9.9.9.9 is the same client"
        );
    }

    #[test]
    fn rate_limiter_entry_count_is_bounded() {
        // The bucket map must stay bounded. 32 entries over 16 shards is 2 per shard; feeding 5000
        // distinct /64s must not grow past that ceiling.
        let rl = RateLimiter::with_capacity(1000, 60, 32);
        for i in 0..5000u32 {
            let ip: IpAddr = format!("2001:db8:{:x}:{:x}::1", i >> 16, i & 0xffff)
                .parse()
                .unwrap();
            rl.check_and_record(ip);
        }
        assert!(
            rl.tracked_entries() <= 32,
            "rate-limit map grew unbounded: {} entries",
            rl.tracked_entries()
        );
    }

    #[test]
    fn ssrf_error_maps_to_forbidden() {
        let (status, _) = classify_error(&ProcessError::Security(SecurityError::SsrfBlocked));
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[test]
    fn source_too_large_maps_to_413() {
        let (status, _) = classify_error(&ProcessError::Security(SecurityError::SourceTooLarge));
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[test]
    fn decompression_bomb_maps_to_422() {
        let (status, _) = classify_error(&ProcessError::Security(SecurityError::DecompressionBomb));
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn invalid_magic_maps_to_422() {
        let (status, _) = classify_error(&ProcessError::Security(SecurityError::InvalidMagicBytes));
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn fetch_error_maps_to_bad_gateway() {
        let (status, _) = classify_error(&ProcessError::Fetch("timeout".into()));
        assert_eq!(status, StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn decode_error_maps_to_422() {
        let (status, _) = classify_error(&ProcessError::Decode("corrupt".into()));
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    }
}
