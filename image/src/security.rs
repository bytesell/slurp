use std::net::IpAddr;

pub fn validate_src(src: &str) -> Result<url::Url, SecurityError> {
    if src.len() > 4096 {
        return Err(SecurityError::InvalidUrl);
    }

    if src.bytes().any(|b| b < 0x20 || b == 0x7f) {
        return Err(SecurityError::InvalidUrl);
    }

    let url = src
        .parse::<url::Url>()
        .map_err(|_| SecurityError::InvalidUrl)?;

    match url.scheme() {
        "http" | "https" => {}
        s => return Err(SecurityError::DisallowedScheme(s.to_string())),
    }

    if url.host_str().is_none() {
        return Err(SecurityError::InvalidUrl);
    }

    // Userinfo in URLs is an SSRF vector - reject it.
    if !url.username().is_empty() || url.password().is_some() {
        return Err(SecurityError::InvalidUrl);
    }

    Ok(url)
}

/// SECURITY: SSRF protection - only globally routable unicast addresses are accepted.
///
/// This is an ALLOW-list, not a deny-list, and that direction is the point: a deny-list silently
/// permits every range nobody thought to enumerate (240.0.0.0/4, for instance, is easy to overlook
/// and is real internal space in some cloud fabrics). Anything outside the globally routable
/// unicast space fails closed.
#[must_use]
pub fn is_safe_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_safe_ipv4(v4),
        IpAddr::V6(v6) => is_safe_ipv6(v6),
    }
}

/// IPv4 blocks that are NOT globally routable unicast (IANA IPv4 Special-Purpose Address Registry).
/// Every entry lives inside first octet 1-223; the 0/8 and 224/3 space is excluded structurally by
/// `is_safe_ipv4` before this table is consulted.
const IPV4_RESERVED: &[([u8; 4], u8)] = &[
    ([10, 0, 0, 0], 8),      // RFC 1918 private
    ([100, 64, 0, 0], 10),   // shared address space (CGNAT, Tailscale)
    ([127, 0, 0, 0], 8),     // loopback
    ([169, 254, 0, 0], 16),  // link-local
    ([172, 16, 0, 0], 12),   // RFC 1918 private
    ([192, 0, 0, 0], 24),    // IETF protocol assignments
    ([192, 0, 2, 0], 24),    // TEST-NET-1
    ([192, 88, 99, 0], 24),  // 6to4 relay anycast (deprecated)
    ([192, 168, 0, 0], 16),  // RFC 1918 private
    ([198, 18, 0, 0], 15),   // benchmarking
    ([198, 51, 100, 0], 24), // TEST-NET-2
    ([203, 0, 113, 0], 24),  // TEST-NET-3
];

fn in_v4_cidr(octets: [u8; 4], base: [u8; 4], prefix_len: u8) -> bool {
    let addr = u32::from_be_bytes(octets);
    let net = u32::from_be_bytes(base);
    // prefix_len is always 1-32 in the table above; a /0 would need a separate branch because
    // shifting a u32 by 32 is undefined.
    let mask = u32::MAX << (32 - prefix_len);
    (addr & mask) == (net & mask)
}

fn is_safe_ipv4(v4: std::net::Ipv4Addr) -> bool {
    let octets = v4.octets();
    // ALLOW-list gate: globally routable unicast IPv4 lives entirely in 1.0.0.0 - 223.255.255.255.
    // This one comparison covers 0.0.0.0/8 ("this network", where any 0.x.y.z reaches localhost on
    // Linux), 224.0.0.0/4 (multicast), 240.0.0.0/4 (reserved, cloud-internal in some fabrics) and
    // 255.255.255.255 (broadcast) without needing an entry for each.
    if octets[0] == 0 || octets[0] >= 224 {
        return false;
    }
    !IPV4_RESERVED
        .iter()
        .any(|&(base, len)| in_v4_cidr(octets, base, len))
}

