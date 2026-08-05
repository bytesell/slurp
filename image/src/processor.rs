use std::sync::Arc;

use image::{DynamicImage, ImageDecoder, ImageFormat};
use tokio::net::lookup_host;
use tokio::sync::Semaphore;
use tracing::debug;

use crate::config::Config;
use crate::security::{self, SecurityError, SourceFormat};

#[derive(Debug, Clone, Copy)]
pub struct ProcessOptions {
    pub width: u32,
    pub format: OutputFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Avif,
    Webp,
    Jpeg,
}

impl OutputFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            OutputFormat::Avif => "avif",
            OutputFormat::Webp => "webp",
            OutputFormat::Jpeg => "jpeg",
        }
    }

    pub fn mime_type(&self) -> &'static str {
        match self {
            OutputFormat::Avif => "image/avif",
            OutputFormat::Webp => "image/webp",
            OutputFormat::Jpeg => "image/jpeg",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    #[error("Security violation: {0}")]
    Security(#[from] SecurityError),
    #[error("Invalid URL")]
    InvalidUrl,
    #[error("DNS resolution failed: {0}")]
    DnsError(String),
    #[error("Fetch failed: {0}")]
    Fetch(String),
    #[error("Decode failed: {0}")]
    Decode(String),
    #[error("Encode failed: {0}")]
    Encode(String),
    #[error("Server is shutting down")]
    Unavailable,
}

pub async fn process_image(
    src: &str,
    opts: ProcessOptions,
    config: &Config,
    transform_permits: &Arc<Semaphore>,
) -> Result<(Vec<u8>, &'static str), ProcessError> {
    let url = security::validate_src(src)?;

    // Resolve before connecting so DNS rebinding cannot bypass SSRF checks.
    let host = url.host_str().ok_or(ProcessError::InvalidUrl)?;
    let port = url.port_or_known_default().unwrap_or(80);
    let lookup = format!("{host}:{port}");

    let addrs: Vec<_> = lookup_host(&lookup)
        .await
        .map_err(|e| ProcessError::DnsError(e.to_string()))?
        .collect();

    if addrs.is_empty() {
        return Err(ProcessError::DnsError("no addresses resolved".into()));
    }

    if !private_hosts_allowed(config) {
        for addr in &addrs {
            if !security::is_safe_ip(addr.ip()) {
                return Err(ProcessError::Security(SecurityError::SsrfBlocked));
            }
        }
    }

    debug!(url = url.as_str(), "SSRF check passed");

    // SECURITY (DNS-rebinding): validating the resolved IPs above is useless if
    // reqwest then RE-RESOLVES the host independently when it connects (a second
    // lookup can return a private IP). Pin the connection to the addresses
    // validated above via `resolve_to_addrs`, so reqwest connects ONLY to a
    // checked IP and never performs its own resolution for this host.
    let bytes = fetch_with_limits(&url, host, &addrs, config).await?;

    // Cheap header check before any permit is taken or any thread is committed.
    let source_format = match security::detect_source_format(&bytes) {
        Some(f) => f,
        None if security::is_iso_bmff_image(&bytes) => {
            return Err(ProcessError::Security(
                SecurityError::UnsupportedSourceFormat,
            ));
        }
        None => return Err(ProcessError::Security(SecurityError::InvalidMagicBytes)),
    };

    let config = config.clone();
    with_transform_permit(transform_permits, move || {
        transform(&bytes, source_format, opts, &config)
    })
    .await?
}

/// Run CPU-bound work on the blocking pool under a global transform permit.
///
/// SECURITY (CPU exhaustion): decode, resize and encode are seconds of blocking CPU work. Running
/// them on an async worker lets N concurrent large requests pin every worker, at which point the
/// runtime cannot accept connections or even run its own rate limiter, and the service wedges.
/// They run on the blocking pool instead, behind a global permit so the machine is never
/// oversubscribed. The permit is taken BEFORE `spawn_blocking`, so a queued request occupies a
/// cheap future rather than a blocking thread.
///
/// THE PERMIT IS MOVED INTO THE CLOSURE, and that is the whole point of this function existing.
///
/// Held in the request future instead, the permit is released when that future is DROPPED - and
/// `TimeoutLayer` cancels a request by dropping its future. A `spawn_blocking` closure is not
/// cancellable: it keeps running to completion on its pool thread whatever happens to the
/// JoinHandle. Every timeout would therefore hand a permit to a NEW request while the abandoned
/// transform carried on burning a core, and the real bound on concurrent CPU work would be
/// tokio's blocking pool (512 threads by default) rather than `max_concurrent_transforms`. That
/// is the exact wedge the semaphore exists to prevent, reached BY the timeout meant to protect
/// against it.
///
/// Moving the permit in ties its lifetime to the WORK. A timed-out request stops waiting, as it
/// should, and the machine still runs at most `max_concurrent_transforms` transforms.
async fn with_transform_permit<T, F>(
    transform_permits: &Arc<Semaphore>,
    work: F,
) -> Result<T, ProcessError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    // `acquire_owned` rather than `acquire`: an `OwnedSemaphorePermit` carries its own `Arc` and
    // so can outlive this borrow, which is what lets it be moved into the closure at all.
    let permit = transform_permits
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| ProcessError::Unavailable)?;

    tokio::task::spawn_blocking(move || {
        let out = work();
        // Explicit, and last: the permit must outlive `work()`. Dropping it earlier - or letting
        // a future refactor move it out of this closure - restores the bug above.
        drop(permit);
        out
    })
    .await
    .map_err(|e| ProcessError::Decode(format!("transform worker failed: {e}")))
}

