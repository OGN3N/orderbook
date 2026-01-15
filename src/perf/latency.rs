use super::rdtsc;

/// Get CPU frequency from /proc/cpuinfo (Linux only)
/// Returns frequency in GHz, or None if not available
#[cfg(target_os = "linux")]
pub fn get_cpu_frequency_from_proc() -> Option<f64> {
    use std::fs;

    let cpuinfo = fs::read_to_string("/proc/cpuinfo").ok()?;

    for line in cpuinfo.lines() {
        if line.starts_with("cpu MHz") {
            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() == 2 {
                let mhz = parts[1].trim().parse::<f64>().ok()?;
                return Some(mhz / 1000.0); // Convert MHz to GHz
            }
        }
    }
    None
}

/// Get CPU frequency - tries /proc/cpuinfo first, falls back to estimation
pub fn get_cpu_frequency() -> f64 {
    #[cfg(target_os = "linux")]
    {
        if let Some(freq) = get_cpu_frequency_from_proc() {
            return freq;
        }
    }

    // Fallback: estimate by measurement
    estimate_cpu_frequency()
}

/// Estimate CPU frequency in GHz by measuring cycles over a known time period
pub fn estimate_cpu_frequency() -> f64 {
    use std::time::Instant;

    let start_time = Instant::now();
    let start_cycles = rdtsc();

    // Sleep for 10ms to get a good measurement
    std::thread::sleep(std::time::Duration::from_millis(10));

    let end_cycles = rdtsc();
    let end_time = Instant::now();

    let elapsed_ns = end_time.duration_since(start_time).as_nanos() as f64;
    let elapsed_cycles = (end_cycles - start_cycles) as f64;

    // GHz = (cycles / nanoseconds)
    elapsed_cycles / elapsed_ns
}

/// Convert CPU cycles to nanoseconds given a CPU frequency in GHz
pub fn cycles_to_ns(cycles: u64, cpu_ghz: f64) -> f64 {
    cycles as f64 / cpu_ghz
}

pub struct LatencyTracker {
    samples: Vec<u64>,
}

impl LatencyTracker {
    pub fn new(capacity: usize) -> Self {
        Self {
            samples: Vec::with_capacity(capacity),
        }
    }

    #[inline(always)]
    pub fn record<F, R>(&mut self, op: F) -> R
    where
        F: FnOnce() -> R,
    {
        let start = rdtsc();
        let result = op();
        let end = rdtsc();

        self.samples.push(end - start);

        result
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn clear(&mut self) {
        self.samples.clear();
    }
}

#[derive(Debug, Clone)]
pub struct Percentiles {
    pub min: u64,
    pub max: u64,
    pub mean: f64,
    pub p50: u64, // Median
    /// 95 % of operations are faster
    pub p95: u64,
    /// tail latencies
    pub p99: u64,
    pub p999: u64,  // p99.9
    pub p9999: u64, // p99.99
}

impl LatencyTracker {
    pub fn precentiles(&mut self) -> Option<Percentiles> {
        if self.samples.is_empty() {
            return None;
        }

        self.samples.sort_unstable();

        let len = self.samples.len();
        let min = self.samples[0];
        let max = self.samples[len - 1];
        let sum: u64 = self.samples.iter().sum();
        let mean = sum as f64 / len as f64;

        Some(Percentiles {
            min,
            max,
            mean,
            p50: self.percentile_at(0.50),
            p95: self.percentile_at(0.95),
            p99: self.percentile_at(0.99),
            p999: self.percentile_at(0.999),
            p9999: self.percentile_at(0.9999),
        })
    }

    fn percentile_at(&self, p: f64) -> u64 {
        assert!(
            !self.samples.is_empty(),
            "No samples to calculate percentile"
        );
        assert!(
            p >= 0.0 && p <= 1.0,
            "Percentile must be between 0.0 and 1.0"
        );

        let index = (p * (self.samples.len() - 1) as f64) as usize;
        self.samples[index]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_latency_tracker_basic() {
        let mut tracker = LatencyTracker::new(100);

        // Record a simple operation
        let result = tracker.record(|| {
            let mut sum = 0;
            for i in 0..10 {
                sum += i;
            }
            sum
        });

        // Check that operation returned correct value
        assert_eq!(result, 45); // 0+1+2+...+9 = 45

        // Check that we recorded one sample
        assert_eq!(tracker.len(), 1);
        assert!(!tracker.is_empty());
    }

    #[test]
    fn test_percentiles_calculation() {
        let mut tracker = LatencyTracker::new(1000);

        // Record 1000 operations with predictable latencies
        // We'll simulate latencies from 100 to 1099 cycles
        for i in 100..1100 {
            tracker.record(|| {
                // Simulate work by reading rdtsc multiple times
                // This creates artificial delay
                let start = rdtsc();
                let mut dummy = start;
                for _ in 0..i {
                    dummy = dummy.wrapping_add(1);
                }
                // Use dummy to prevent optimization
                std::hint::black_box(dummy);
            });
        }

        // Calculate percentiles
        let stats = tracker.precentiles().expect("Should have percentiles");

        // Basic sanity checks
        println!("Min: {}", stats.min);
        println!("p50 (median): {}", stats.p50);
        println!("p95: {}", stats.p95);
        println!("p99: {}", stats.p99);
        println!("p999: {}", stats.p999);
        println!("p9999: {}", stats.p9999);
        println!("Max: {}", stats.max);
        println!("Mean: {:.2}", stats.mean);

        // Verify ordering: min <= p50 <= p95 <= p99 <= p999 <= p9999 <= max
        assert!(stats.min <= stats.p50);
        assert!(stats.p50 <= stats.p95);
        assert!(stats.p95 <= stats.p99);
        assert!(stats.p99 <= stats.p999);
        assert!(stats.p999 <= stats.p9999);
        assert!(stats.p9999 <= stats.max);

        // Verify we recorded all samples
        assert_eq!(tracker.len(), 1000);
    }

    #[test]
    fn test_empty_tracker() {
        let mut tracker = LatencyTracker::new(10);

        // Empty tracker should return None
        assert!(tracker.precentiles().is_none());
        assert!(tracker.is_empty());
        assert_eq!(tracker.len(), 0);
    }

    #[test]
    fn test_clear() {
        let mut tracker = LatencyTracker::new(10);

        // Record some operations
        for _ in 0..5 {
            tracker.record(|| 42);
        }

        assert_eq!(tracker.len(), 5);

        // Clear and verify
        tracker.clear();
        assert!(tracker.is_empty());
        assert_eq!(tracker.len(), 0);
    }
}
