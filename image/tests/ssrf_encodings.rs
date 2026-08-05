//! Every alternate encoding of a private or reserved address that an attacker can put in the
//! `src` parameter.
//!
//! This runs against the lib compiled WITHOUT `cfg(test)`, so the `allow_private_hosts` seam does
//! not exist here: it is the production path exactly as deployed. Each case must be refused before
//! any connection is made. Do not relax a case to make a change pass.

use slurp_image::config::Config;
use slurp_image::processor::{OutputFormat, ProcessError, ProcessOptions, process_image};
use slurp_image::security::SecurityError;
use tokio::sync::Semaphore;

#[tokio::test]
async fn all_21_ssrf_encodings_are_blocked() {
    let cases: [(&str, &str); 21] = [
        ("plain loopback", "http://127.0.0.1/i.png"),
        ("decimal 2130706433", "http://2130706433/i.png"),
        ("octal 0177.0.0.1", "http://0177.0.0.1/i.png"),
        ("hex 0x7f000001", "http://0x7f000001/i.png"),
        ("short 127.1", "http://127.1/i.png"),
        ("0.0.0.0", "http://0.0.0.0/i.png"),
        ("localhost", "http://localhost/i.png"),
        ("[::1]", "http://[::1]/i.png"),
        ("[::]", "http://[::]/i.png"),
        ("[::ffff:127.0.0.1]", "http://[::ffff:127.0.0.1]/i.png"),
        ("[fc00::1]", "http://[fc00::1]/i.png"),
        ("[fe80::1]", "http://[fe80::1]/i.png"),
        ("NAT64 [64:ff9b::7f00:1]", "http://[64:ff9b::7f00:1]/i.png"),
        ("10/8", "http://10.0.0.1/i.png"),
        ("192.168/16", "http://192.168.1.1/i.png"),
        ("100.64/10 CGNAT", "http://100.64.0.1/i.png"),
        ("198.18/15 benchmarking", "http://198.18.0.1/i.png"),
        ("192.0.0/24 protocol assignments", "http://192.0.0.1/i.png"),
        ("multicast", "http://224.0.0.1/i.png"),
        ("AWS metadata", "http://169.254.169.254/latest/meta-data/"),
        (
            "GCP/Azure metadata",
            "http://169.254.169.254/metadata/instance",
        ),
    ];

    let config = Config::new(0, 60);
    let sem = std::sync::Arc::new(Semaphore::new(1));
    let mut ssrf = 0;
    let mut other_blocked = 0;

    for (label, url) in cases {
        let opts = ProcessOptions {
            width: 100,
            format: OutputFormat::Jpeg,
        };
        let result = process_image(url, opts, &config, &sem).await;
        match result {
            Err(ProcessError::Security(SecurityError::SsrfBlocked)) => {
                ssrf += 1;
                println!("BLOCKED (SsrfBlocked / 403) : {label:38} {url}");
            }
            Err(ProcessError::DnsError(ref e)) => {
                other_blocked += 1;
                println!("BLOCKED (unresolvable)      : {label:38} {url}  [{e}]");
            }
            Err(ProcessError::Security(SecurityError::InvalidUrl))
            | Err(ProcessError::InvalidUrl) => {
                other_blocked += 1;
                println!("BLOCKED (invalid URL)       : {label:38} {url}");
            }
            other => panic!("REGRESSION: {label} ({url}) was not blocked: {other:?}"),
        }
    }

    println!(
        "\n{ssrf} blocked by the SSRF check, {other_blocked} blocked earlier, {}/21 total",
        ssrf + other_blocked
    );
    assert_eq!(ssrf + other_blocked, 21);
}