/// The whole CPU-bound half of a request: decode, resize, encode. Runs on the blocking pool.
fn transform(
    bytes: &[u8],
    source_format: SourceFormat,
    opts: ProcessOptions,
    config: &Config,
) -> Result<(Vec<u8>, &'static str), ProcessError> {
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| ProcessError::Decode(e.to_string()))?;

    // SECURITY (polyglot): the magic-byte check and the decoder must agree. `with_guessed_format()`
    // picks by real content, so a polyglot whose header satisfied the magic check can still be
    // routed elsewhere. Refuse unless the decoder landed on the format the header declared.
    match reader.format() {
        Some(f) if f == expected_image_format(source_format) => {}
        _ => {
            return Err(ProcessError::Security(
                SecurityError::UnsupportedSourceFormat,
            ));
        }
    }

    // Header parse only - no pixel buffer is allocated yet. Limits are applied to the decoder
    // afterwards (rather than to the reader) so the more precise guard below reports first.
    let mut decoder = reader
        .into_decoder()
        .map_err(|e| ProcessError::Decode(e.to_string()))?;

    let (width, height) = decoder.dimensions();

    // SECURITY: decompression-bomb guard, enforced BEFORE any decoded buffer is allocated. The
    // header declares the dimensions, so a small crafted file claiming huge dimensions is rejected
    // here. Saturating math: with header dimensions near u32::MAX the product would overflow u64
    // and wrap to a small value, sneaking a bomb past this guard.
    //
    // The per-pixel cost comes from the decoder's own colour type rather than a fixed 4 bytes. A
    // 16-bit PNG or TIFF decodes at 8 bytes per pixel, so a fixed estimate would admit twice the
    // configured budget.
    let bytes_per_pixel = u64::from(decoder.color_type().bytes_per_pixel()).max(1);
    let pixel_count = u64::from(width).saturating_mul(u64::from(height));
    let estimated_bytes = pixel_count.saturating_mul(bytes_per_pixel);
    if estimated_bytes > config.max_decoded_bytes as u64 {
        return Err(ProcessError::Security(SecurityError::DecompressionBomb));
    }

    // SECURITY (CPU exhaustion): a width cap bounds nothing on a very tall source, and encode cost
    // scales with pixels. Reject before decoding rather than after paying for it.
    let (out_w, out_h) = output_dimensions(width, height, opts.width);
    if u64::from(out_w).saturating_mul(u64::from(out_h)) > config.max_output_pixels {
        return Err(ProcessError::Security(SecurityError::OutputTooLarge));
    }

    // Defense-in-depth: also hand the decoder an allocation limit in case the header dimensions
    // understate the real decode cost.
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(config.max_decoded_bytes as u64);
    decoder
        .set_limits(limits)
        .map_err(|e| ProcessError::Decode(e.to_string()))?;

    let img =
        DynamicImage::from_decoder(decoder).map_err(|e| ProcessError::Decode(e.to_string()))?;

    debug!(
        width = img.width(),
        height = img.height(),
        target_width = opts.width,
        "decoded image"
    );

    let resized = resize_image(img, opts.width);
    encode_image(resized, opts.format)
}

