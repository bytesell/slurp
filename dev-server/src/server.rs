use std::{collections::HashMap, net::SocketAddr, path::PathBuf, sync::Arc};

use axum::{
    Router,
    body::Body,
    extract::{ConnectInfo, Path, Request, State, ws::WebSocketUpgrade},
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::{any, get},
};
use tracing::{info, warn};

use crate::{
    hmr::HmrHub,
    security::{
        host_allowed, http_origin_allowed, is_contained, is_localhost, request_path_is_safe,
        safe_path, ws_origin_allowed,
    },
};

const HMR_CLIENT_JS: &str = r#"(function() {
  var port = (document.querySelector('meta[name="slurp-hmr-port"]') || {}).content || '3000';
  var ws;
  var overlay;

  function connect() {
    ws = new WebSocket('ws://localhost:' + port + '/_slurp/ws');

    ws.onopen = function() {
      if (overlay) {
        overlay.parentNode && overlay.parentNode.removeChild(overlay);
        overlay = null;
      }
    };

    ws.onmessage = function(e) {
      var msg;
      try { msg = JSON.parse(e.data); } catch (_) { return; }

      if (msg.type === 'reload') {
        location.reload();
      }

      if (msg.type === 'error') {
        showErrorOverlay(msg.errors);
      }

      if (msg.type === 'clear') {
        if (overlay) {
          overlay.parentNode && overlay.parentNode.removeChild(overlay);
          overlay = null;
        }
      }
    };

    ws.onclose = function() {
      setTimeout(connect, 1000);
    };

    ws.onerror = function() {
      ws.close();
    };
  }

  function showErrorOverlay(errors) {
    if (!overlay) {
      overlay = document.createElement('div');
      overlay.id = '__slurp_error_overlay__';
      overlay.style.cssText = [
        'position:fixed', 'inset:0', 'z-index:99999',
        'background:rgba(0,0,0,0.88)', 'color:#ff6b6b',
        'font-family:monospace', 'font-size:14px',
        'padding:2rem', 'overflow:auto', 'white-space:pre-wrap'
      ].join(';');
      document.body.appendChild(overlay);
    }

    var lines = ['[Slurp] Build error(s):\n'];
    (errors || []).forEach(function(err) {
      lines.push(
        '[' + (err.code || 'E') + '] ' +
        err.message +
        (err.file ? ' (' + err.file + ':' + err.line + ':' + err.column + ')' : '')
      );
    });
    overlay.textContent = lines.join('\n');
  }

  connect();
})();
"#;

const HMR_INJECT_TEMPLATE: &str = r#"<script>window.SLURP_MODE = 'dev'</script>
<meta name="slurp-hmr-port" content="PORT" />
<script src="/_slurp/hmr-client.js"></script>"#;

#[derive(Clone)]
pub struct ServerState {
    pub theme_dir: Arc<PathBuf>,
    pub source_dir: Option<Arc<PathBuf>>,
    pub hub: HmrHub,
    pub port: u16,
    /// Base URL for proxying /api/* and /auth/* and resolving relative {fetch} URLs.
    pub backend_url: Option<Arc<String>>,
    /// Offline data source: when set, {fetch} URLs and /api, /auth requests are answered from
    /// canned JSON instead of (or before) the backend, so a theme renders with no backend.
    pub fixtures: Option<Arc<crate::fixtures::Fixtures>>,
    /// Globals seeded into every SSR context (`--globals`), matching what `slurp build --globals`
    /// baked into the static pages. Route params still win - they are the more specific value.
    pub globals: Option<Arc<serde_json::Value>>,
    /// Extra hostnames this dev server will answer to (`--allow-host`), for the developer who
    /// points a hosts-file entry at it. Empty by default: loopback names only.
    pub allowed_hosts: Arc<Vec<String>>,
    pub http_client: reqwest::Client,
}

/// THE ONLY PLACE A ROUTE MAY BE ADDED. See [`build_router`] for why that matters.
fn routes() -> Router<ServerState> {
    Router::new()
        .route("/_slurp/ws", get(ws_handler))
        .route("/_slurp/hmr-client.js", get(hmr_client_js))
        // Proxy /api/* and /auth/* to backend when --backend-url is configured
        .route("/api/*path", any(backend_proxy))
        .route("/auth/*path", any(backend_proxy))
        .route("/*path", get(static_file_handler))
        .route("/", get(index_handler))
}

pub fn build_router(state: ServerState) -> Router {
    // SECURITY: `Router::layer` wraps only the routes registered BEFORE it. A route added AFTER
    // this call is NOT guarded - the opposite of a guarantee that every handler is covered.
    //
    // Nothing is unguarded today, and the split above is what keeps it that way: routes live in
    // `routes()`, this function only wraps the finished table, so there is no tempting `.route(`
    // sitting after the `.layer(` for someone to append to. Adding a route here instead would
    // ship it unauthenticated into a server that proxies to the developer's own backend with
    // their cookies attached.
    //
    // `every_route_is_behind_the_local_only_guard` is the check; extend its path list when you
    // add a route.
    routes()
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            local_only_guard,
        ))
        .with_state(state)
}

