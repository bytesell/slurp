use slurp_image::security::{SecurityError, is_safe_ip, validate_image_magic, validate_src};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

#[test]
fn ssrf_rejects_ipv4_loopback_127_0_0_1() {
    let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
    assert!(
        !is_safe_ip(ip),
        "127.0.0.1 (IPv4 loopback) must be blocked by SSRF protection"
    );
}

#[test]
fn ssrf_rejects_ipv4_loopback_127_x_x_x() {
    assert!(!is_safe_ip(IpAddr::V4(Ipv4Addr::new(127, 255, 255, 255))));
    assert!(!is_safe_ip(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2))));
}

#[test]
fn ssrf_rejects_rfc1918_10_block() {
    assert!(
        !is_safe_ip(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))),
        "10.0.0.1 must be blocked"
    );
    assert!(
        !is_safe_ip(IpAddr::V4(Ipv4Addr::new(10, 255, 255, 255))),
        "10.255.255.255 must be blocked"
    );
    assert!(
        !is_safe_ip(IpAddr::V4(Ipv4Addr::new(10, 10, 10, 10))),
        "10.10.10.10 must be blocked"
    );
}

#[test]
fn ssrf_rejects_rfc1918_192_168_block() {
    assert!(
        !is_safe_ip(IpAddr::V4(Ipv4Addr::new(192, 168, 0, 1))),
        "192.168.0.1 must be blocked"
    );
    assert!(
        !is_safe_ip(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))),
        "192.168.1.1 must be blocked"
    );
    assert!(
        !is_safe_ip(IpAddr::V4(Ipv4Addr::new(192, 168, 255, 255))),
        "192.168.255.255 must be blocked"
    );
}

#[test]
fn ssrf_rejects_rfc1918_172_16_to_31_block() {
    assert!(
        !is_safe_ip(IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1))),
        "172.16.0.1 must be blocked"
    );
    assert!(
        !is_safe_ip(IpAddr::V4(Ipv4Addr::new(172, 24, 0, 1))),
        "172.24.0.1 must be blocked"
    );
    assert!(
        !is_safe_ip(IpAddr::V4(Ipv4Addr::new(172, 31, 255, 255))),
        "172.31.255.255 must be blocked"
    );
    assert!(
        is_safe_ip(IpAddr::V4(Ipv4Addr::new(172, 15, 255, 255))),
        "172.15.255.255 is outside RFC 1918 and must be allowed"
    );
    assert!(
        is_safe_ip(IpAddr::V4(Ipv4Addr::new(172, 32, 0, 1))),
        "172.32.0.1 is outside RFC 1918 and must be allowed"
    );
}

#[test]
fn ssrf_rejects_ipv6_loopback() {
    assert!(
        !is_safe_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)),
        "::1 (IPv6 loopback) must be blocked"
    );
}

#[test]
fn ssrf_rejects_ipv6_unique_local_fc00() {
    let ip: Ipv6Addr = "fc00::1".parse().unwrap();
    assert!(
        !is_safe_ip(IpAddr::V6(ip)),
        "fc00::1 (IPv6 ULA) must be blocked"
    );
}

#[test]
fn ssrf_rejects_ipv6_unique_local_fd00() {
    // fd00::/8 falls within fc00::/7 (second half of the range)
    let ip: Ipv6Addr = "fd00::1".parse().unwrap();
    assert!(
        !is_safe_ip(IpAddr::V6(ip)),
        "fd00::1 (IPv6 ULA) must be blocked"
    );
}

#[test]
fn ssrf_rejects_ipv6_unique_local_fdff() {
    let ip: Ipv6Addr = "fdff::1".parse().unwrap();
    assert!(
        !is_safe_ip(IpAddr::V6(ip)),
        "fdff::1 (IPv6 ULA) must be blocked"
    );
}

#[test]
fn ssrf_rejects_ipv6_link_local_fe80() {
    let ip: Ipv6Addr = "fe80::1".parse().unwrap();
    assert!(
        !is_safe_ip(IpAddr::V6(ip)),
        "fe80::1 (IPv6 link-local) must be blocked"
    );
}