fn expected_image_format(f: SourceFormat) -> ImageFormat {
    match f {
        SourceFormat::Jpeg => ImageFormat::Jpeg,
        SourceFormat::Png => ImageFormat::Png,
        SourceFormat::Gif => ImageFormat::Gif,
        SourceFormat::WebP => ImageFormat::WebP,
    }
}

/// The dimensions `resize_image` will produce.
///
/// It is the only source of those numbers: the pixel-budget check calls it before decoding, and
/// `resize_image` calls it to size the resample, so the budget cannot be enforced against one
/// figure while the resize produces another. Keep it that way - a second copy of this arithmetic
/// is a budget that can be stepped over by a rounding difference.
#[must_use]
pub fn output_dimensions(width: u32, height: u32, target_width: u32) -> (u32, u32) {
    // An image no wider than the target is returned untouched, which also puts `width >= 1` on the
    // divide below.
    if width <= target_width {
        return (width, height);
    }
    let target_height = ((u64::from(height) * u64::from(target_width)) / u64::from(width)) as u32;
    (target_width, target_height.max(1))
}

/// TEST-ONLY seam (see `Config::allow_private_hosts`). In any build that is not the crate's own
/// test harness this is a compile-time `false`, so the SSRF check cannot be skipped.
#[cfg(not(test))]
fn private_hosts_allowed(_config: &Config) -> bool {
    false
}

#[cfg(test)]
fn private_hosts_allowed(config: &Config) -> bool {
    config.allow_private_hosts
}

async fn fetch_with_limits(
    url: &url::Url,
    host: &str,
    addrs: &[std::net::SocketAddr],
    config: &Config,
) -> Result<Vec<u8>, ProcessError> {
    let mut builder = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(config.fetch_timeout_secs))
        .redirect(reqwest::redirect::Policy::none()); // redirect following enables redirect-based SSRF

    // Pin the host to the validated IPs so reqwest cannot independently re-resolve
    // to a private address (DNS-rebinding defense). Connections are made only to
    // addresses that already passed `is_safe_ip`.
    builder = builder.resolve_to_addrs(host, addrs);

    let client = builder
        .build()
        .map_err(|e| ProcessError::Fetch(e.to_string()))?;

    let response = client
        .get(url.as_str())
        .send()
        .await
        .map_err(|e| ProcessError::Fetch(e.to_string()))?;

    if !response.status().is_success() {
        return Err(ProcessError::Fetch(format!(
            "upstream returned HTTP {}",
            response.status()
        )));
    }

    if response
        .content_length()
        .is_some_and(|len| len > config.max_source_bytes as u64)
    {
        return Err(ProcessError::Security(SecurityError::SourceTooLarge));
    }

    // SECURITY: stream the body and abort the moment the running total exceeds
    // the cap, instead of buffering the whole response first (Content-Length can
    // lie - chunked/gzip - so a malicious upstream could otherwise stream an
    // unbounded body into memory before any check).
    let mut response = response;
    let mut bytes: Vec<u8> = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| ProcessError::Fetch(e.to_string()))?
    {
        if bytes.len() + chunk.len() > config.max_source_bytes {
            return Err(ProcessError::Security(SecurityError::SourceTooLarge));
        }
        bytes.extend_from_slice(&chunk);
    }

    Ok(bytes)
}

fn resize_image(img: DynamicImage, target_width: u32) -> DynamicImage {
    let (w, h) = (img.width(), img.height());
    let (out_w, out_h) = output_dimensions(w, h, target_width);
    if (out_w, out_h) == (w, h) {
        return img;
    }
    img.resize_exact(out_w, out_h, image::imageops::FilterType::Lanczos3)
}