/// SECURITY: rejects requests that are not really coming from this machine's own dev session.
///
/// Two independent lies a browser can be made to tell, both closed here:
/// - a rebound `Host` header, which makes an attacker's domain same-origin with the dev server;
/// - a foreign `Origin`, which is how a cross-site simple request turns the backend proxy into a
///   confused deputy (no preflight is sent for those, so CORS never gets a say).
async fn local_only_guard(
    State(state): State<ServerState>,
    req: Request,
    next: axum::middleware::Next,
) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    if !host_allowed(host.as_deref(), &state.allowed_hosts) {
        let host = host.unwrap_or_default();
        warn!(host = %host, "rejected request: Host header is not this machine");
        return (
            StatusCode::FORBIDDEN,
            format!(
                "403 Forbidden: slurp-dev only answers to localhost, 127.0.0.1 and [::1].\n\
                 This request arrived with Host: {host}\n\n\
                 That usually means a hostname outside your machine resolves to 127.0.0.1, which \
                 is how a DNS rebinding attack reads your dev server from a web page.\n\
                 If {host} is your own hosts-file entry, restart with: --allow-host {host}\n"
            ),
        )
            .into_response();
    }

    let origin = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    if !http_origin_allowed(origin.as_deref(), state.port, &state.allowed_hosts) {
        let origin = origin.unwrap_or_default();
        warn!(origin = %origin, "rejected cross-origin request");
        return (
            StatusCode::FORBIDDEN,
            format!(
                "403 Forbidden: slurp-dev refuses cross-origin requests.\n\
                 This request came from Origin: {origin}\n\n\
                 Only pages served by this dev server itself may call it.\n"
            ),
        )
            .into_response();
    }

    next.run(req).await
}

async fn hmr_client_js() -> impl IntoResponse {
    Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(HMR_CLIENT_JS))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    // Defence in depth, but NOT what stops a malicious page: that page runs in the developer's
    // own browser, so its TCP peer is loopback like any other tab.
    if !is_localhost(&addr) {
        warn!(addr = %addr, "rejected non-localhost WebSocket connection");
        return StatusCode::FORBIDDEN.into_response();
    }

    // SECURITY: this is the check that actually prevents cross-site WebSocket hijacking.
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    if !ws_origin_allowed(origin, state.port, &state.allowed_hosts) {
        warn!(
            addr = %addr,
            origin = origin.unwrap_or("<none>"),
            "rejected WebSocket upgrade: Origin is not this dev server"
        );
        return StatusCode::FORBIDDEN.into_response();
    }

    let hub = state.hub.clone();
    ws.on_upgrade(move |socket| async move {
        hub.handle_socket(socket, addr).await;
    })
}

async fn index_handler(State(state): State<ServerState>) -> impl IntoResponse {
    serve_theme_path(&state, "", None).await
}

async fn static_file_handler(
    Path(path): Path<String>,
    State(state): State<ServerState>,
    req: Request,
) -> impl IntoResponse {
    let cookie = req
        .headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    serve_theme_path(&state, &path, cookie.as_deref()).await
}

async fn backend_proxy(
    State(state): State<ServerState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let path_and_query = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");
    // Already validated by `local_only_guard`: a present Origin here is one of ours.
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());

    // Fixtures win: serve canned JSON with no backend round-trip.
    if let Some(ref fixtures) = state.fixtures {
        if let Some(value) = fixtures.lookup(path_and_query) {
            return json_response(value, origin);
        }
        // No fixture, and no backend to fall through to: a helpful 404 (record this path).
        if state.backend_url.is_none() {
            warn!(path = path_and_query, "no fixture for request path");
            return (
                StatusCode::NOT_FOUND,
                format!("no fixture for {path_and_query} (add it to your fixtures file)"),
            )
                .into_response();
        }
    }

    let Some(ref base_url) = state.backend_url else {
        return (
            StatusCode::NOT_FOUND,
            "no --backend-url or --fixtures configured",
        )
            .into_response();
    };

    let target_url = format!("{base_url}{path_and_query}");

    let method =
        reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap_or(reqwest::Method::GET);

    let mut req = state.http_client.request(method, &target_url);
    if let Some(v) = headers.get(header::COOKIE) {
        req = req.header(reqwest::header::COOKIE, v.as_bytes());
    }
    if let Some(v) = headers.get(header::CONTENT_TYPE) {
        req = req.header(reqwest::header::CONTENT_TYPE, v.as_bytes());
    }
    if let Some(v) = headers.get(header::AUTHORIZATION) {
        req = req.header(reqwest::header::AUTHORIZATION, v.as_bytes());
    }
    req = req.body(body.to_vec());

    match req.send().await {
        Ok(resp) => {
            let status = StatusCode::from_u16(resp.status().as_u16())
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            let mut builder = axum::http::Response::builder().status(status);
            for (name, value) in resp.headers() {
                match name.as_str() {
                    // SECURITY: `access-control-allow-origin` is deliberately NOT relayed. The
                    // upstream's CORS policy was written for the upstream's own origin; replaying
                    // it here would let a permissive dev backend (`*` is common) hand a foreign
                    // page read access to responses this proxy fetched with the developer's
                    // cookies. What this proxy allows is this proxy's decision, not upstream's.
                    "content-type" | "set-cookie" | "location" | "cache-control" => {
                        builder = builder.header(name.as_str(), value.as_bytes());
                    }
                    _ => {}
                }
            }
            let bytes = resp.bytes().await.unwrap_or_default();
            builder
                .body(Body::from(bytes))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Err(e) => {
            warn!(url = target_url, error = %e, "backend proxy error");
            (StatusCode::BAD_GATEWAY, "upstream error").into_response()
        }
    }
}

