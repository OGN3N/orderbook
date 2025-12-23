# Trading Orderbook Performance Testing - Project Structure

```
trading/
├── Cargo.toml
├── Cargo.lock
├── README.md
├── PROJECT_STRUCTURE.md (this file)
│
├── src/
│   ├── main.rs                          # CLI entry point for running benchmarks
│   ├── lib.rs                           # Library root, re-exports public API
│   │
│   ├── types/                           # Core domain types
│   │   ├── mod.rs
│   │   ├── order.rs                     # Order, OrderId, OrderSide
│   │   ├── price.rs                     # Price type with tick size
│   │   ├── quantity.rs                  # Quantity type with lot size
│   │   ├── timestamp.rs                 # Timestamp handling
│   │   └── resolution.rs                # Resolution parameters (tick/lot size)
│   │
│   ├── orderbook/                       # Orderbook implementations
│   │   ├── mod.rs                       # Trait definition: Orderbook trait
│   │   ├── traits.rs                    # Common traits and interfaces
│   │   │
│   │   ├── aos/                         # Array-of-Structs implementation
│   │   │   ├── mod.rs
│   │   │   └── orderbook.rs
│   │   │
│   │   ├── soa/                         # Structure-of-Arrays implementation
│   │   │   ├── mod.rs
│   │   │   ├── orderbook.rs
│   │   │   └── order_storage.rs         # Separate arrays for each field
│   │   │
│   │   ├── fixed_tick/                  # Fixed-tick array implementation
│   │   │   ├── mod.rs
│   │   │   ├── orderbook.rs
│   │   │   └── price_level.rs
│   │   │
│   │   ├── tree_based/                  # BTreeMap baseline implementation
│   │   │   ├── mod.rs
│   │   │   └── orderbook.rs
│   │   │
│   │   └── hybrid/                      # Hybrid hot/cold implementation
│   │       ├── mod.rs
│   │       ├── orderbook.rs
│   │       └── tier_manager.rs          # Manages hot/cold tiers
│   │
│   ├── matching/                        # Matching engine logic
│   │   ├── mod.rs
│   │   ├── engine.rs                    # Core matching engine
│   │   ├── priority.rs                  # Price-time priority logic
│   │   └── execution.rs                 # Trade execution and fills
│   │
│   ├── workload/                        # Workload generators
│   │   ├── mod.rs
│   │   ├── generator.rs                 # Trait for workload generation
│   │   ├── uniform.rs                   # Uniform random distribution
│   │   ├── clustered.rs                 # Clustered around mid-price
│   │   ├── zipfian.rs                   # Zipfian distribution
│   │   ├── bursty.rs                    # Bursty traffic patterns
│   │   ├── realistic.rs                 # Real-world order flow simulator
│   │   └── replay.rs                    # Replay from recorded data
│   │
│   ├── benchmark/                       # Benchmarking infrastructure
│   │   ├── mod.rs
│   │   ├── harness.rs                   # Custom benchmark harness
│   │   ├── metrics.rs                   # Metric collection and aggregation
│   │   ├── config.rs                    # Benchmark configuration
│   │   └── runner.rs                    # Orchestrates benchmark execution
│   │
│   ├── perf/                            # Performance measurement
│   │   ├── mod.rs
│   │   ├── counters.rs                  # Hardware performance counters (perf_event)
│   │   ├── latency.rs                   # Latency measurement (RDTSC, timing)
│   │   ├── cache.rs                     # Cache miss tracking
│   │   ├── tlb.rs                       # TLB miss tracking
│   │   └── memory.rs                    # Memory usage and alignment utilities
│   │
│   ├── optimization/                    # Memory layout optimizations
│   │   ├── mod.rs
│   │   ├── alignment.rs                 # Cache-line alignment helpers
│   │   ├── hugepages.rs                 # Huge page allocation
│   │   ├── prefetch.rs                  # Prefetching hints
│   │   └── padding.rs                   # False sharing prevention
│   │
│   └── analysis/                        # Data analysis and output
│       ├── mod.rs
│       ├── statistics.rs                # Statistical analysis (percentiles, etc.)
│       ├── export.rs                    # CSV/JSON export
│       ├── comparison.rs                # Compare implementations
│       └── visualization.rs             # Generate plot data
│
├── benches/                             # Criterion benchmarks
│   ├── orderbook_ops.rs                 # Benchmark individual operations
│   ├── scenarios.rs                     # Full scenario benchmarks
│   ├── cache_behavior.rs                # Cache-specific benchmarks
│   └── throughput.rs                    # Throughput benchmarks
│
├── tests/                               # Integration and correctness tests
│   ├── correctness.rs                   # Verify matching logic correctness
│   ├── property_tests.rs                # Property-based tests (proptest)
│   └── scenarios.rs                     # Test various order flow scenarios
│
├── examples/                            # Example usage
│   ├── simple_orderbook.rs              # Basic orderbook usage
│   ├── run_benchmark.rs                 # Run a single benchmark
│   └── compare_implementations.rs       # Compare different implementations
│
├── data/                                # Test data and results
│   ├── workloads/                       # Pre-generated workload files
│   ├── results/                         # Benchmark results (CSV/JSON)
│   └── plots/                           # Generated visualization data
│
├── scripts/                             # Utility scripts
│   ├── setup_hugepages.sh               # Configure huge pages on Linux
│   ├── run_all_benchmarks.sh            # Run complete benchmark suite
│   ├── analyze_results.py               # Python analysis scripts
│   └── generate_plots.py                # Generate thesis plots
│
└── docs/                                # Additional documentation
    ├── BENCHMARKING.md                  # How to run benchmarks
    ├── IMPLEMENTATIONS.md               # Details on each orderbook variant
    ├── PERFORMANCE_COUNTERS.md          # Guide to perf counters
    └── RESULTS.md                       # Interpreting results
```

