# Memory-Aware Limit Order Books in Rust

A limit order book (LOB) is the core data structure used by electronic exchanges to store outstanding bids and asks. Limit orders add liquidity, cancellations remove resting liquidity, and market orders consume orders from the best available prices. Within each price level, this project uses first-in, first-out priority, giving the usual price-time matching rule.

At matching-engine timescales, performance depends on more than asymptotic complexity. Cache locality, memory layout, pointer chasing, sparse price ranges, and address-translation overhead can dominate operations that otherwise perform little computation. This repository accompanies a master's thesis investigating those effects through interchangeable order-book implementations written in Rust.

The central question is: **how do different data layouts affect insertion, cancellation, and market-order latency under changing price locality and book state?** Rust provides explicit control over representation and allocation while retaining compile-time memory safety, making it suitable for comparing low-level designs without implementing each one in a different programming model.

## Implementations

Every implementation satisfies the same `OrderbookTrait` and supports limit-order insertion, cancellation by identifier, market-order execution, best bid/ask queries, and depth queries.

| Implementation | Representation | Main trade-off |
|---|---|---|
| Fixed-tick AoS | A fixed 10,000-level array per side; each level stores complete `Order` values | Direct price indexing, but best-price discovery may scan many empty levels |
| Fixed-tick SoA | The same fixed price grid with separate ID, side, price, and quantity vectors | Compact field-specific scans, with multiple vectors to update |
| B-tree AoS | Sparse ordered `BTreeMap` levels containing complete orders | Efficient ordered traversal without a full price grid, with tree-lookup overhead |
| Hybrid | A 200-level array around tick 5,000 plus B-trees for cold prices | Fast access near the reference price while retaining sparse out-of-range storage |

All variants maintain an order-ID index for cancellation and are checked against one another with deterministic and property-based correctness tests.

## What is implemented

- A common price-time-priority order-book interface and four working representations.
- Cross-implementation correctness tests for book state, cancellation, and normalized fills.
- A baseline comparison of insertion, cancellation, and market-order latency.
- Four price-distribution workloads: [uniform](docs/02_uniform_distribution.md), [clustered](docs/03_clustered_distribution.md), [Zipfian](docs/04_zipfian_distribution.md), and [bursty](docs/05_bursty_distribution.md).
- Operational workloads covering a [10:1 cancellation ratio](docs/06_high_cancel.md), [multi-level market sweeps](docs/07_sweep.md), [order-book build-up](docs/08_buildup.md), and stable-depth mixed traffic.
- Isolated experiments for [order-record alignment and padding](docs/09_alignment.md), huge pages, [software prefetching](docs/10_prefetch.md), and market-order matching strategy.
- CSV export containing minimum, mean, maximum, p50, p95, p99, p99.9, and p99.99 latency.
- Reproducible SVG figures for the documented workloads and optimization experiments.
- A written [experimental methodology](docs/01_methodology.md), including timing boundaries, TSC calibration, statistical aggregation, and limitations.
- A concluding [comparative evaluation, discussion of future directions, and conclusion](docs/11_comparative_evaluation_and_conclusion.md).

The current experiments measure **latency**, not throughput or hardware performance counters. On x86-64, operations are bounded with `LFENCE`/`RDTSC` at the start and `RDTSCP`/`LFENCE` at the end. The time-stamp counter is calibrated against a monotonic clock for each run. CSV files retain the directly measured TSC ticks and their derived nanosecond values.

The completed distribution experiments show that no layout dominates every operation. Direct indexing is frequently strong for insertion and cancellation, while the tree and hybrid designs avoid expensive empty-level scans during market-order execution in sparse or centrally concentrated books. These results are hardware- and workload-dependent; the committed CSV files should be treated as experimental observations rather than universal performance guarantees.

## Repository layout

```text
src/orderbook/          four order-book implementations and common trait
src/perf/               TSC timing, calibration, and percentile collection
examples/baseline/      baseline latency benchmark
examples/distribution/  uniform, clustered, Zipfian, and bursty workloads
examples/operational/   cancellation, sweep, build-up, and steady-state tests
examples/optimizations/ isolated memory-layout and access-path experiments
tests/                  deterministic and property-based correctness tests
results/                generated benchmark CSV files
figures/                generated thesis SVG figures
docs/                   methodology and scenario chapters
scripts/                dependency-free SVG generation from result CSVs
```

## Running the project

The project uses Rust 2024 edition. A recent stable Rust toolchain and Cargo are required. Linux on x86-64 is recommended for results comparable to the included TSC measurements. Python 3 is needed only to regenerate the SVG figures.

Run the correctness suite first:

```bash
cargo test
```

List all available benchmarks:

```bash
cargo run --release -- --list
```

Run the baseline comparison:

```bash
cargo run --release -- bench_latency
```

Run all four distribution scenarios:

```bash
cargo run --release -- \
  scenario_uniform \
  scenario_clustered \
  scenario_zipfian \
  scenario_bursty
```

Run selected operational or optimization experiments in the same way:

```bash
cargo run --release -- scenario_high_cancel scenario_sweep scenario_buildup
cargo run --release -- bench_alignment bench_prefetch
```

Run the complete benchmark collection by providing no benchmark names:

```bash
cargo run --release
```

Benchmarks use optimized release binaries and many collect one million observations per measurement point, so the complete collection can take substantial time. Scenario runs overwrite their corresponding files in `results/`.

Regenerate a documented experiment's three SVG figures from its CSV:

```bash
python3 scripts/generate_thesis_plots.py --scenario uniform
python3 scripts/generate_thesis_plots.py --scenario clustered
python3 scripts/generate_thesis_plots.py --scenario zipfian
python3 scripts/generate_thesis_plots.py --scenario bursty
python3 scripts/generate_thesis_plots.py --scenario buildup
python3 scripts/generate_thesis_plots.py --scenario alignment
python3 scripts/generate_thesis_plots.py --scenario prefetch
```

The CSVs contain aggregate percentiles rather than raw samples. The generated latency graphics are therefore percentile comparisons, while the other graphics visualize workload models or memory layouts rather than raw observations.