/// Build a JSON `200` response for a fixture value with no-store caching, so the theme's browser
/// JS can read it exactly as it would a backend response.
///
/// SECURITY: CORS is scoped to the dev origin, never `*`. `origin` has already been through
/// `local_only_guard`, so a value present here is one of ours and is echoed back; a request with
/// no Origin is same-origin (or not a browser) and needs no CORS header at all.
fn json_response(value: &serde_json::Value, origin: Option<&str>) -> Response {
    let body = serde_json::to_vec(value).unwrap_or_default();
    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store");
    if let Some(origin) = origin {
        builder = builder
            .header("access-control-allow-origin", origin)
            .header(header::VARY, "origin");
    }
    builder
        .body(Body::from(body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

async fn serve_theme_path(state: &ServerState, requested: &str, cookie: Option<&str>) -> Response {
    let theme_dir: &PathBuf = &state.theme_dir;
    let clean = requested.trim_start_matches('/');

    // SECURITY: the traversal guard lives HERE, at the top, so every branch below inherits it.
    // `safe_path` protects the static-file loop only; the dynamic-route branch builds its own
    // path from these same segments, so a guard confined to `safe_path` would leave it uncovered.
    // A 404 rather than a 403: a traversal is never a legitimate dev request, and there is
    // nothing to tell the caller.
    if !request_path_is_safe(clean) {
        warn!(
            requested,
            "rejected request path that escapes the theme root"
        );
        return not_found();
    }

    // Build candidate paths to try in order.
    let candidates: Vec<String> = if std::path::Path::new(clean).extension().is_some() {
        // Has a file extension - serve as a static asset directly from theme dir.
        vec![clean.to_string()]
    } else {
        // No extension - route to pages/ subdirectory.
        if clean.is_empty() {
            vec!["pages/index.html".to_string()]
        } else {
            vec![
                format!("pages/{}.html", clean),
                format!("pages/{}/index.html", clean),
            ]
        }
    };

    for candidate in &candidates {
        let abs_path = match safe_path(theme_dir, candidate) {
            Some(p) => p,
            None => continue,
        };
        let bytes = match tokio::fs::read(&abs_path).await {
            Ok(b) => b,
            Err(_) => continue,
        };
        let mime = mime_for_path(&abs_path);
        if is_html_mime(mime) {
            let html = String::from_utf8_lossy(&bytes);
            let injected = inject_hmr(&html, state.port);
            return Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                .header(header::CACHE_CONTROL, "no-store")
                .body(Body::from(injected))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
        }
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, HeaderValue::from_static(mime))
            .header(header::CACHE_CONTROL, "no-store")
            .body(Body::from(bytes))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
    }

    // Dynamic route fallback: only for extensionless paths (pages, not assets)
    if std::path::Path::new(clean).extension().is_none()
        && let Some((bracket_file, param_name, param_value)) =
            find_dynamic_candidate(theme_dir.as_path(), requested)
    {
        // SECURITY: defence in depth behind the shape guard above. `find_dynamic_candidate`
        // joins raw URL segments, so this re-verifies AFTER canonicalisation - which is also
        // what catches a symlink inside pages/ pointing out of the theme.
        if !is_contained(theme_dir.as_path(), &bracket_file) {
            warn!(
                requested,
                file = %bracket_file.display(),
                "rejected dynamic route resolving outside the theme root"
            );
            return not_found();
        }
        // Try SSR if we have a source dir
        if let Some(ref source_dir) = state.source_dir
            && let Some(html) = ssr_render(
                source_dir.as_path(),
                theme_dir.as_path(),
                bracket_file.as_path(),
                &param_name,
                &param_value,
                cookie,
                state.backend_url.as_deref().map(|s| s.as_str()),
                state.fixtures.as_deref(),
                state.globals.as_deref(),
                &state.http_client,
            )
            .await
        {
            let injected = inject_hmr(&html, state.port);
            return Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                .header(header::CACHE_CONTROL, "no-store")
                .body(Body::from(injected))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
        }
        // Fallback: serve the pre-built bracket file as-is (shows skeleton)
        if let Ok(bytes) = tokio::fs::read(&bracket_file).await {
            let html = String::from_utf8_lossy(&bytes);
            let injected = inject_hmr(&html, state.port);
            return Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                .header(header::CACHE_CONTROL, "no-store")
                .body(Body::from(injected))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
        }
    }

    warn!(requested, "Page not found");
    not_found()
}

/// Scans `theme_dir/pages/` for a bracket file matching `path`.
/// Returns `(abs_bracket_html_path, param_name, param_value)`.
fn find_dynamic_candidate(
    theme_dir: &std::path::Path,
    path: &str,
) -> Option<(PathBuf, String, String)> {
    let clean = path.trim_start_matches('/');
    if clean.is_empty() {
        return None;
    }
    let segments: Vec<&str> = clean.split('/').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return None;
    }
    let pages_dir = theme_dir.join("pages");

    // Try from deepest to shallowest: dir = segments[0..end-1], param = segments[end-1]
    for end in (1..=segments.len()).rev() {
        let dir_path = if end == 1 {
            pages_dir.clone()
        } else {
            pages_dir.join(segments[..end - 1].join("/"))
        };
        let param_value = segments[end - 1].to_string();

        if let Ok(entries) = std::fs::read_dir(&dir_path) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with('[') && name.ends_with("].html") {
                    let param_name = name[1..name.len() - 6].to_string();
                    return Some((dir_path.join(&name), param_name, param_value));
                }
            }
        }
    }
    None
}

