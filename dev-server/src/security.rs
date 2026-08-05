use std::net::IpAddr;
use std::path::{Component, Path, PathBuf};

/// SECURITY: Primary guard against path-traversal attacks. Every static file
/// request MUST pass through this function before the file is opened.
#[must_use]
pub fn safe_path(root: &Path, requested: &str) -> Option<PathBuf> {
    // Strip leading slashes so root.join() does not treat the path as absolute.
    let stripped = requested.trim_start_matches('/');

    if stripped.is_empty() {
        return None;
    }

    // Reject `..` before canonicalization as defence-in-depth.
    if !request_path_is_safe(stripped) {
        return None;
    }

    let joined = root.join(stripped);

    // canonicalize returns Err when the path does not exist - treat as 404.
    let canonical = joined.canonicalize().ok()?;

    // Canonicalize root too so symlinks in the theme path don't break the prefix check.
    let canonical_root = root.canonicalize().ok()?;

    if canonical.starts_with(&canonical_root) {
        Some(canonical)
    } else {
        None
    }
}

/// SECURITY: Shape check for a raw request path, applied BEFORE any branch turns it into a
/// filesystem path.
///
/// `safe_path` covers the static-file branch, but the dynamic-route and SSR branches build their
/// own paths, so a guard that lives only inside `safe_path` is one every other branch skips. This
/// is the one every branch inherits: reject `..` (however it was encoded - the HTTP layer has
/// already percent-decoded by the time we see it), a drive prefix such as `C:`, and embedded NULs.
///
/// It does NOT reject an absolute path, and does not need to: every leading `/` is trimmed first,
/// which turns `//etc/passwd` into the relative `etc/passwd` and keeps `root.join()` inside the
/// root. That is deliberate - a doubled slash is a typo in a stylesheet href far more often than
/// it is an attack, and 404ing it would be a confusing dev-server bug. `leading_slashes_are_
/// trimmed_not_rejected` pins the behaviour.
///
/// A consequence worth knowing before you read the match below: because of that trim,
/// `Component::RootDir` is UNREACHABLE on Unix. It is still matched because on Windows a path
/// like `/\windows\win.ini` survives the `/` trim and reaches `components()` as a root, which
/// `root.join()` would then honour by discarding the root entirely. So the arm is Windows
/// defence, not dead code, and removing it is a Windows-only path escape.
#[must_use]
pub fn request_path_is_safe(requested: &str) -> bool {
    if requested.contains('\0') {
        return false;
    }
    // On Windows `components()` also splits on `\`, so a `..\..` payload is caught here too.
    // On Unix `\` is an ordinary filename character and cannot traverse, so nothing is lost.
    for component in Path::new(requested.trim_start_matches('/')).components() {
        if matches!(
            component,
            Component::ParentDir | Component::Prefix(_) | Component::RootDir
        ) {
            return false;
        }
    }
    true
}

/// SECURITY: Post-canonicalisation containment check for a path that was NOT built by
/// [`safe_path`] - the dynamic-route bracket file and the SSR source file. Both sides are
/// canonicalised so a symlink cannot escape, and the comparison is component-wise (`starts_with`
/// on `Path`), never textual, so `/theme-evil` cannot pass as being inside `/theme`.
#[must_use]
pub fn is_contained(root: &Path, candidate: &Path) -> bool {
    let (Ok(root), Ok(candidate)) = (root.canonicalize(), candidate.canonicalize()) else {
        return false;
    };
    candidate.starts_with(root)
}

/// SECURITY: Reject connections that did not arrive from the local machine.
///
/// This is a TCP-peer check only. It is defence in depth and NOT by itself a defence against
/// cross-site attacks: a malicious page in the developer's own browser IS a local peer.
/// See [`ws_origin_allowed`] and [`http_origin_allowed`] for the browser-facing half.
#[must_use]
pub fn is_localhost(addr: &std::net::SocketAddr) -> bool {
    addr.ip().is_loopback()
}