fn is_safe_ipv6(v6: std::net::Ipv6Addr) -> bool {
    // An IPv4-mapped (::ffff:a.b.c.d) or IPv4-compatible (::a.b.c.d) address connects to an IPv4
    // target on a dual-stack host, so it must pass the IPv4 checks. Ipv6Addr::is_loopback() etc. do
    // NOT see the embedded address (::ffff:127.0.0.1 is not ::1), so an attacker-controlled AAAA
    // record could otherwise reach loopback or cloud-metadata targets.
    if let Some(v4) = v6.to_ipv4_mapped().or_else(|| v6.to_ipv4()) {
        return is_safe_ipv4(v4);
    }

    let seg = v6.segments();

    // NAT64 well-known prefix 64:ff9b::/96 embeds an IPv4 in the low 32 bits; on a host with a
    // NAT64 gateway it routes to that IPv4, so an attacker AAAA like 64:ff9b::7f00:1 (127.0.0.1)
    // must face the IPv4 checks rather than look like a public IPv6. Checked before the 2000::/3
    // gate below, which would otherwise reject the whole prefix including its public targets.
    if seg[0] == 0x0064 && seg[1] == 0xff9b && seg[2..6] == [0, 0, 0, 0] {
        return is_safe_ipv4(embedded_v4(seg[6], seg[7]));
    }

    // ALLOW-list gate: global unicast IPv6 is 2000::/3. Everything else fails closed - ::1,
    // :: (unspecified), fc00::/7 (ULA), fe80::/10 (link-local), ff00::/8 (multicast),
    // 100::/64 (discard-only) and every unallocated block.
    if (seg[0] & 0xe000) != 0x2000 {
        return false;
    }

    // 2001:db8::/32 - documentation range; never a legitimate fetch target.
    if seg[0] == 0x2001 && seg[1] == 0x0db8 {
        return false;
    }
    // 2001::/32 - Teredo tunnelling; the low bits carry an obfuscated IPv4 that cannot be
    // meaningfully validated, so refuse the prefix outright.
    if seg[0] == 0x2001 && seg[1] == 0x0000 {
        return false;
    }
    // 2002::/16 - 6to4; segments 1 and 2 are the embedded IPv4, which must face the IPv4 checks.
    if seg[0] == 0x2002 {
        return is_safe_ipv4(embedded_v4(seg[1], seg[2]));
    }

    true
}

fn embedded_v4(hi: u16, lo: u16) -> std::net::Ipv4Addr {
    std::net::Ipv4Addr::new(
        (hi >> 8) as u8,
        (hi & 0xff) as u8,
        (lo >> 8) as u8,
        (lo & 0xff) as u8,
    )
}

/// The image formats this service will actually decode. This list is the single authority: it must
/// match the decoders enabled in Cargo.toml, and `processor` re-checks the decoder-guessed format
/// against it so the magic-byte check and the decoder can never disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceFormat {
    Jpeg,
    Png,
    Gif,
    WebP,
}

/// SECURITY: identify the format from the file header rather than the extension or Content-Type,
/// so a polyglot cannot be routed to a decoder this service never declared.
#[must_use]
pub fn detect_source_format(bytes: &[u8]) -> Option<SourceFormat> {
    // JPEG: FF D8 FF
    if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        return Some(SourceFormat::Jpeg);
    }
    // PNG: 89 50 4E 47 0D 0A 1A 0A
    if bytes.len() >= 8 && &bytes[..8] == b"\x89PNG\r\n\x1a\n" {
        return Some(SourceFormat::Png);
    }
    // GIF: GIF87a or GIF89a
    if bytes.len() >= 6 && (&bytes[..6] == b"GIF87a" || &bytes[..6] == b"GIF89a") {
        return Some(SourceFormat::Gif);
    }
    // WebP: RIFF....WEBP
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some(SourceFormat::WebP);
    }
    None
}