/// Renders a dynamic route by pre-fetching API data and re-compiling the source template.
// SSR needs the full request/route context (dirs, params, cookie, backend, client); grouping it into
// a struct would just move the noise without adding meaning.
#[allow(clippy::too_many_arguments)]
async fn ssr_render(
    source_dir: &std::path::Path,
    theme_dir: &std::path::Path,
    bracket_html: &std::path::Path,
    param_name: &str,
    param_value: &str,
    cookie_header: Option<&str>,
    backend_url: Option<&str>,
    fixtures: Option<&crate::fixtures::Fixtures>,
    globals: Option<&serde_json::Value>,
    client: &reqwest::Client,
) -> Option<String> {
    let rel = bracket_html.strip_prefix(theme_dir.join("pages")).ok()?;
    let source_file = source_dir.join("pages").join(rel.with_extension("slurp"));
    // SECURITY: `rel` comes from a path built out of raw URL segments, so re-verify containment
    // after canonicalisation before READING AND COMPILING the file. This branch is the worst of
    // the two: it does not merely serve bytes, it runs an arbitrary .slurp through the compiler.
    if !is_contained(source_dir, &source_file) {
        warn!(
            file = %source_file.display(),
            "refused to SSR a source file outside the source root"
        );
        return None;
    }
    let source = tokio::fs::read_to_string(&source_file).await.ok()?;
    let source_str = source_file.to_string_lossy().to_string();

    // Globals first, then the route param on top - the param is the more specific value and a
    // globals file must never be able to shadow it.
    let mut context = match globals {
        Some(serde_json::Value::Object(map)) => serde_json::Value::Object(map.clone()),
        _ => serde_json::json!({}),
    };
    context[param_name] = serde_json::Value::String(param_value.to_string());

    let (doc, _) = slurp_compiler::parse(&source, &source_str).ok()?;
    let fetch_urls = slurp_compiler::eval_fetch_urls(&doc, &context);

    for (var_name, url) in fetch_urls {
        // Fixtures win: resolve the fetch from canned JSON with no HTTP round-trip.
        if let Some(fixtures) = fixtures {
            if let Some(value) = fixtures.lookup(&url) {
                context[var_name.as_str()] = value.clone();
                continue;
            }
            // In pure fixtures mode a miss surfaces the {error} branch and logs the missing key.
            if backend_url.is_none() {
                warn!(
                    url,
                    "no fixture for {{fetch}} URL (add it to your fixtures file)"
                );
                context[var_name.as_str()] = serde_json::json!({ "__error": "fixture_missing" });
                continue;
            }
        }

        let resolved = if url.starts_with('/') {
            match backend_url {
                Some(base) => format!("{base}{url}"),
                None => {
                    warn!(
                        url,
                        "relative {{fetch}} URL skipped - no --backend-url configured"
                    );
                    continue;
                }
            }
        } else {
            url
        };
        let mut req = client.get(&resolved);
        if let Some(cookie) = cookie_header {
            req = req.header("Cookie", cookie);
        }
        if let Ok(resp) = req.send().await {
            if resp.status().is_success() {
                if let Ok(data) = resp.json::<serde_json::Value>().await {
                    context[var_name.as_str()] = data;
                }
            } else {
                context[var_name.as_str()] = serde_json::json!({
                    "__error": resp.status().as_str()
                });
            }
        }
    }

    let mut registry_map = HashMap::new();
    build_registry(source_dir, source_dir, &mut registry_map);
    let registry = Arc::new(registry_map);

    let (html, _) = slurp_compiler::compile_with_registry(
        &source,
        source_str,
        &context,
        slurp_compiler::BuildMode::Development,
        registry,
    );

    if html.is_empty() { None } else { Some(html) }
}

/// Recursively walks `dir` and inserts every `.slurp` file into `out`
/// keyed by its import path (e.g. `@sections/Header`).
fn build_registry(
    root: &std::path::Path,
    dir: &std::path::Path,
    out: &mut HashMap<String, String>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.starts_with('.')
                && name != "node_modules"
                && name != "dist"
                && name != "target"
            {
                build_registry(root, &path, out);
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("slurp")
            && let Ok(rel) = path.strip_prefix(root)
        {
            let key = format!(
                "@{}",
                rel.with_extension("").to_string_lossy().replace('\\', "/")
            );
            if let Ok(src) = std::fs::read_to_string(&path) {
                out.insert(key, src);
            }
        }
    }
}

