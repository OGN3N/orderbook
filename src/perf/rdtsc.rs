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

/// Read the timestamp counter at the start of a measured region.
///
/// `LFENCE` prevents earlier instructions from overlapping the measurement.
#[inline(always)]
pub fn rdtsc_start() -> u64 {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::x86_64::_mm_lfence();
        let timestamp = core::arch::x86_64::_rdtsc();
        core::arch::x86_64::_mm_lfence();
        timestamp
    }

    #[cfg(not(target_arch = "x86_64"))]
    {
        rdtsc()
    }
}

/// Read the timestamp counter at the end of a measured region.
///
/// `RDTSCP` waits for earlier instructions to finish, and the trailing
/// `LFENCE` prevents later instructions from entering the measured region.
#[inline(always)]
pub fn rdtsc_end() -> u64 {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let mut aux = 0u32;
        let timestamp = core::arch::x86_64::__rdtscp(&mut aux);
        core::arch::x86_64::_mm_lfence();
        timestamp
    }

    #[cfg(not(target_arch = "x86_64"))]
    {
        rdtsc()
    }
}