/// Split a `Host`/origin authority into its hostname and optional port, handling the bracketed
/// IPv6 form (`[::1]:3000`). Returns `None` when the authority is malformed, so callers fail
/// closed.
fn split_host_port(authority: &str) -> Option<(String, Option<u16>)> {
    let authority = authority.trim();
    if authority.is_empty() {
        return None;
    }
    if let Some(rest) = authority.strip_prefix('[') {
        let (inner, tail) = rest.split_once(']')?;
        let port = match tail {
            "" => None,
            p => Some(p.strip_prefix(':')?.parse().ok()?),
        };
        return Some((inner.to_string(), port));
    }
    match authority.rsplit_once(':') {
        Some((name, port)) => Some((name.to_string(), Some(port.parse().ok()?))),
        None => Some((authority.to_string(), None)),
    }
}

/// True when a hostname names this machine's loopback interface.
fn host_name_is_loopback(name: &str) -> bool {
    if name.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match name.parse::<IpAddr>() {
        Ok(ip) => ip.is_loopback(),
        Err(_) => false,
    }
}

/// SECURITY: Reject a `Host` header naming anything but this machine, which is what makes DNS
/// rebinding fail.
///
/// Binding to `127.0.0.1` does not stop rebinding: the attacker's own domain is made to resolve
/// to `127.0.0.1`, the browser then treats `http://evil.example:3000` as same-origin with the dev
/// server, and every response the dev server produces becomes cross-origin readable. The Host
/// header is the only place that lie is visible, so it is the only place it can be caught.
///
/// An ABSENT Host header is allowed: it carries no name to rebind, and rejecting it would break
/// HTTP/1.0 clients for no gain. `extra` holds operator-supplied `--allow-host` values for the
/// developer who deliberately points a hosts-file entry at the dev server.
#[must_use]
pub fn host_allowed(host: Option<&str>, extra: &[String]) -> bool {
    let Some(raw) = host else {
        return true;
    };
    let Some((name, _port)) = split_host_port(raw) else {
        return false;
    };
    host_name_is_loopback(&name) || extra.iter().any(|h| h.eq_ignore_ascii_case(&name))
}

/// True when `origin` is this dev server's own origin.
///
/// Loopback origins must match the scheme (`http`), a loopback host and OUR port exactly - another
/// local dev app on a different port is still a different origin and must not be able to read us.
/// An operator-supplied `--allow-host` entry matches on hostname alone, because the operator who
/// added it chose the scheme and port their proxy uses.
#[must_use]
pub fn origin_is_local(origin: &str, port: u16, extra: &[String]) -> bool {
    let origin = origin.trim();
    // `null` is what a sandboxed iframe, a `data:` document or a redirected request sends. It is
    // opaque by definition, so it can never be proven to be us.
    if origin.eq_ignore_ascii_case("null") {
        return false;
    }
    let (is_http, rest) = match origin.strip_prefix("http://") {
        Some(rest) => (true, rest),
        None => match origin.strip_prefix("https://") {
            Some(rest) => (false, rest),
            None => return false,
        },
    };
    // An Origin is `scheme://host[:port]` and nothing else - a path means it is not an Origin.
    if rest.contains('/') {
        return false;
    }
    let Some((name, origin_port)) = split_host_port(rest) else {
        return false;
    };
    if extra.iter().any(|h| h.eq_ignore_ascii_case(&name)) {
        return true;
    }
    is_http && host_name_is_loopback(&name) && origin_port == Some(port)
}

/// SECURITY: The real defence against cross-site WebSocket hijacking.
///
/// WebSockets have no same-origin policy and no preflight, so any page on the internet can open a
/// socket to `ws://localhost:3000/_slurp/ws` from the victim's browser and read every frame the
/// dev server pushes - which includes page paths and build errors carrying absolute local
/// filesystem paths. The Origin header is browser-controlled and unforgeable by page script, so it
/// is the check that works.
///
/// A MISSING or `null` Origin is rejected. Unlike a plain HTTP navigation, a legitimate browser
/// WebSocket handshake always carries one; anything without it is either not a browser or is
/// deliberately hiding, and neither needs HMR.
#[must_use]
pub fn ws_origin_allowed(origin: Option<&str>, port: u16, extra: &[String]) -> bool {
    match origin {
        Some(origin) => origin_is_local(origin, port, extra),
        None => false,
    }
}