fn inject_hmr(html: &str, port: u16) -> String {
    let snippet = HMR_INJECT_TEMPLATE.replace("PORT", &port.to_string());

    if let Some(idx) = find_case_insensitive(html, "</head>") {
        let mut out = String::with_capacity(html.len() + snippet.len() + 1);
        out.push_str(&html[..idx]);
        out.push('\n');
        out.push_str(&snippet);
        out.push('\n');
        out.push_str(&html[idx..]);
        return out;
    }

    if let Some(idx) = find_case_insensitive(html, "</body>") {
        let mut out = String::with_capacity(html.len() + snippet.len() + 1);
        out.push_str(&html[..idx]);
        out.push('\n');
        out.push_str(&snippet);
        out.push('\n');
        out.push_str(&html[idx..]);
        return out;
    }

    format!("{html}\n{snippet}\n")
}

/// Byte index of the first ASCII-case-insensitive match of `needle` in `haystack`.
///
/// THE INDEX MUST BE INTO `haystack` ITSELF, so the search has to run over the original bytes.
/// Folding the haystack first (`haystack.to_lowercase()`) returns an index into a DIFFERENT
/// string, and the caller slices the original: `to_lowercase` is Unicode-aware and does not
/// preserve byte length, so the two disagree on any page containing a character whose lowercase
/// form is a different size:
///
///   * U+0130 (`I` with dot) lowercases to two chars and grows the string by one byte, so the
///     index lands one byte PAST the real `<`, silently emitting `</ead>`;
///   * U+1E9E (capital sharp s) SHRINKS by one, so the index lands one byte BEFORE - and if the
///     preceding character is multi-byte, inside it, which panics on `byte index N is not a char
///     boundary`.
///
/// Matching ASCII-case-insensitively over the original bytes avoids both. The needles are ASCII
/// HTML tags (`</head>`, `</body>`), and an ASCII byte can never occur inside a multi-byte UTF-8
/// sequence, so any index this returns is a valid char boundary by construction.
///
/// ASCII-only folding is also the CORRECT rule here rather than a shortcut: HTML tag names are
/// ASCII, and Unicode folding would match `</heaİ>`-style lookalikes that are not tags at all.
fn find_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    debug_assert!(
        needle.is_ascii(),
        "find_case_insensitive folds ASCII only; a non-ASCII needle would not match as expected"
    );
    let hay = haystack.as_bytes();
    let ndl = needle.as_bytes();
    if ndl.is_empty() || hay.len() < ndl.len() {
        return None;
    }
    (0..=hay.len() - ndl.len()).find(|&i| hay[i..i + ndl.len()].eq_ignore_ascii_case(ndl))
}

fn mime_for_path(path: &std::path::Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js" | "mjs") => "application/javascript; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("txt" | "slurp") => "text/plain; charset=utf-8",
        Some("xml") => "application/xml",
        Some("wasm") => "application/wasm",
        _ => "application/octet-stream",
    }
}

fn is_html_mime(mime: &str) -> bool {
    mime.starts_with("text/html")
}

fn not_found() -> Response {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from("404 Not Found"))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