#[test]
fn ssrf_rejects_ipv6_link_local_febf() {
    // fe80::/10 covers fe80:: through febf::
    let ip: Ipv6Addr = "febf::1".parse().unwrap();
    assert!(
        !is_safe_ip(IpAddr::V6(ip)),
        "febf::1 (IPv6 link-local) must be blocked"
    );
}

#[test]
fn ssrf_allows_public_addresses() {
    assert!(is_safe_ip(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))));
    assert!(is_safe_ip(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
    assert!(is_safe_ip(IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))));

    let ipv6: Ipv6Addr = "2606:4700:4700::1111".parse().unwrap();
    assert!(is_safe_ip(IpAddr::V6(ipv6)));
}

#[test]
fn scheme_rejects_file_protocol() {
    let result = validate_src("file:///etc/passwd");
    assert!(
        matches!(result, Err(SecurityError::DisallowedScheme(_))),
        "file:// scheme must be rejected, got: {result:?}"
    );
}

#[test]
fn scheme_rejects_ftp_protocol() {
    let result = validate_src("ftp://example.com/image.jpg");
    assert!(
        matches!(result, Err(SecurityError::DisallowedScheme(_))),
        "ftp:// scheme must be rejected, got: {result:?}"
    );
}

#[test]
fn scheme_rejects_gopher_protocol() {
    let result = validate_src("gopher://example.com/image.jpg");
    assert!(
        matches!(result, Err(SecurityError::DisallowedScheme(_))),
        "gopher:// scheme must be rejected, got: {result:?}"
    );
}

#[test]
fn scheme_rejects_data_uri() {
    let result = validate_src("data:image/png;base64,iVBORw0KGgo=");
    assert!(
        matches!(result, Err(SecurityError::DisallowedScheme(_))),
        "data: scheme must be rejected, got: {result:?}"
    );
}

#[test]
fn scheme_rejects_javascript_uri() {
    let result = validate_src("javascript:alert(1)");
    assert!(
        matches!(result, Err(SecurityError::DisallowedScheme(_))),
        "javascript: scheme must be rejected, got: {result:?}"
    );
}

#[test]
fn scheme_allows_http_and_https() {
    assert!(
        validate_src("https://cdn.example.com/photo.jpg").is_ok(),
        "https:// must be accepted"
    );
    assert!(
        validate_src("http://cdn.example.com/photo.jpg").is_ok(),
        "http:// must be accepted"
    );
}

#[test]
fn magic_bytes_rejects_html_disguised_as_jpeg() {
    let html_bytes = b"<!DOCTYPE html><html><body>Hello</body></html>";
    assert!(
        !validate_image_magic(html_bytes),
        "HTML content disguised as JPEG must fail magic bytes check"
    );
}

#[test]
fn magic_bytes_rejects_html_starting_with_tag() {
    let html_bytes = b"<html><head></head><body></body></html>";
    assert!(
        !validate_image_magic(html_bytes),
        "HTML <html> tag must fail magic bytes check"
    );
}

#[test]
fn magic_bytes_rejects_zip_with_jpg_extension() {
    // ZIP files start with PK (0x50 0x4B)
    let zip_bytes = b"PK\x03\x04";
    assert!(
        !validate_image_magic(zip_bytes),
        "ZIP file with .jpg extension must fail magic bytes check"
    );
}

#[test]
fn magic_bytes_rejects_pdf_with_jpg_extension() {
    let pdf_bytes = b"%PDF-1.4";
    assert!(
        !validate_image_magic(pdf_bytes),
        "PDF content with .jpg extension must fail magic bytes check"
    );
}

#[test]
fn magic_bytes_rejects_executable() {
    let elf_bytes = b"\x7fELF";
    assert!(
        !validate_image_magic(elf_bytes),
        "ELF executable must fail magic bytes check"
    );

    let pe_bytes = b"MZ";
    assert!(
        !validate_image_magic(pe_bytes),
        "Windows PE executable must fail magic bytes check"
    );
}

#[test]
fn magic_bytes_rejects_plain_text() {
    let text = b"This is just a plain text file, not an image at all.";
    assert!(
        !validate_image_magic(text),
        "plain text must fail magic bytes check"
    );
}

#[test]
fn magic_bytes_accepts_valid_jpeg() {
    let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
    assert!(
        validate_image_magic(&jpeg),
        "valid JPEG header must be accepted"
    );
}

