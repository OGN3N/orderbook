pub mod latency;
mod rdtsc;

pub use latency::{cycles_to_ns, estimate_cpu_frequency, get_cpu_frequency};
pub use rdtsc::rdtsc;