## Module Responsibilities

### `types/`
Core domain types used throughout the system. These are the fundamental building blocks.

- **order.rs**: Order struct, OrderId (u64), OrderSide enum (Bid/Ask)
- **price.rs**: Price wrapper with tick size validation
- **quantity.rs**: Quantity wrapper with lot size validation
- **timestamp.rs**: Nanosecond-precision timestamps
- **resolution.rs**: Tick and lot size parameters

### `orderbook/`
Different orderbook implementations, all implementing a common `Orderbook` trait.

**Common trait**:
```rust
pub trait Orderbook {
    fn add_limit_order(&mut self, order: Order) -> Result<(), OrderbookError>;
    fn execute_market_order(&mut self, side: OrderSide, quantity: Quantity) -> Vec<Fill>;
    fn cancel_order(&mut self, order_id: OrderId) -> Result<Order, OrderbookError>;
    fn best_bid(&self) -> Option<Price>;
    fn best_ask(&self) -> Option<Price>;
    fn depth_at_price(&self, price: Price, side: OrderSide) -> Quantity;
    // ... more methods
}
```

Each implementation variant lives in its own subdirectory.

### `matching/`
Matching engine that works with any Orderbook implementation.

- **engine.rs**: Orchestrates order processing
- **priority.rs**: Implements price-time priority (FIFO queues)
- **execution.rs**: Generates Fill events when orders match

### `workload/`
Generates different order flow patterns for testing.

All generators implement a common trait:
```rust
pub trait WorkloadGenerator {
    fn next_event(&mut self) -> OrderEvent;
    fn reset(&mut self);
}
```

### `benchmark/`
Custom benchmarking harness that integrates with performance counters.

- **harness.rs**: Main benchmarking loop with warmup/measurement phases
- **metrics.rs**: Collects latency, throughput, cache misses, etc.
- **config.rs**: TOML/JSON configuration for benchmark parameters
- **runner.rs**: Runs multiple benchmarks across implementations and scenarios