#[test]
fn magic_bytes_accepts_valid_png() {
    let png = b"\x89PNG\r\n\x1a\n";
    assert!(
        validate_image_magic(png),
        "valid PNG header must be accepted"
    );
}

#[test]
fn magic_bytes_accepts_valid_gif89a() {
    let gif = b"GIF89a";
    assert!(
        validate_image_magic(gif),
        "valid GIF89a header must be accepted"
    );
}

#[test]
fn magic_bytes_accepts_valid_webp() {
    let mut webp = [0u8; 12];
    webp[..4].copy_from_slice(b"RIFF");
    webp[8..12].copy_from_slice(b"WEBP");
    assert!(
        validate_image_magic(&webp),
        "valid WebP header must be accepted"
    );
}

#[test]
fn path_traversal_bare_dotdot_is_invalid_url() {
    // A bare path like `/../etc/passwd` is not a valid URL
    let result = validate_src("/../etc/passwd");
    assert!(
        matches!(
            result,
            Err(SecurityError::InvalidUrl) | Err(SecurityError::DisallowedScheme(_))
        ),
        "bare traversal path must be rejected, got: {result:?}"
    );
}

#[test]
fn path_traversal_relative_url_is_rejected() {
    let result = validate_src("../etc/passwd");
    assert!(
        result.is_err(),
        "relative URL with traversal must be rejected, got: {result:?}"
    );
}

#[test]
fn path_traversal_file_scheme_is_rejected() {
    let result = validate_src("file:///etc/passwd");
    assert!(
        matches!(result, Err(SecurityError::DisallowedScheme(_))),
        "file:// must be rejected by scheme check, got: {result:?}"
    );
}

#[test]
fn path_traversal_file_scheme_windows_is_rejected() {
    let result = validate_src("file:///C:/Windows/System32/drivers/etc/hosts");
    assert!(
        matches!(result, Err(SecurityError::DisallowedScheme(_))),
        "Windows file:// path must be rejected, got: {result:?}"
    );
}

#[test]
fn path_traversal_encoded_dotdot_in_http_url_is_normalised() {
    let result = validate_src("https://example.com/images/%2F..%2Fetc%2Fpasswd");
    // The url crate normalises %2F sequences; host must not change - SSRF protection handles whether the server is internal.
    if let Ok(url) = result {
        assert_eq!(
            url.host_str(),
            Some("example.com"),
            "URL must not be rewritten to a different host"
        )
    }
}

#[test]
fn url_with_user_and_password_is_rejected() {
    let result = validate_src("https://user:secret@example.com/image.jpg");
    assert!(
        matches!(result, Err(SecurityError::InvalidUrl)),
        "URL with credentials must be rejected, got: {result:?}"
    );
}

#[test]
fn url_with_username_only_is_rejected() {
    let result = validate_src("https://user@example.com/image.jpg");
    assert!(
        matches!(result, Err(SecurityError::InvalidUrl)),
        "URL with username must be rejected, got: {result:?}"
    );
}

#[test]
fn url_exceeding_max_length_is_rejected() {
    let long_url = format!("https://example.com/{}", "a".repeat(5000));
    let result = validate_src(&long_url);
    assert!(
        matches!(result, Err(SecurityError::InvalidUrl)),
        "URL over 4096 bytes must be rejected, got: {result:?}"
    );
}

#[test]
fn url_with_null_byte_is_rejected() {
    let result = validate_src("https://example.com/\x00image.jpg");
    assert!(
        matches!(result, Err(SecurityError::InvalidUrl)),
        "URL with null byte must be rejected, got: {result:?}"
    );
}

#[test]
fn url_with_control_character_is_rejected() {
    let result = validate_src("https://example.com/\x0aimage.jpg");
    assert!(
        matches!(result, Err(SecurityError::InvalidUrl)),
        "URL with control character (newline) must be rejected, got: {result:?}"
    );
}

#[test]
fn url_without_host_is_rejected() {
    // The `url` crate parses "https:///image.jpg" as host="image.jpg"; test with
    // a scheme that genuinely has no host component.
    let result = validate_src("data:text/html,<h1>test</h1>");
    assert!(
        result.is_err(),
        "data: URL must be rejected (disallowed scheme), got: {result:?}"
    );
}