pub async fn run(state: ServerState) -> crate::errors::DevResult<()> {
    use crate::errors::DevServerError;

    let port = state.port;
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let router = build_router(state).into_make_service_with_connect_info::<SocketAddr>();

    info!(addr = %addr, "Slurp dev server listening");

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| DevServerError::Bind { port, source: e })?;

    axum::serve(listener, router)
        .await
        .map_err(|e| DevServerError::Io {
            path: addr.to_string(),
            source: e,
        })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inject_before_head_close() {
        let html = "<html><head><title>Test</title></head><body></body></html>";
        let out = inject_hmr(html, 3000);
        // The snippet must appear before </head>
        let snippet_pos = out.find("slurp-hmr-port").unwrap();
        let head_pos = out.to_lowercase().find("</head>").unwrap();
        assert!(snippet_pos < head_pos, "snippet must precede </head>");
    }

    #[test]
    fn inject_falls_back_to_body_close() {
        let html = "<html><body><p>Hello</p></body></html>";
        let out = inject_hmr(html, 3000);
        let snippet_pos = out.find("slurp-hmr-port").unwrap();
        let body_pos = out.to_lowercase().find("</body>").unwrap();
        assert!(snippet_pos < body_pos, "snippet must precede </body>");
    }

    /// REGRESSION: the injection index must be a byte offset into the page being sliced.
    ///
    /// See `find_case_insensitive` for the mechanism. The two characters below are the two
    /// directions it fails in, and both are ordinary text (German and Turkish): U+1E9E shrinks
    /// when lowercased, moving the index backwards into the preceding multi-byte character and
    /// PANICKING; U+0130 grows, moving it forwards and quietly emitting `</ead>`.
    #[test]
    fn inject_handles_text_whose_lowercase_form_is_a_different_byte_length() {
        // SHRINKS: U+1E9E -> U+00DF is 3 bytes -> 2, and the multi-byte `é` sits where the
        // displaced index would land.
        let shrinking = "<html><head><title>\u{1e9e}\u{e9}</title></head><body>x</body></html>";
        let out = inject_hmr(shrinking, 3000);
        assert!(out.contains("slurp-hmr-port"), "snippet missing: {out}");
        assert!(
            out.contains("</head>"),
            "the closing tag was corrupted: {out}"
        );
        assert!(
            out.contains('\u{1e9e}') && out.contains('\u{e9}'),
            "page text was mangled: {out}"
        );
        let snippet_pos = out.find("slurp-hmr-port").expect("snippet");
        assert!(
            snippet_pos < out.find("</head>").expect("head close"),
            "snippet must still precede </head>: {out}"
        );

        // GROWS: U+0130 -> "i" + U+0307 is 2 bytes -> 3.
        let growing = "<html><head><title>\u{130}\u{130}</title></head><body>x</body></html>";
        let out = inject_hmr(growing, 3000);
        assert!(out.contains("</head>"), "closing tag corrupted: {out}");
        assert!(
            out.find("slurp-hmr-port").expect("snippet") < out.find("</head>").expect("head"),
            "snippet must precede </head>: {out}"
        );

        // Same again on the `</body>` fallback path, which is a separate call site.
        let body_only = "<html><body>\u{1e9e}\u{e9}<p>x</p></body></html>";
        let out = inject_hmr(body_only, 3000);
        assert!(
            out.find("slurp-hmr-port").expect("snippet") < out.find("</body>").expect("body"),
            "snippet must precede </body>: {out}"
        );
    }

    /// The match itself stays case-insensitive over ASCII, which is what the function is for,
    /// and the index it returns indexes the ORIGINAL string.
    #[test]
    fn find_case_insensitive_returns_an_index_into_the_original_string() {
        let hay = "\u{1e9e}\u{e9}</HEAD>";
        let idx = find_case_insensitive(hay, "</head>").expect("must match regardless of case");
        assert!(
            hay.is_char_boundary(idx),
            "index {idx} is not a char boundary in the original string"
        );
        assert_eq!(&hay[idx..], "</HEAD>", "index points at the wrong place");
        assert_eq!(find_case_insensitive("abc", "</head>"), None);
        assert_eq!(find_case_insensitive("", "</head>"), None);
    }

    #[test]
    fn inject_appends_when_no_tags() {
        let html = "plain text";
        let out = inject_hmr(html, 3000);
        assert!(out.contains("slurp-hmr-port"));
        assert!(out.starts_with("plain text"));
    }

    #[test]
    fn port_substituted_correctly() {
        let out = inject_hmr("<html><head></head></html>", 4567);
        assert!(out.contains("content=\"4567\""));
    }

    #[test]
    fn mime_html_detected() {
        let p = std::path::Path::new("index.html");
        assert!(is_html_mime(mime_for_path(p)));
    }

    #[test]
    fn mime_css_not_html() {
        let p = std::path::Path::new("style.css");
        assert!(!is_html_mime(mime_for_path(p)));
    }

    // ---------------------------------------------------------------------------------------
    // End-to-end guard tests.
    //
    // These drive a REAL router over a REAL socket with hand-written HTTP, because that is the
    // only way to send the payloads that matter: an HTTP client library normalises `..` and
    // `%2e%2e` out of a URL before it ever reaches the wire, so a client-driven test would pass
    // whether or not the guard is present.
    // ---------------------------------------------------------------------------------------

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    struct TestServer {
        port: u16,
        _dir: tempdir::TempDir,
    }

    /// A theme root with a page, an asset and a bracket route, plus a sibling directory OUTSIDE
    /// the theme holding a file a traversal would reach.
    fn fixture_tree() -> (tempdir::TempDir, PathBuf) {
        let dir = tempdir::TempDir::new("slurp_dev_guard");
        let theme = dir.path().join("theme");
        std::fs::create_dir_all(theme.join("pages")).expect("create pages");
        std::fs::create_dir_all(dir.path().join("outside")).expect("create outside");
        std::fs::write(
            theme.join("pages/index.html"),
            "<html><head></head><body>HOME PAGE OK</body></html>",
        )
        .expect("write index");
        std::fs::write(
            theme.join("pages/[slug].html"),
            "<html><head></head><body>BRACKET PAGE OK</body></html>",
        )
        .expect("write bracket");
        std::fs::write(theme.join("style.css"), "body{color:red}").expect("write css");
        std::fs::write(
            dir.path().join("outside/[anything].html"),
            "<html><head></head><body>SECRET OUTSIDE FILE</body></html>",
        )
        .expect("write outside");
        (dir, theme)
    }

    async fn start_test_server() -> TestServer {
        let (dir, theme) = fixture_tree();
        // Bind first: the port is part of the Origin check, so state needs the real one.
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind");
        let port = listener.local_addr().expect("local_addr").port();

        let hub = HmrHub::new();
        // No watcher is started: `spawn_watcher` parks a blocking thread forever by design, and a
        // parked blocking task stops the test's tokio runtime from ever shutting down.
        let fixtures = crate::fixtures::Fixtures::from_str(r#"{"/api/config":{"name":"Demo"}}"#)
            .expect("fixtures");

        let state = ServerState {
            theme_dir: Arc::new(theme),
            source_dir: None,
            hub,
            port,
            backend_url: None,
            fixtures: Some(Arc::new(fixtures)),
            globals: None,
            allowed_hosts: Arc::new(Vec::new()),
            http_client: reqwest::Client::new(),
        };

        let router = build_router(state).into_make_service_with_connect_info::<SocketAddr>();
        tokio::spawn(async move {
            axum::serve(listener, router).await.ok();
        });

        TestServer { port, _dir: dir }
    }

    /// Write a raw request and read whatever comes back. Bounded by a timeout because a 101 leaves
    /// the socket open forever by design.
    async fn raw(port: u16, request: &str) -> String {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("connect");
        stream
            .write_all(request.as_bytes())
            .await
            .expect("write request");
        let mut buf = Vec::new();
        let _ = tokio::time::timeout(
            std::time::Duration::from_millis(1500),
            stream.read_to_end(&mut buf),
        )
        .await;
        String::from_utf8_lossy(&buf).into_owned()
    }

    fn get(port: u16, path: &str, extra_headers: &str) -> String {
        format!(
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{extra_headers}Connection: close\r\n\r\n"
        )
    }

    fn status_line(response: &str) -> &str {
        response.lines().next().unwrap_or("").trim_end()
    }

    // --- the guard really does cover every route ---

    /// `Router::layer` wraps only the routes registered BEFORE it, so "applied to every route" is
    /// a property of how `build_router` is WRITTEN, not something axum enforces. A route appended
    /// after the `.layer(` call is silently unguarded, and this server proxies to the developer's
    /// backend carrying their cookies - so an unguarded route is a confused deputy reachable from
    /// any web page they happen to have open.
    ///
    /// This drives the assembled router with a rebound `Host`, which the guard refuses and no
    /// handler does, so a 403 can only have come from the middleware. Every path the server
    /// answers is listed; ADD TO THIS LIST when you add a route. Two entries are structural
    /// rather than incidental: `/` is a separate registration from the `/*path` catch-all, and
    /// `/_slurp/ws` is the WebSocket upgrade, which is the one an "it is only a GET handler"
    /// reading would most likely leave out.
    #[tokio::test]
    async fn every_route_is_behind_the_local_only_guard() {
        let s = start_test_server().await;
        for path in [
            "/",
            "/_slurp/hmr-client.js",
            "/_slurp/ws",
            "/api/config",
            "/auth/session",
            "/index.html",
            "/some/deep/asset.css",
            // A path no route matches: it still passes through the layer, so it must be refused
            // BEFORE routing decides there is nothing there.
            "/definitely-not-a-route",
        ] {
            let req =
                format!("GET {path} HTTP/1.1\r\nHost: evil.example\r\nConnection: close\r\n\r\n");
            let resp = raw(s.port, &req).await;
            assert!(
                status_line(&resp).contains("403"),
                "{path} is not behind local_only_guard (got {}); a route registered AFTER the \
                 .layer() call in build_router is not wrapped by it",
                status_line(&resp)
            );
        }
    }

    /// The same list under a foreign `Origin`, which is the other half of the guard. A route can
    /// only be reached cross-site through a simple request (no preflight is sent), so the Origin
    /// check is what stops the backend proxy being driven from another page.
    #[tokio::test]
    async fn every_route_refuses_a_foreign_origin_too() {
        let s = start_test_server().await;
        for path in ["/", "/_slurp/hmr-client.js", "/api/config", "/auth/session"] {
            let resp = raw(
                s.port,
                &get(s.port, path, "Origin: https://evil.example\r\n"),
            )
            .await;
            assert!(
                status_line(&resp).contains("403"),
                "{path} accepted a foreign Origin (got {})",
                status_line(&resp)
            );
        }
    }

    // --- path traversal ---

    #[tokio::test]
    async fn traversal_is_refused_in_every_encoding() {
        let s = start_test_server().await;
        for path in [
            "/../../outside/anything",
            "/%2e%2e/%2e%2e/outside/anything",
            "/..%2f..%2foutside/anything",
            "/pages/../../outside/anything",
        ] {
            let resp = raw(s.port, &get(s.port, path, "")).await;
            assert!(
                !resp.contains("SECRET OUTSIDE FILE"),
                "traversal served a file outside the theme root via {path}:\n{resp}"
            );
            assert!(
                status_line(&resp).contains("404"),
                "expected 404 for {path}, got: {}",
                status_line(&resp)
            );
        }
    }

    // --- Host / DNS rebinding ---

    #[tokio::test]
    async fn rebinding_host_is_refused_and_says_why() {
        let s = start_test_server().await;
        for host in [
            "evil.example",
            "rebind.attacker.test",
            "localhost.evil.example",
        ] {
            let req = format!("GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
            let resp = raw(s.port, &req).await;
            assert!(
                status_line(&resp).contains("403"),
                "expected 403 for Host: {host}, got: {}",
                status_line(&resp)
            );
            assert!(
                !resp.contains("HOME PAGE OK"),
                "body leaked for Host: {host}"
            );
            // The developer with a real hosts-file entry has to be able to tell what happened.
            assert!(
                resp.contains("--allow-host"),
                "403 body must name the escape hatch, got:\n{resp}"
            );
        }
    }

    #[tokio::test]
    async fn loopback_hosts_still_work() {
        let s = start_test_server().await;
        for host in [
            format!("localhost:{}", s.port),
            format!("127.0.0.1:{}", s.port),
            format!("[::1]:{}", s.port),
            "localhost".to_string(),
        ] {
            let req = format!("GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
            let resp = raw(s.port, &req).await;
            assert!(
                resp.contains("HOME PAGE OK"),
                "normal request with Host: {host} must still work, got:\n{resp}"
            );
        }
    }

    // --- the proxy as a confused deputy ---

    #[tokio::test]
    async fn cross_origin_request_to_the_proxy_is_refused() {
        let s = start_test_server().await;
        for method in ["GET", "POST", "PUT", "DELETE", "PATCH"] {
            let req = format!(
                "{method} /api/config HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\
                 Origin: https://evil.example\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                s.port
            );
            let resp = raw(s.port, &req).await;
            assert!(
                status_line(&resp).contains("403"),
                "cross-origin {method} reached the proxy, got: {}",
                status_line(&resp)
            );
            assert!(
                !resp.contains("Demo"),
                "proxy body leaked cross-origin for {method}"
            );
        }
    }

    #[tokio::test]
    async fn same_origin_proxy_call_still_works_and_scopes_cors() {
        let s = start_test_server().await;
        let origin = format!("Origin: http://localhost:{}\r\n", s.port);
        let resp = raw(s.port, &get(s.port, "/api/config", &origin)).await;
        assert!(
            resp.contains("Demo"),
            "same-origin proxy call broke:\n{resp}"
        );
        let lower = resp.to_lowercase();
        assert!(
            !lower.contains("access-control-allow-origin: *"),
            "fixture CORS must not be a wildcard:\n{resp}"
        );
        assert!(
            lower.contains(&format!(
                "access-control-allow-origin: http://localhost:{}",
                s.port
            )),
            "fixture CORS must echo the dev origin:\n{resp}"
        );
    }

    // --- cross-site WebSocket hijacking ---

    fn ws_handshake(port: u16, origin_header: &str) -> String {
        format!(
            "GET /_slurp/ws HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\
             Upgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
             {origin_header}\r\n"
        )
    }

    #[tokio::test]
    async fn websocket_rejects_foreign_null_and_missing_origin() {
        let s = start_test_server().await;
        for origin in [
            "Origin: https://evil.example\r\n",
            "Origin: http://evil.example\r\n",
            "Origin: null\r\n",
            "Origin: http://localhost:1\r\n",
            "", // no Origin at all
        ] {
            let resp = raw(s.port, &ws_handshake(s.port, origin)).await;
            assert!(
                !status_line(&resp).contains("101"),
                "WebSocket upgrade accepted for [{}], got: {}",
                origin.trim(),
                status_line(&resp)
            );
        }
    }

    #[tokio::test]
    async fn websocket_accepts_the_dev_servers_own_origin() {
        let s = start_test_server().await;
        for origin in [
            format!("Origin: http://localhost:{}\r\n", s.port),
            format!("Origin: http://127.0.0.1:{}\r\n", s.port),
            format!("Origin: http://[::1]:{}\r\n", s.port),
        ] {
            let resp = raw(s.port, &ws_handshake(s.port, &origin)).await;
            assert!(
                status_line(&resp).contains("101"),
                "HMR must still connect from [{}], got: {}",
                origin.trim(),
                status_line(&resp)
            );
        }
    }

    // --- the ordinary developer workflow must survive all of the above ---

    #[tokio::test]
    async fn normal_requests_are_untouched() {
        let s = start_test_server().await;

        let home = raw(s.port, &get(s.port, "/", "")).await;
        assert!(home.contains("HOME PAGE OK"), "home page broke:\n{home}");
        assert!(
            home.contains("slurp-hmr-port"),
            "HMR snippet missing:\n{home}"
        );

        let css = raw(s.port, &get(s.port, "/style.css", "")).await;
        assert!(css.contains("body{color:red}"), "asset broke:\n{css}");

        // A dynamic bracket route INSIDE the theme must still resolve.
        let dynamic = raw(s.port, &get(s.port, "/some-product", "")).await;
        assert!(
            dynamic.contains("BRACKET PAGE OK"),
            "dynamic route broke:\n{dynamic}"
        );

        // The HMR client script is what the injected snippet asks for.
        let js = raw(s.port, &get(s.port, "/_slurp/hmr-client.js", "")).await;
        assert!(js.contains("_slurp/ws"), "HMR client script broke:\n{js}");
    }

    /// Minimal scoped temp directory, removed on drop. Not worth a dependency.
    mod tempdir {
        use std::path::{Path, PathBuf};

        pub struct TempDir(PathBuf);

        impl TempDir {
            pub fn new(prefix: &str) -> Self {
                let unique = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0);
                let path = std::env::temp_dir().join(format!("{prefix}_{unique}"));
                std::fs::create_dir_all(&path).expect("create temp dir");
                Self(path)
            }

            pub fn path(&self) -> &Path {
                &self.0
            }
        }

        impl Drop for TempDir {
            fn drop(&mut self) {
                std::fs::remove_dir_all(&self.0).ok();
            }
        }
    }
}
