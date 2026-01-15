/// Read the CPU Time-Stamp Counter
///
/// Returns the number of CPU cycles since processor reset.
/// This is the fastest way to measure time on x86/x64.
#[inline(always)]
pub fn rdtsc() -> u64 {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::x86_64::_rdtsc()
    }

    #[cfg(not(target_arch = "x86_64"))]
    {
        // Fallback for non-x86 platforms (uses std::time)
        use std::time::Instant;
        static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        let start = START.get_or_init(|| Instant::now());
        start.elapsed().as_nanos() as u64
    }
}
