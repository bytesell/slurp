mod compiler;
mod errors;
mod fixtures;
mod hmr;
mod security;
mod server;
mod watcher;

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use tracing::info;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

use crate::{
    errors::{DevResult, DevServerError},
    hmr::HmrHub,
    server::{ServerState, run},
    watcher::{build_source, spawn_source_watcher, spawn_watcher},
};

/// Slurp development server - hot-reload theme files with WebSocket HMR.
#[derive(Parser, Debug)]
#[command(name = "slurp-dev", version, about)]
struct Args {
    /// TCP port for the HTTP server (also used as the HMR WebSocket port).
    #[arg(long, default_value = "3000")]
    port: u16,

    /// Path to the theme directory to serve.
    #[arg(long, default_value = ".")]
    theme_dir: PathBuf,

    /// Path to the source directory (site/) for SSR on dynamic routes.
    #[arg(long)]
    source_dir: Option<PathBuf>,

    /// Backend URL to proxy /api/* and /auth/* requests to, and to resolve
    /// relative {fetch} URLs in SSR templates (e.g. http://localhost:8080).
    #[arg(long)]
    backend_url: Option<String>,

    /// An extra hostname this dev server will answer to, repeatable. By default it answers only
    /// to localhost, 127.0.0.1 and [::1]: any other Host header is refused, because a hostname
    /// outside this machine resolving to 127.0.0.1 is how a DNS rebinding attack reads a dev
    /// server from a web page. Name a hosts-file entry here to opt it in.
    #[arg(long = "allow-host", value_name = "HOST")]
    allow_host: Vec<String>,

    /// A CSS build command to run in watch mode alongside the dev server, e.g.
    /// "tailwindcss -i styles/app.css -o dist/css/theme.css --watch". slurp-dev
    /// stays CSS-agnostic: it spawns the command as given (and kills it on exit),
    /// so template + CSS changes hot-reload together.
    #[arg(long)]
    css_watch_cmd: Option<String>,

    /// Path to a fixtures file (or a directory containing `fixtures.json`): a JSON object mapping
    /// a request path to the canned JSON response for it. When set, {fetch} URLs and /api, /auth
    /// requests are answered from these fixtures instead of a live backend, so a theme can be
    /// previewed with no backend running. Can be combined with --backend-url (fixtures win; a
    /// miss falls through to the backend).
    #[arg(long)]
    fixtures: Option<PathBuf>,

    /// Path to a JSON file of globals seeded into every SSR render context - the same file passed to
    /// `slurp build --globals`, so a dynamic route resolves platform values (`${ platform.alpine }`)
    /// exactly like a statically built page does.
    #[arg(long)]
    globals: Option<PathBuf>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(
            EnvFilter::from_default_env()
                .add_directive("slurp_dev_server=info".parse().expect("valid directive")),
        )
        .init();

