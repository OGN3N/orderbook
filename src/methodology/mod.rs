pub mod latency;
mod rdtsc;

pub use latency::{estimate_tsc_frequency, get_tsc_frequency, tsc_ticks_to_ns};
pub use rdtsc::{rdtsc, rdtsc_end, rdtsc_start};
