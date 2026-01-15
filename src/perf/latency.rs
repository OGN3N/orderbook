use super::rdtsc;

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
    pub fn precentiles(&mut self) -> Option<Percentiles>
    {
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
            p9999: self.percentile_at(0.9999)
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