    if let Err(e) = run_server().await {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

async fn run_server() -> DevResult<()> {
    let args = Args::parse();

    let theme_dir = args
        .theme_dir
        .canonicalize()
        .map_err(|_| DevServerError::ThemeDirNotFound(args.theme_dir.display().to_string()))?;

    info!(
        port = args.port,
        theme_dir = %theme_dir.display(),
        "Starting Slurp dev server"
    );

    let hub = HmrHub::new();

    let _watcher_handle = spawn_watcher(theme_dir.clone(), hub.clone()).await?;

    let source_dir = args
        .source_dir
        .map(|p| p.canonicalize().unwrap_or(p))
        .map(Arc::new);

    let backend_url = args
        .backend_url
        .map(|u| Arc::new(u.trim_end_matches('/').to_string()));

    if let Some(ref url) = backend_url {
        info!(backend_url = %url, "proxying /api/* and /auth/* to backend");
    }

    let fixtures = match args.fixtures {
        Some(ref path) => match fixtures::Fixtures::load(path) {
            Ok(f) => {
                info!(
                    path = %path.display(),
                    routes = f.len(),
                    "serving {{fetch}} + /api, /auth from fixtures (no backend needed)"
                );
                Some(Arc::new(f))
            }
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        },
        None => None,
    };

    // Same contract as `slurp build --globals`: named-but-unusable is fatal, because the values it
    // carries are ones a page cannot render correctly without.
    let globals = match args.globals {
        Some(ref path) => match std::fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|raw| {
                serde_json::from_str::<serde_json::Value>(&raw).map_err(|e| e.to_string())
            }) {
            Ok(v @ serde_json::Value::Object(_)) => {
                info!(path = %path.display(), "seeding SSR context from globals file");
                Some(Arc::new(v))
            }
            Ok(_) => {
                eprintln!(
                    "error: globals file {} must contain a JSON object",
                    path.display()
                );
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("error: globals file {}: {e}", path.display());
                std::process::exit(1);
            }
        },
        None => None,
    };

    if !args.allow_host.is_empty() {
        info!(
            hosts = ?args.allow_host,
            "answering to these extra hostnames as well as localhost"
        );
    }

    let state = ServerState {
        theme_dir: Arc::new(theme_dir),
        source_dir,
        hub,
        port: args.port,
        backend_url,
        fixtures,
        globals,
        allowed_hosts: Arc::new(args.allow_host),
        http_client: reqwest::Client::new(),
    };

    // Health-check the backend only when we're actually proxying to one (not in fixtures mode).
    if let Some(ref backend_url) = state.backend_url {
        let check_url = format!("{backend_url}/api/config");
        match state.http_client.get(&check_url).send().await {
            Ok(resp) if resp.status().is_success() => {
                info!(backend_url = %backend_url, "Backend reachable: OK");
            }
            Ok(resp) => {
                tracing::warn!(
                    "backend at {} returned {} for {} - API calls may fail until it is configured.",
                    backend_url,
                    resp.status(),
                    check_url
                );
            }
            Err(_) => {
                tracing::warn!(
                    "backend at {} is not reachable yet - API calls will fail until it starts.",
                    backend_url
                );
            }
        }
    }

    if let Some(ref src) = state.source_dir {
        // Resolved once, here, rather than per rebuild. `current_exe()` fails precisely when the
        // binary is being replaced - a `cargo build` while the server runs - which is also exactly
        // when the rebuild loop would be asking. Resolving at startup turns that into a clear
        // startup error instead of an intermittent one mid-session.
        let slurp_bin = Arc::new(slurp_bin()?);
        build_source(&slurp_bin, src, &state.theme_dir).await?;
        spawn_source_watcher(
            slurp_bin,
            src.clone(),
            state.theme_dir.clone(),
            state.hub.clone(),
        );
    }

    // Held for the server's lifetime; kill_on_drop stops the watcher when we exit.
    let _css_watch = args.css_watch_cmd.as_deref().and_then(spawn_css_watch);

    run(state).await?;

    Ok(())
}

/// Spawn a CSS build/watch command (e.g. `tailwindcss ... --watch`) as a child of the dev server.
/// slurp does not know or care what CSS tool this is; it runs the command via the platform shell so
/// arguments and PATH (e.g. an npm `node_modules/.bin`) resolve as they would in a terminal.
fn spawn_css_watch(cmd: &str) -> Option<tokio::process::Child> {
    info!("CSS watch: {cmd}");
    let mut command = if cfg!(windows) {
        let mut c = tokio::process::Command::new("cmd");
        c.args(["/C", cmd]);
        c
    } else {
        let mut c = tokio::process::Command::new("sh");
        c.args(["-c", cmd]);
        c
    };
    command.kill_on_drop(true);
    match command.spawn() {
        Ok(child) => Some(child),
        Err(e) => {
            tracing::warn!("failed to start CSS watch command: {e}");
            None
        }
    }
}

/// Absolute path to the `slurp` CLI, which ships in the same directory as this binary.
fn slurp_bin() -> DevResult<PathBuf> {
    let exe = std::env::current_exe().map_err(|e| DevServerError::Io {
        path: "current executable".to_string(),
        source: e,
    })?;
    Ok(exe.with_file_name(if cfg!(windows) { "slurp.exe" } else { "slurp" }))
}
