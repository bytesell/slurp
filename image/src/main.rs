use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use axum::{Router, routing::get};
use clap::Parser;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tower_http::timeout::TimeoutLayer;
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use slurp_image::cache::ImageCache;
use slurp_image::config::{Config, default_concurrency};
use slurp_image::handler::{AppState, RateLimiter, image_handler};

#[derive(Parser, Debug)]
#[command(name = "slurp-image", about = "Slurp on-demand image optimiser")]
struct Args {
    /// TCP port to listen on
    #[arg(long, default_value = "3001")]
    port: u16,

    /// Address to bind. Defaults to loopback; pass 0.0.0.0 to accept connections from the network.
    /// This service fetches attacker-supplied URLs, so exposing it should be a deliberate act.
    #[arg(long, default_value = "127.0.0.1")]
    bind: IpAddr,

    /// Allowed CORS origin. Repeatable. Pass `*` to allow any origin. When omitted no CORS headers
    /// are sent at all, which is correct for the usual deployment behind a reverse proxy on the
    /// same origin.
    #[arg(long = "cors-allow-origin", value_name = "ORIGIN")]
    cors_allow_origin: Vec<String>,

    /// Maximum images decoded/resized/encoded at once. Defaults to the number of available cores.
    #[arg(long)]
    max_concurrent_transforms: Option<usize>,

    /// Whole-request deadline in seconds.
    #[arg(long, default_value_t = 30)]
    request_timeout_secs: u64,

    /// Maximum output pixels (width x height) after resizing.
    #[arg(long, default_value_t = 8_000_000)]
    max_output_pixels: u64,

    /// Directory for the persistent on-disk image cache
    #[arg(long, default_value = "/tmp/slurp-image-cache")]
    cache_dir: PathBuf,

    /// Cache TTL in seconds
    #[arg(long, default_value = "3600")]
    cache_ttl: u64,

    /// In-memory cache capacity (number of entries)
    #[arg(long, default_value = "512")]
    memory_cache_size: u64,

    /// Max total on-disk cache size in bytes; a background sweeper evicts to stay under it
    #[arg(long, default_value_t = 2 * 1024 * 1024 * 1024)]
    max_disk_cache_bytes: u64,

    /// How often the disk-cache sweeper runs, in seconds
    #[arg(long, default_value_t = 300)]
    disk_sweep_interval_secs: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "slurp_image=info,tower_http=warn".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let args = Args::parse();

    let mut config = Config::new(args.port, args.cache_ttl);
    config.memory_cache_size = args.memory_cache_size;
    config.max_disk_bytes = args.max_disk_cache_bytes;
    config.disk_sweep_interval_secs = args.disk_sweep_interval_secs;
    config.request_timeout_secs = args.request_timeout_secs;
    config.max_output_pixels = args.max_output_pixels;
    config.max_concurrent_transforms = args
        .max_concurrent_transforms
        .filter(|n| *n > 0)
        .unwrap_or_else(default_concurrency);

    let transform_permits = Arc::new(Semaphore::new(config.max_concurrent_transforms));
    let rate_limiter = RateLimiter::with_capacity(
        config.rate_limit_requests,
        config.rate_limit_window_secs,
        config.rate_limit_max_entries,
    );

    // Sized from `config`, not from `args`. `Config` is what the rest of the process consults, so a
    // cache built straight from the flags would silently ignore any value a caller of the library
    // set on `Config` instead.
    let state = Arc::new(AppState {
        cache: ImageCache::new(
            args.cache_dir,
            config.cache_ttl,
            config.memory_cache_size,
            config.max_disk_bytes,
        ),
        rate_limiter,
        transform_permits,
        config,
    });

    // Background disk-cache sweeper: bounds total on-disk cache size off the request path. The
    // write path additionally sweeps out of band once a burst has written a fraction of the budget,
    // so growth is bounded between these ticks too.
    {
        let state = state.clone();
        let interval = std::time::Duration::from_secs(state.config.disk_sweep_interval_secs.max(1));
        let max = state.config.max_disk_bytes;
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(interval);
            loop {
                tick.tick().await;
                let state = state.clone();
                let _ = tokio::task::spawn_blocking(move || state.cache.sweep(max)).await;
            }
        });
    }

    // Rate-limit pruning runs here, on a timer, rather than on the request path.
    {
        let state = state.clone();
        let interval =
            std::time::Duration::from_secs(state.config.rate_limit_window_secs.max(1) * 2);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(interval);
            loop {
                tick.tick().await;
                state.rate_limiter.prune();
            }
        });
    }

    let mut app = Router::new().route("/_slurp/image", get(image_handler));

    // SECURITY (CORS): no CORS headers unless an origin is configured. Defaulting to
    // `allow_origin(Any)` would let any page on the internet drive this service from a visitor's
    // browser.
    if let Some(cors) = build_cors(&args.cors_allow_origin)? {
        app = app.layer(cors);
    }

    // A whole-request deadline, so a slow upstream plus a slow transform cannot hold a connection
    // (and a transform permit) indefinitely.
    let app = app
        .layer(TimeoutLayer::new(std::time::Duration::from_secs(
            state.config.request_timeout_secs.max(1),
        )))
        .with_state(state.clone());

    let addr = SocketAddr::new(args.bind, state.config.port);
    let listener = TcpListener::bind(addr).await?;

    if !args.bind.is_loopback() {
        warn!(%addr, "listening on a non-loopback address; ensure a reverse proxy and rate limits sit in front");
    }
    info!(
        %addr,
        max_concurrent_transforms = state.config.max_concurrent_transforms,
        "slurp-image listening"
    );

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;

    Ok(())
}

/// Build the CORS layer from `--cors-allow-origin`. `None` means send no CORS headers at all.
fn build_cors(origins: &[String]) -> anyhow::Result<Option<CorsLayer>> {
    if origins.is_empty() {
        return Ok(None);
    }

    let layer = CorsLayer::new().allow_methods([axum::http::Method::GET]);

    if origins.iter().any(|o| o == "*") {
        warn!("CORS is configured to allow any origin");
        return Ok(Some(layer.allow_origin(Any)));
    }

    let parsed = origins
        .iter()
        .map(|o| {
            o.parse::<axum::http::HeaderValue>()
                .map_err(|_| anyhow::anyhow!("invalid --cors-allow-origin value: {o}"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    Ok(Some(layer.allow_origin(AllowOrigin::list(parsed))))
}