/// SECURITY: Origin check for ordinary HTTP requests, including the backend proxy.
///
/// An ABSENT Origin is allowed - a top-level navigation and a plain `curl` both send none, and
/// those are the normal way a developer uses the dev server. A PRESENT but foreign Origin is
/// exactly the confused-deputy case: a cross-site `fetch`/form POST reaching the developer's own
/// backend through our proxy, which needs no preflight when it is a simple request.
#[must_use]
pub fn http_origin_allowed(origin: Option<&str>, port: u16, extra: &[String]) -> bool {
    match origin {
        Some(origin) => origin_is_local(origin, port, extra),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

    #[test]
    fn localhost_ipv4() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 3000);
        assert!(is_localhost(&addr));
    }

    #[test]
    fn localhost_ipv6() {
        let addr = SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 3000);
        assert!(is_localhost(&addr));
    }

    #[test]
    fn non_localhost_rejected() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), 3000);
        assert!(!is_localhost(&addr));
    }

    #[test]
    fn traversal_explicit_dotdot() {
        let root = std::env::temp_dir();
        // Explicit ../ in the URL - must be rejected.
        assert!(safe_path(&root, "/../etc/passwd").is_none());
        assert!(safe_path(&root, "../etc/passwd").is_none());
    }

    #[test]
    fn empty_path_rejected() {
        let root = std::env::temp_dir();
        assert!(safe_path(&root, "").is_none());
        assert!(safe_path(&root, "/").is_none());
    }

    // --- request path shape ---

    #[test]
    fn request_path_rejects_traversal() {
        assert!(!request_path_is_safe("../../outside/anything"));
        assert!(!request_path_is_safe("/../../outside/anything"));
        assert!(!request_path_is_safe("pages/../../etc/passwd"));
        assert!(!request_path_is_safe(".."));
        assert!(!request_path_is_safe("a/b/.."));
    }

    #[test]
    fn request_path_rejects_nul() {
        assert!(!request_path_is_safe("ok/\0/bad"));
    }

    #[test]
    fn leading_slashes_are_trimmed_not_rejected() {
        // Trimming every leading slash leaves a relative path, which `root.join()` keeps inside
        // the root. This is not a traversal, so it must not 404 a legitimate `//css/a.css` typo.
        assert!(request_path_is_safe("//etc/passwd"));
        assert!(request_path_is_safe("///a/b"));
    }

    #[cfg(windows)]
    #[test]
    fn request_path_rejects_windows_absolute_forms() {
        // A drive prefix or a root would make `root.join()` discard the root entirely.
        // These are Windows-only: on Unix `C:` and `\etc` are ordinary filenames.
        assert!(!request_path_is_safe("C:/windows/win.ini"));
        assert!(!request_path_is_safe("/\\windows\\win.ini"));
        assert!(!request_path_is_safe("a/..\\..\\outside"));
    }

    #[test]
    fn request_path_allows_normal_paths() {
        assert!(request_path_is_safe(""));
        assert!(request_path_is_safe("index.html"));
        assert!(request_path_is_safe("/css/theme.css"));
        assert!(request_path_is_safe("products/widget"));
        assert!(request_path_is_safe("./a/b"));
        // A filename that merely STARTS with dots is a normal name, not a traversal.
        assert!(request_path_is_safe("..hidden/file.txt"));
    }

    #[test]
    fn containment_rejects_escape_and_sibling_prefix() {
        let base = std::env::temp_dir().join("slurp_contain_test");
        let inside = base.join("root").join("nested");
        let sibling = base.join("root-evil");
        std::fs::create_dir_all(&inside).expect("create inside");
        std::fs::create_dir_all(&sibling).expect("create sibling");
        let root = base.join("root");

        assert!(is_contained(&root, &inside));
        assert!(is_contained(&root, &root));
        // Textual prefix match would wrongly accept `root-evil` as inside `root`.
        assert!(!is_contained(&root, &sibling));
        assert!(!is_contained(&root, &base));
        // A path that does not exist canonicalises to Err, so it fails closed.
        assert!(!is_contained(&root, &root.join("does-not-exist")));

        std::fs::remove_dir_all(&base).ok();
    }

    // --- Origin: cross-site request and WebSocket hijacking ---

    #[test]
    fn ws_origin_accepts_only_our_own_loopback_origin() {
        let none: [String; 0] = [];
        assert!(ws_origin_allowed(
            Some("http://localhost:3000"),
            3000,
            &none
        ));
        assert!(ws_origin_allowed(
            Some("http://127.0.0.1:3000"),
            3000,
            &none
        ));
        assert!(ws_origin_allowed(Some("http://[::1]:3000"), 3000, &none));
    }

    #[test]
    fn ws_origin_rejects_foreign_null_and_missing() {
        let none: [String; 0] = [];
        assert!(!ws_origin_allowed(
            Some("https://evil.example"),
            3000,
            &none
        ));
        assert!(!ws_origin_allowed(Some("http://evil.example"), 3000, &none));
        assert!(!ws_origin_allowed(Some("null"), 3000, &none));
        assert!(!ws_origin_allowed(None, 3000, &none));
        // A different local port is still a different origin.
        assert!(!ws_origin_allowed(
            Some("http://localhost:9999"),
            3000,
            &none
        ));
        // A hostname that merely CONTAINS localhost is not localhost.
        assert!(!ws_origin_allowed(
            Some("http://localhost.evil.example:3000"),
            3000,
            &none
        ));
        // No port means it is not our origin.
        assert!(!ws_origin_allowed(Some("http://localhost"), 3000, &none));
    }

    #[test]
    fn http_origin_allows_absent_but_not_foreign() {
        let none: [String; 0] = [];
        // A top-level navigation and plain curl send no Origin - the normal dev path.
        assert!(http_origin_allowed(None, 3000, &none));
        assert!(http_origin_allowed(
            Some("http://127.0.0.1:3000"),
            3000,
            &none
        ));
        assert!(!http_origin_allowed(
            Some("https://evil.example"),
            3000,
            &none
        ));
        assert!(!http_origin_allowed(Some("null"), 3000, &none));
    }

    #[test]
    fn allow_host_opt_in_extends_origin_and_host() {
        let extra = [String::from("mytheme.test")];
        assert!(ws_origin_allowed(
            Some("http://mytheme.test:3000"),
            3000,
            &extra
        ));
        assert!(ws_origin_allowed(
            Some("https://mytheme.test"),
            3000,
            &extra
        ));
        assert!(host_allowed(Some("mytheme.test:3000"), &extra));
        // Opting one host in must not open the door to every host.
        assert!(!host_allowed(Some("other.test"), &extra));
    }

    // --- Host: DNS rebinding ---

    #[test]
    fn host_accepts_loopback_forms() {
        let none: [String; 0] = [];
        assert!(host_allowed(Some("localhost:3000"), &none));
        assert!(host_allowed(Some("localhost"), &none));
        assert!(host_allowed(Some("127.0.0.1:3000"), &none));
        assert!(host_allowed(Some("127.0.0.53"), &none));
        assert!(host_allowed(Some("[::1]:3000"), &none));
        assert!(host_allowed(Some("[::1]"), &none));
        // No Host header carries no name to rebind onto.
        assert!(host_allowed(None, &none));
    }

    #[test]
    fn host_rejects_rebinding_names() {
        let none: [String; 0] = [];
        assert!(!host_allowed(Some("evil.example"), &none));
        assert!(!host_allowed(Some("rebind.attacker.test:3000"), &none));
        assert!(!host_allowed(Some("localhost.evil.example"), &none));
        assert!(!host_allowed(Some("192.168.1.10:3000"), &none));
        assert!(!host_allowed(Some(""), &none));
    }
}