fn encode_image(
    img: DynamicImage,
    format: OutputFormat,
) -> Result<(Vec<u8>, &'static str), ProcessError> {
    match format {
        OutputFormat::Jpeg => {
            let mut buf = Vec::new();
            img.write_to(&mut std::io::Cursor::new(&mut buf), ImageFormat::Jpeg)
                .map_err(|e| ProcessError::Encode(e.to_string()))?;
            Ok((buf, "image/jpeg"))
        }
        OutputFormat::Webp => {
            let mut buf = Vec::new();
            img.write_to(&mut std::io::Cursor::new(&mut buf), ImageFormat::WebP)
                .map_err(|e| ProcessError::Encode(e.to_string()))?;
            Ok((buf, "image/webp"))
        }
        OutputFormat::Avif => {
            let rgba = img.to_rgba8();
            let (width, height) = rgba.dimensions();

            let pixels: Vec<rgb::RGBA8> = rgba
                .pixels()
                .map(|p| rgb::RGBA8 {
                    r: p[0],
                    g: p[1],
                    b: p[2],
                    a: p[3],
                })
                .collect();

            // TODO: expose avif_quality (0-100) and avif_speed (1-10) as Config fields
            let enc = ravif::Encoder::new().with_quality(80.0).with_speed(6);

            let result = enc
                .encode_rgba(ravif::Img::new(&pixels, width as usize, height as usize))
                .map_err(|e| ProcessError::Encode(e.to_string()))?;

            Ok((result.avif_file, "image/avif"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn tiny_jpeg() -> DynamicImage {
        DynamicImage::new_rgb8(4, 4)
    }

    // ---------------------------------------------------------------------
    // Local test origin.
    //
    // The fetch-path guards (streaming cap, timeout, redirect refusal, magic rejection,
    // decompression bomb) all sit BEHIND the SSRF check, which correctly refuses to fetch from
    // 127.0.0.1, so an ordinary test cannot reach them. These tests use the
    // `Config::allow_private_hosts` seam, which is `#[cfg(test)]`-gated and so does not exist in a
    // release build (see `private_hosts_allowed`).
    // ---------------------------------------------------------------------

    enum Origin {
        /// 200 with an accurate Content-Length.
        Body(Vec<u8>),
        /// 200 with no Content-Length at all, body streamed then EOF. Exercises the streaming cap
        /// rather than the header check.
        StreamedBody(Vec<u8>),
        /// 200 declaring a Content-Length far larger than the cap.
        DeclaredLength(usize),
        /// 302 to somewhere else.
        Redirect(&'static str),
        /// Accepts, reads the request, never answers.
        Hang,
    }

    /// Serves exactly one request on loopback. Returns the URL and the task handle.
    async fn spawn_origin(kind: Origin) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let handle = tokio::spawn(async move {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };

            // Drain the request head so the client is not blocked on its own write.
            let mut req = Vec::new();
            let mut buf = [0u8; 1024];
            while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                match sock.read(&mut buf).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => req.extend_from_slice(&buf[..n]),
                }
            }

            match kind {
                Origin::Body(body) => {
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = sock.write_all(head.as_bytes()).await;
                    let _ = sock.write_all(&body).await;
                }
                Origin::StreamedBody(body) => {
                    let head =
                        "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nConnection: close\r\n\r\n";
                    let _ = sock.write_all(head.as_bytes()).await;
                    // Write in slices so the cap has to trip mid-stream.
                    for chunk in body.chunks(16 * 1024) {
                        if sock.write_all(chunk).await.is_err() {
                            return;
                        }
                    }
                }
                Origin::DeclaredLength(len) => {
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n"
                    );
                    let _ = sock.write_all(head.as_bytes()).await;
                }
                Origin::Redirect(to) => {
                    let head = format!(
                        "HTTP/1.1 302 Found\r\nLocation: {to}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                    let _ = sock.write_all(head.as_bytes()).await;
                }
                Origin::Hang => {
                    // Hold the connection open without answering.
                    std::future::pending::<()>().await;
                }
            }
            let _ = sock.shutdown().await;
        });

        (format!("http://127.0.0.1:{port}/image.png"), handle)
    }

    fn test_config() -> Config {
        let mut c = Config::new(0, 60);
        c.allow_private_hosts = true;
        c.max_source_bytes = 64 * 1024;
        c.fetch_timeout_secs = 2;
        c
    }

    fn opts() -> ProcessOptions {
        ProcessOptions {
            width: 100,
            format: OutputFormat::Jpeg,
        }
    }

    async fn fetch_with(
        kind: Origin,
        config: &Config,
    ) -> Result<(Vec<u8>, &'static str), ProcessError> {
        let (url, handle) = spawn_origin(kind).await;
        let sem = Arc::new(Semaphore::new(1));
        let result = process_image(&url, opts(), config, &sem).await;
        handle.abort();
        result
    }

    /// REGRESSION: a transform permit belongs to the WORK, not to the request.
    ///
    /// See `with_transform_permit` for why. The test drops the future exactly as `TimeoutLayer`
    /// does (`tokio::spawn` + `abort`) while the work is provably still running, then asks the
    /// semaphore whether it handed the permit back. It must not.
    #[tokio::test]
    async fn a_transform_permit_outlives_a_cancelled_request() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let permits = Arc::new(Semaphore::new(1));
        let started = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));

        let task = tokio::spawn({
            let permits = permits.clone();
            let (started, release, finished) = (started.clone(), release.clone(), finished.clone());
            async move {
                let _ = with_transform_permit(&permits, move || {
                    started.store(true, Ordering::SeqCst);
                    // Stands in for decode/resize/encode: uncancellable blocking work.
                    while !release.load(Ordering::SeqCst) {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    finished.store(true, Ordering::SeqCst);
                })
                .await;
            }
        });

        // Wait until the blocking work is genuinely in flight, so the abort below lands in the
        // window the bug lived in rather than before the permit was ever taken.
        for _ in 0..2_000 {
            if started.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        assert!(
            started.load(Ordering::SeqCst),
            "the blocking work never started; the test proves nothing"
        );

        // This is what a request timeout does: drop the future.
        task.abort();
        let _ = task.await;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        // MEASURE FIRST, ASSERT LATER, and release the worker in between.
        //
        // The blocking closure spins until `release` is set, and a running `spawn_blocking`
        // thread keeps the runtime from shutting down. Asserting here would unwind with the
        // worker still spinning, so a FAILING run would hang the whole test binary rather than
        // report - which is how a regression stops being read. The guard returned by
        // `try_acquire` is dropped at the end of its statement, so probing costs no permit.
        let work_still_running = !finished.load(Ordering::SeqCst);
        let permit_was_released = permits.try_acquire().is_ok();
        release.store(true, Ordering::SeqCst);

        assert!(
            work_still_running,
            "the work finished early; the test window closed before it could measure anything"
        );
        assert!(
            !permit_was_released,
            "the permit was released while the transform was still running - a new request would              be admitted alongside abandoned CPU work, so concurrency is bounded by the blocking              pool rather than by max_concurrent_transforms"
        );

        // The permit must come back once the work ends, or the fix would leak permits until the
        // service refused everything.
        for _ in 0..2_000 {
            if permits.available_permits() > 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        assert!(
            finished.load(Ordering::SeqCst),
            "the abandoned work never ran to completion"
        );
        assert!(
            permits.try_acquire().is_ok(),
            "the permit was never returned after the work finished - permits now leak"
        );
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in bytes {
            crc ^= u32::from(b);
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }

    fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let mut crc_input = Vec::with_capacity(4 + data.len());
        crc_input.extend_from_slice(kind);
        crc_input.extend_from_slice(data);
        out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
        out
    }

    /// A structurally valid PNG head declaring `width` x `height` at the given bit depth and RGBA
    /// colour type. Only the header is real: the point is that a tiny file can claim vast
    /// dimensions, which is exactly the decompression-bomb shape.
    fn png_header_claiming(width: u32, height: u32, bit_depth: u8) -> Vec<u8> {
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.push(bit_depth);
        ihdr.push(6); // colour type 6 = RGBA
        ihdr.extend_from_slice(&[0, 0, 0]); // compression, filter, interlace

        let mut png = Vec::new();
        png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        png.extend_from_slice(&png_chunk(b"IHDR", &ihdr));
        // The decoder stops reading metadata at the first IDAT, which is where the guard runs. The
        // payload is never reached.
        png.extend_from_slice(&png_chunk(b"IDAT", &[0x78, 0x01, 0x01, 0x00, 0x00]));
        png.extend_from_slice(&png_chunk(b"IEND", &[]));
        png
    }

    #[tokio::test]
    async fn fetch_guard_streaming_cap_trips_without_content_length() {
        let config = test_config();
        // 512 KB against a 64 KB cap, and the server never declares a length.
        let err = fetch_with(Origin::StreamedBody(vec![0u8; 512 * 1024]), &config)
            .await
            .unwrap_err();
        assert!(
            matches!(err, ProcessError::Security(SecurityError::SourceTooLarge)),
            "streaming cap must trip, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn fetch_guard_content_length_cap_trips() {
        let config = test_config();
        let err = fetch_with(Origin::DeclaredLength(100 * 1024 * 1024), &config)
            .await
            .unwrap_err();
        assert!(
            matches!(err, ProcessError::Security(SecurityError::SourceTooLarge)),
            "declared Content-Length over the cap must be refused, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn fetch_guard_redirects_are_refused() {
        let config = test_config();
        let err = fetch_with(Origin::Redirect("http://example.com/evil.png"), &config)
            .await
            .unwrap_err();
        // Policy::none() means the 302 is returned as-is and fails the success check. It must NOT
        // be followed: following redirects is a documented SSRF bypass.
        match err {
            ProcessError::Fetch(msg) => assert!(
                msg.contains("302"),
                "expected the 302 to surface unfollowed, got: {msg}"
            ),
            other => panic!("redirect must not be followed, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn fetch_guard_rejects_non_image_body() {
        let config = test_config();
        let err = fetch_with(
            Origin::Body(b"<!DOCTYPE html><html><body>hi</body></html>".to_vec()),
            &config,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(
                err,
                ProcessError::Security(SecurityError::InvalidMagicBytes)
            ),
            "HTML served as an image must be refused, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn fetch_guard_rejects_iso_bmff_source_as_unsupported() {
        let config = test_config();
        let mut avif = vec![0u8; 32];
        avif[4..8].copy_from_slice(b"ftyp");
        avif[8..12].copy_from_slice(b"avif");
        let err = fetch_with(Origin::Body(avif), &config).await.unwrap_err();
        assert!(
            matches!(
                err,
                ProcessError::Security(SecurityError::UnsupportedSourceFormat)
            ),
            "AVIF input is recognised but not decodable, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn fetch_guard_rejects_decompression_bomb() {
        let config = test_config();
        // 60000 x 60000 RGBA is about 14.4 GB decoded, from a file under 100 bytes.
        let bomb = png_header_claiming(60_000, 60_000, 8);
        assert!(
            bomb.len() < 200,
            "the bomb must be a tiny file to be a bomb"
        );
        let err = fetch_with(Origin::Body(bomb), &config).await.unwrap_err();
        assert!(
            matches!(
                err,
                ProcessError::Security(SecurityError::DecompressionBomb)
            ),
            "decompression bomb must be refused before decoding, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn fetch_guard_times_out_on_a_silent_upstream() {
        let config = test_config(); // fetch_timeout_secs = 2
        let started = std::time::Instant::now();
        let err = fetch_with(Origin::Hang, &config).await.unwrap_err();
        assert!(
            matches!(err, ProcessError::Fetch(_)),
            "a silent upstream must time out, got: {err:?}"
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "the fetch timeout did not fire"
        );
    }

    #[tokio::test]
    async fn ssrf_check_still_blocks_loopback_when_the_seam_is_off() {
        // The seam must be opt-in. With the default config, loopback stays refused, which is what
        // makes every test above safe to have.
        let config = Config::new(0, 60);
        assert!(!config.allow_private_hosts);
        let sem = Arc::new(Semaphore::new(1));
        let err = process_image("http://127.0.0.1:1/x.png", opts(), &config, &sem)
            .await
            .unwrap_err();
        assert!(
            matches!(err, ProcessError::Security(SecurityError::SsrfBlocked)),
            "loopback must stay blocked by default, got: {err:?}"
        );
    }

    #[test]
    fn decoder_format_must_match_the_declared_header() {
        // Defence in depth: even if a magic-byte branch were loosened, the decoder-inferred format
        // is re-checked against the declared one.
        let mut tiff = vec![0x49, 0x49, 0x2A, 0x00, 0x10, 0x00, 0x00, 0x00];
        tiff.extend_from_slice(b"mif1");
        tiff.extend_from_slice(&[0u8; 32]);
        let err = transform(&tiff, SourceFormat::Png, opts(), &test_config()).unwrap_err();
        assert!(
            matches!(
                err,
                ProcessError::Security(SecurityError::UnsupportedSourceFormat)
            ),
            "a TIFF claiming to be PNG must be refused, got: {err:?}"
        );
    }

    #[test]
    fn sixteen_bit_source_is_charged_at_its_real_per_pixel_cost() {
        // 3000 x 3000 is 36 MB at a fixed 4 bytes/px (under the 50 MB budget) but 72 MB at the
        // real 8 bytes/px of a 16-bit RGBA PNG.
        let mut config = test_config();
        config.max_decoded_bytes = 50 * 1024 * 1024;

        let eight_bit = png_header_claiming(3000, 3000, 8);
        let err = transform(&eight_bit, SourceFormat::Png, opts(), &config).unwrap_err();
        assert!(
            !matches!(
                err,
                ProcessError::Security(SecurityError::DecompressionBomb)
            ),
            "8-bit 3000x3000 is inside the budget and must pass the bomb guard, got: {err:?}"
        );

        let sixteen_bit = png_header_claiming(3000, 3000, 16);
        let err = transform(&sixteen_bit, SourceFormat::Png, opts(), &config).unwrap_err();
        assert!(
            matches!(
                err,
                ProcessError::Security(SecurityError::DecompressionBomb)
            ),
            "16-bit 3000x3000 costs 72 MB and must be refused, got: {err:?}"
        );
    }

    #[test]
    fn output_pixel_budget_is_enforced() {
        // A width cap alone bounds nothing on a very tall source.
        let mut config = test_config();
        config.max_decoded_bytes = 500 * 1024 * 1024; // let the bomb guard pass so this cap is what bites
        config.max_output_pixels = 1_000_000;

        let tall = png_header_claiming(4000, 8000, 8);
        let wide_opts = ProcessOptions {
            width: 4000,
            format: OutputFormat::Jpeg,
        };
        let err = transform(&tall, SourceFormat::Png, wide_opts, &config).unwrap_err();
        assert!(
            matches!(err, ProcessError::Security(SecurityError::OutputTooLarge)),
            "32 MP of output against a 1 MP budget must be refused, got: {err:?}"
        );
    }

    #[test]
    fn output_dimensions_match_the_resize() {
        for (w, h, target) in [
            (1000u32, 500u32, 200u32),
            (800, 600, 400),
            (100, 50, 200),
            (10000, 1, 1),
        ] {
            let (ow, oh) = output_dimensions(w, h, target);
            let resized = resize_image(DynamicImage::new_rgb8(w, h), target);
            assert_eq!(
                (ow, oh),
                (resized.width(), resized.height()),
                "output_dimensions disagreed with resize_image for {w}x{h} -> {target}"
            );
        }
    }

    #[test]
    fn resize_narrows_wide_image() {
        let img = DynamicImage::new_rgb8(1000, 500);
        let resized = resize_image(img, 200);
        assert_eq!(resized.width(), 200);
        assert_eq!(resized.height(), 100);
    }

    #[test]
    fn resize_leaves_narrow_image_unchanged() {
        let img = DynamicImage::new_rgb8(100, 50);
        let resized = resize_image(img, 200);
        assert_eq!(resized.width(), 100);
        assert_eq!(resized.height(), 50);
    }

    #[test]
    fn resize_preserves_aspect_ratio() {
        let img = DynamicImage::new_rgb8(800, 600);
        let resized = resize_image(img, 400);
        assert_eq!(resized.width(), 400);
        assert_eq!(resized.height(), 300);
    }

    #[test]
    fn resize_never_produces_zero_height() {
        let img = DynamicImage::new_rgb8(10000, 1);
        let resized = resize_image(img, 1);
        assert!(resized.height() >= 1);
    }

    #[test]
    fn encode_jpeg_produces_jpeg_header() {
        let img = tiny_jpeg();
        let (bytes, mime) = encode_image(img, OutputFormat::Jpeg).unwrap();
        assert_eq!(mime, "image/jpeg");
        assert!(bytes.len() >= 3);
        assert_eq!(&bytes[..3], &[0xFF, 0xD8, 0xFF]);
    }

    #[test]
    fn encode_webp_produces_webp_riff_header() {
        let img = tiny_jpeg();
        let (bytes, mime) = encode_image(img, OutputFormat::Webp).unwrap();
        assert_eq!(mime, "image/webp");
        assert!(bytes.len() >= 12);
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WEBP");
    }

    #[test]
    fn encode_avif_produces_non_empty_output() {
        let img = tiny_jpeg();
        let (bytes, mime) = encode_image(img, OutputFormat::Avif).unwrap();
        assert_eq!(mime, "image/avif");
        assert!(!bytes.is_empty());
    }

    #[test]
    fn output_format_str_is_correct() {
        assert_eq!(OutputFormat::Avif.as_str(), "avif");
        assert_eq!(OutputFormat::Webp.as_str(), "webp");
        assert_eq!(OutputFormat::Jpeg.as_str(), "jpeg");
    }
}