/// True when `bytes` carries an ISO base media (`ftyp`) header for a still-image brand.
///
/// SECURITY: the `ftyp` marker at bytes[4..8] is REQUIRED. Checking the brand at bytes[8..12] alone
/// would leave bytes 0..8 fully attacker-controlled, so a TIFF/EXR/DDS/BMP/QOI header carrying
/// `mif1` at offset 8 would pass, then be handed to whichever decoder the content really matched.
///
/// These brands are recognised but NOT decodable: no ISO-BMFF decoder is compiled in (AVIF is an
/// output format only). They are kept here so such a file is reported as an unsupported source
/// format rather than as corrupt data.
#[must_use]
pub fn is_iso_bmff_image(bytes: &[u8]) -> bool {
    if bytes.len() < 12 || &bytes[4..8] != b"ftyp" {
        return false;
    }
    let brand = &bytes[8..12];
    brand == b"avif" || brand == b"avis" || brand == b"heic" || brand == b"mif1"
}

/// SECURITY: Validate magic bytes rather than extension/Content-Type to prevent polyglot attacks.
///
/// Accepting a header here is necessary but not sufficient - `processor` additionally requires that
/// the format the decoder infers is one of the declared ones.
#[must_use]
pub fn validate_image_magic(bytes: &[u8]) -> bool {
    detect_source_format(bytes).is_some() || is_iso_bmff_image(bytes)
}

#[derive(Debug, thiserror::Error)]
pub enum SecurityError {
    #[error("Invalid URL")]
    InvalidUrl,
    #[error("Disallowed URL scheme: {0}")]
    DisallowedScheme(String),
    #[error("SSRF: target resolves to private/loopback address")]
    SsrfBlocked,
    #[error("Invalid image magic bytes")]
    InvalidMagicBytes,
    #[error("Source format is not one of the decoders this service enables")]
    UnsupportedSourceFormat,
    #[error("Image too large: decompressed size exceeds the configured limit")]
    DecompressionBomb,
    // No figure in the message: the ceiling is `Config::max_source_bytes`, which is configurable
    // (the tests set it as low as 64 KB). Naming a default here misleads in the worst direction -
    // an operator reading a "20MB" refusal has no reason to go looking at their own configuration.
    #[error("Source too large: content-length exceeds the configured limit")]
    SourceTooLarge,
    #[error("Requested output exceeds the configured pixel budget")]
    OutputTooLarge,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn valid_https_url_is_accepted() {
        assert!(validate_src("https://example.com/image.jpg").is_ok());
    }

    #[test]
    fn valid_http_url_is_accepted() {
        assert!(validate_src("http://cdn.example.com/img.png").is_ok());
    }

    #[test]
    fn file_scheme_is_rejected() {
        assert!(matches!(
            validate_src("file:///etc/passwd"),
            Err(SecurityError::DisallowedScheme(_))
        ));
    }

    #[test]
    fn ftp_scheme_is_rejected() {
        assert!(matches!(
            validate_src("ftp://example.com/img.jpg"),
            Err(SecurityError::DisallowedScheme(_))
        ));
    }

    #[test]
    fn data_uri_is_rejected() {
        assert!(matches!(
            validate_src("data:image/png;base64,abc"),
            Err(SecurityError::DisallowedScheme(_))
        ));
    }

    #[test]
    fn invalid_url_is_rejected() {
        assert!(matches!(
            validate_src("not a url"),
            Err(SecurityError::InvalidUrl)
        ));
    }

    #[test]
    fn url_with_userinfo_is_rejected() {
        assert!(matches!(
            validate_src("https://user:pass@example.com/img.jpg"),
            Err(SecurityError::InvalidUrl)
        ));
    }

    #[test]
    fn url_with_username_only_is_rejected() {
        assert!(matches!(
            validate_src("https://user@example.com/img.jpg"),
            Err(SecurityError::InvalidUrl)
        ));
    }

    #[test]
    fn extremely_long_url_is_rejected() {
        let long = format!("https://example.com/{}", "a".repeat(5000));
        assert!(matches!(
            validate_src(&long),
            Err(SecurityError::InvalidUrl)
        ));
    }

    #[test]
    fn url_with_control_char_is_rejected() {
        assert!(matches!(
            validate_src("https://example.com/\x00img.jpg"),
            Err(SecurityError::InvalidUrl)
        ));
    }

