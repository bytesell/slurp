#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub cache_ttl: u64,
    pub memory_cache_size: u64,
    /// SECURITY: reject source images exceeding this to limit fetch cost.
    pub max_source_bytes: usize,
    /// SECURITY: reject decoded buffers exceeding this to prevent decompression bomb attacks.
    pub max_decoded_bytes: usize,
    pub max_width: u32,
    /// SECURITY: cap the OUTPUT pixel count, not just the width. A width cap alone bounds nothing
    /// on a very tall source, and encode cost (Lanczos3 resample plus AVIF encode) scales with
    /// pixels, so this is the limit that actually bounds per-request CPU.
    pub max_output_pixels: u64,
    pub fetch_timeout_secs: u64,
    /// SECURITY: whole-request deadline enforced by a tower timeout layer, so a slow upstream plus
    /// a slow encode cannot hold a connection open indefinitely.
    pub request_timeout_secs: u64,
    /// SECURITY: how many images may be decoded/resized/encoded at once. CPU work runs on the
    /// blocking pool; without a cap, N concurrent large requests saturate the machine.
    pub max_concurrent_transforms: usize,
    pub rate_limit_requests: u64,
    pub rate_limit_window_secs: u64,
    /// SECURITY: hard ceiling on tracked rate-limit buckets. An attacker with a routine IPv6 /64
    /// supplies unlimited distinct source addresses; without a ceiling the map grows unbounded.
    pub rate_limit_max_entries: usize,
    /// SECURITY: cap the total size of the on-disk cache. A background sweeper deletes expired
    /// entries and evicts the oldest until under this budget, so an attacker requesting many distinct
    /// (src, width, format) variants cannot fill the disk.
    pub max_disk_bytes: u64,
    /// How often the disk-cache sweeper runs.
    pub disk_sweep_interval_secs: u64,
    /// TEST-ONLY seam. Compiled out entirely outside `cfg(test)`, so it cannot exist in a release
    /// build. It lets the crate's own unit tests point the fetch path at a loopback test origin and
    /// exercise the guards that sit BEHIND the SSRF check (streaming cap, timeout, redirect refusal,
    /// magic rejection, decompression bomb), which are otherwise untestable without weakening the
    /// SSRF protection itself.
    #[cfg(test)]
    pub allow_private_hosts: bool,
}

impl Config {
    #[must_use]
    pub fn new(port: u16, cache_ttl: u64) -> Self {
        Self {
            port,
            cache_ttl,
            memory_cache_size: 512,
            max_source_bytes: 20 * 1024 * 1024,
            max_decoded_bytes: 50 * 1024 * 1024,
            max_width: 8192,
            max_output_pixels: 8_000_000,
            fetch_timeout_secs: 10,
            request_timeout_secs: 30,
            max_concurrent_transforms: default_concurrency(),
            rate_limit_requests: 100,
            rate_limit_window_secs: 60,
            rate_limit_max_entries: 65_536,
            max_disk_bytes: 2 * 1024 * 1024 * 1024,
            disk_sweep_interval_secs: 300,
            #[cfg(test)]
            allow_private_hosts: false,
        }
    }
}

/// One in-flight transform per core. Image decode/resize/encode is CPU-bound, so more permits than
/// cores only trades throughput for latency and memory (each in-flight transform peaks in the
/// hundreds of megabytes).
#[must_use]
pub fn default_concurrency() -> usize {
    std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(2)
}