### `perf/`
Low-level performance measurement using hardware counters and cycle-accurate timing.

- **counters.rs**: Wraps Linux perf_event API
- **latency.rs**: RDTSC-based latency measurement
- **cache.rs**: L1/L2/L3 miss tracking
- **tlb.rs**: TLB miss tracking
- **memory.rs**: Memory allocation, alignment checking, huge page support

### `optimization/`
Memory layout optimization utilities.

- **alignment.rs**: Macros and helpers for cache-line alignment
- **hugepages.rs**: Allocate memory using 2MB/1GB pages
- **prefetch.rs**: Software prefetch intrinsics
- **padding.rs**: Prevent false sharing with padding

### `analysis/`
Post-processing of benchmark results.

- **statistics.rs**: Calculate p50, p95, p99, p99.9 percentiles
- **export.rs**: Export to CSV/JSON for external analysis
- **comparison.rs**: Side-by-side comparison tables
- **visualization.rs**: Prepare data for plotting (can integrate with plotters crate)

## Key Files

### `src/lib.rs`
```rust
pub mod types;
pub mod orderbook;
pub mod matching;
pub mod workload;
pub mod benchmark;
pub mod perf;
pub mod optimization;
pub mod analysis;

// Re-export commonly used types
pub use types::{Order, OrderId, OrderSide, Price, Quantity};
pub use orderbook::Orderbook;
```

### `src/main.rs`
CLI tool to run benchmarks:
```bash
# Run all benchmarks
cargo run --release -- --all

# Run specific implementation with specific workload
cargo run --release -- --implementation soa --workload clustered

# Enable perf counters (requires Linux + permissions)
cargo run --release -- --perf --implementation fixed_tick
```

### `Cargo.toml`
```toml
[package]
name = "trading"
version = "0.1.0"
edition = "2024"

[dependencies]
# Performance measurement
perf-event = "0.4"
libc = "0.2"

# Data structures
arrayvec = "0.7"
smallvec = "1.11"

# Utilities
rand = "0.8"
rand_distr = "0.4"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
csv = "1.3"
anyhow = "1.0"
thiserror = "1.0"

# CLI
clap = { version = "4.4", features = ["derive"] }

# Optional: plotting
plotters = { version = "0.3", optional = true }

[dev-dependencies]
criterion = { version = "0.5", features = ["html_reports"] }
proptest = "1.4"

[profile.release]
opt-level = 3
lto = true
codegen-units = 1
panic = "abort"

[profile.bench]
inherits = "release"

[[bench]]
name = "orderbook_ops"
harness = false

[[bench]]
name = "scenarios"
harness = false
```

## Data Flow

```
Workload Generator
    ↓
Matching Engine
    ↓
Orderbook Implementation
    ↓
Performance Counters (measuring)
    ↓
Metrics Collector
    ↓
Statistical Analysis
    ↓
Export (CSV/JSON) → Visualization → Thesis Plots
```

## Development Workflow

1. **Phase 1**: Implement `types/` and basic `orderbook/aos/`
2. **Phase 2**: Implement `matching/` engine
3. **Phase 3**: Add `workload/uniform.rs` generator
4. **Phase 4**: Add basic `benchmark/` harness (without perf counters first)
5. **Phase 5**: Implement other orderbook variants (SoA, fixed-tick, etc.)
6. **Phase 6**: Add `perf/` counters integration
7. **Phase 7**: Implement all workload generators
8. **Phase 8**: Run full experiments, collect data, analyze

## Testing Strategy

- **Unit tests**: Within each module (`mod.rs` or `*_test.rs`)
- **Integration tests**: In `tests/` directory
- **Property tests**: Use proptest to verify correctness properties
- **Benchmarks**: Criterion benchmarks in `benches/`, custom harness in `benchmark/`

This structure separates concerns cleanly and makes it easy to add new implementations or workloads independently.