    #[test]
    fn loopback_ipv4_is_blocked() {
        assert!(!is_safe_ip(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
    }

    #[test]
    fn private_10_block_is_blocked() {
        assert!(!is_safe_ip(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
    }

    #[test]
    fn private_172_block_is_blocked() {
        assert!(!is_safe_ip(IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1))));
    }

    #[test]
    fn private_192_168_is_blocked() {
        assert!(!is_safe_ip(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))));
    }

    #[test]
    fn link_local_ipv4_is_blocked() {
        assert!(!is_safe_ip(IpAddr::V4(Ipv4Addr::new(169, 254, 0, 1))));
    }

    #[test]
    fn public_ipv4_is_allowed() {
        assert!(is_safe_ip(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))));
        assert!(is_safe_ip(IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))));
    }

    #[test]
    fn loopback_ipv6_is_blocked() {
        assert!(!is_safe_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)));
    }

    #[test]
    fn ipv6_unique_local_is_blocked() {
        // fc00::1
        let ip: Ipv6Addr = "fc00::1".parse().unwrap();
        assert!(!is_safe_ip(IpAddr::V6(ip)));
        // fd00::1
        let ip: Ipv6Addr = "fd00::1".parse().unwrap();
        assert!(!is_safe_ip(IpAddr::V6(ip)));
    }

    #[test]
    fn ipv6_link_local_is_blocked() {
        let ip: Ipv6Addr = "fe80::1".parse().unwrap();
        assert!(!is_safe_ip(IpAddr::V6(ip)));
    }

    #[test]
    fn public_ipv6_is_allowed() {
        let ip: Ipv6Addr = "2606:4700:4700::1111".parse().unwrap();
        assert!(is_safe_ip(IpAddr::V6(ip)));
    }

    #[test]
    fn nat64_embedded_private_ipv4_is_blocked() {
        // 64:ff9b::7f00:1 = NAT64-encoded 127.0.0.1
        let loopback: Ipv6Addr = "64:ff9b::7f00:1".parse().unwrap();
        assert!(!is_safe_ip(IpAddr::V6(loopback)));
        // 64:ff9b::a9fe:a9fe = 169.254.169.254 (cloud metadata)
        let metadata: Ipv6Addr = "64:ff9b::a9fe:a9fe".parse().unwrap();
        assert!(!is_safe_ip(IpAddr::V6(metadata)));
        // A NAT64-wrapped public IPv4 (1.1.1.1) stays allowed.
        let public: Ipv6Addr = "64:ff9b::101:101".parse().unwrap();
        assert!(is_safe_ip(IpAddr::V6(public)));
    }

    #[test]
    fn jpeg_magic_is_accepted() {
        let bytes = [0xFF, 0xD8, 0xFF, 0xE0, 0x00];
        assert!(validate_image_magic(&bytes));
    }

    #[test]
    fn png_magic_is_accepted() {
        let bytes = b"\x89PNG\r\n\x1a\n";
        assert!(validate_image_magic(bytes));
    }

    #[test]
    fn gif87_magic_is_accepted() {
        let bytes = b"GIF87a";
        assert!(validate_image_magic(bytes));
    }

    #[test]
    fn gif89_magic_is_accepted() {
        let bytes = b"GIF89a";
        assert!(validate_image_magic(bytes));
    }

    #[test]
    fn webp_magic_is_accepted() {
        let mut bytes = [0u8; 12];
        bytes[..4].copy_from_slice(b"RIFF");
        bytes[8..12].copy_from_slice(b"WEBP");
        assert!(validate_image_magic(&bytes));
    }

    #[test]
    fn avif_magic_is_accepted_with_ftyp_marker() {
        let mut bytes = [0u8; 12];
        bytes[4..8].copy_from_slice(b"ftyp");
        bytes[8..12].copy_from_slice(b"avif");
        assert!(validate_image_magic(&bytes));
        // Recognised, but not a decodable source format.
        assert_eq!(detect_source_format(&bytes), None);
    }

    #[test]
    fn iso_bmff_brand_without_ftyp_marker_is_rejected() {
        // The `ftyp` marker at bytes[4..8] is what makes the brand meaningful. Accepting on the
        // brand at bytes[8..12] alone would leave bytes 0..8 entirely attacker-controlled.
        let mut bytes = [0u8; 12];
        bytes[8..12].copy_from_slice(b"avif");
        assert!(!validate_image_magic(&bytes));
    }

    #[test]
    fn detect_source_format_identifies_each_declared_format() {
        assert_eq!(
            detect_source_format(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some(SourceFormat::Jpeg)
        );
        assert_eq!(
            detect_source_format(b"\x89PNG\r\n\x1a\n"),
            Some(SourceFormat::Png)
        );
        assert_eq!(detect_source_format(b"GIF89a"), Some(SourceFormat::Gif));
        let mut webp = [0u8; 12];
        webp[..4].copy_from_slice(b"RIFF");
        webp[8..12].copy_from_slice(b"WEBP");
        assert_eq!(detect_source_format(&webp), Some(SourceFormat::WebP));
    }

    #[test]
    fn reserved_ipv4_240_4_is_blocked() {
        // The whole of 240.0.0.0/4 must be refused, not just the 255.255.255.255 broadcast address
        // that `Ipv4Addr::is_broadcast` matches.
        assert!(!is_safe_ip(IpAddr::V4(Ipv4Addr::new(240, 0, 0, 1))));
        assert!(!is_safe_ip(IpAddr::V4(Ipv4Addr::new(255, 255, 255, 254))));
        assert!(!is_safe_ip(IpAddr::V4(Ipv4Addr::new(255, 255, 255, 255))));
        assert!(!is_safe_ip(IpAddr::V4(Ipv4Addr::new(250, 1, 2, 3))));
    }

    #[test]
    fn six_to_four_anycast_and_test_nets_are_blocked() {
        assert!(!is_safe_ip(IpAddr::V4(Ipv4Addr::new(192, 88, 99, 1))));
        assert!(!is_safe_ip(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1))));
        assert!(!is_safe_ip(IpAddr::V4(Ipv4Addr::new(198, 51, 100, 1))));
        assert!(!is_safe_ip(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1))));
    }

    #[test]
    fn non_global_unicast_ipv6_is_blocked() {
        for s in [
            "2001:db8::1",
            "100::1",
            "2001::1",
            "3ffe::1",
            "ff02::1",
            "::",
        ] {
            let ip: Ipv6Addr = s.parse().unwrap();
            let expected_safe = s == "3ffe::1"; // inside 2000::/3, no special-purpose assignment
            assert_eq!(
                is_safe_ip(IpAddr::V6(ip)),
                expected_safe,
                "{s} classified wrongly"
            );
        }
    }

    #[test]
    fn six_to_four_embedded_private_ipv4_is_blocked() {
        // 2002:7f00:0001:: wraps 127.0.0.1
        let ip: Ipv6Addr = "2002:7f00:1::1".parse().unwrap();
        assert!(!is_safe_ip(IpAddr::V6(ip)));
        // 2002:a9fe:a9fe:: wraps 169.254.169.254 (cloud metadata)
        let ip: Ipv6Addr = "2002:a9fe:a9fe::1".parse().unwrap();
        assert!(!is_safe_ip(IpAddr::V6(ip)));
        // A 6to4-wrapped public IPv4 (1.1.1.1) stays allowed.
        let ip: Ipv6Addr = "2002:101:101::1".parse().unwrap();
        assert!(is_safe_ip(IpAddr::V6(ip)));
    }

    #[test]
    fn random_bytes_are_rejected() {
        let bytes = b"PK\x03\x04"; // ZIP file header
        assert!(!validate_image_magic(bytes));
    }

    #[test]
    fn pdf_header_is_rejected() {
        let bytes = b"%PDF-1.4";
        assert!(!validate_image_magic(bytes));
    }

    #[test]
    fn empty_slice_is_rejected() {
        assert!(!validate_image_magic(&[]));
    }

    #[test]
    fn short_slice_is_rejected() {
        assert!(!validate_image_magic(&[0xFF, 0xD8]));
    }
}
