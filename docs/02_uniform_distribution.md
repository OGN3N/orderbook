# Uniform Price-Distribution Workload

This scenario uses the global timing and sampling procedure defined in [Experimental Methodology](01_methodology.md).

## 6.1 Uniform random distribution

### 6.1.1 Definition and purpose

In the uniform scenario, every valid integer price tick has the same probability of being selected. If $P$ denotes the generated price, then

```math
P \sim \mathrm{DiscreteUniform}\{1,2,\ldots,9999\},
\qquad
\Pr(P=p)=\frac{1}{9999}.
```

The expected price is 5,000 ticks. The distribution is intentionally spread across the complete supported price range rather than concentrated around a mid-price. It should therefore be interpreted as a synthetic low-locality stress workload, not as a realistic model of ordinary order placement. Its purpose is to expose how the four data structures behave when successive events frequently refer to distant price levels.

Figure 6.1 illustrates the theoretical workload. The plotted bins have equal width and therefore equal expected mass. It is a model of the generator rather than a histogram of benchmark observations, because the result CSV stores aggregate latency percentiles and does not store the generated prices.

![Theoretical uniform price-distribution workload](../figures/uniform_workload_model.svg)

*Figure 6.1: The discrete uniform price model used by the benchmark. Each price in the interval from 1 through 9,999 is equally likely.*

### 6.1.2 Scenario procedure

For insertion and cancellation, each independent book receives 10,000 orders whose prices are sampled from the uniform distribution. Sides alternate between bid and ask, and all quantities are fixed at 100 units. Once the book is full, its order identifiers are shuffled and every order is cancelled. One hundred such batches produce 1,000,000 timed insertions and 1,000,000 timed cancellations for each implementation.

For market-order execution, a fresh book is initialized with 200 uniformly distributed asks. The benchmark then measures 100 buy market orders of 100 units before replacing the book. This sequence is repeated 10,000 times, yielding 1,000,000 observations for each implementation. The workload favors neither a particular price region nor the hybrid book's 200-level hot zone, which is initially centred at tick 5,000.

The use of a wide price range is expected to reduce spatial and temporal locality, particularly for the two fixed-grid representations. Nevertheless, the benchmark records latency rather than hardware cache or TLB events. Cache- and TLB-related explanations of the results must therefore be presented as architectural interpretations, not as directly measured causal claims.

### 6.1.3 Results

Table 6.1 reports median and p99 latency. TSC tick counts are the measured values; nanoseconds are derived using the calibrated TSC frequency of 4.192 GHz recorded in `results/scenario_uniform.csv`.

| Operation | Metric | Fixed-tick AoS | Fixed-tick SoA | Hybrid | B-tree AoS |
|---|---:|---:|---:|---:|---:|
| Insertion | p50 | 210 (50.1 ns) | 378 (90.2 ns) | 420 (100.2 ns) | 420 (100.2 ns) |
|  | p99 | 378 (90.2 ns) | 546 (130.2 ns) | 798 (190.4 ns) | 840 (200.4 ns) |
| Cancellation | p50 | 252 (60.1 ns) | 294 (70.1 ns) | 588 (140.3 ns) | 588 (140.3 ns) |
|  | p99 | 420 (100.2 ns) | 378 (90.2 ns) | 1,008 (240.5 ns) | 1,092 (260.5 ns) |
| Market order | p50 | 8,778 (2,094.0 ns) | 6,510 (1,553.0 ns) | 546 (130.2 ns) | 546 (130.2 ns) |
|  | p99 | 18,858 (4,498.6 ns) | 13,776 (3,286.3 ns) | 882 (210.4 ns) | 924 (220.4 ns) |

*Table 6.1: Uniform-workload latency in TSC ticks, with derived nanoseconds in parentheses; 1,000,000 observations per implementation and operation.*

![Uniform-workload p50 and p99 latency](../figures/uniform_latency_p50_p99.svg)

*Figure 6.2: Median and p99 operation latency under uniformly distributed prices. Each operation uses a separate vertical scale.*

The fixed-tick AoS implementation has the lowest insertion latency: its median of 210 ticks is 44.4% lower than the SoA median of 378 ticks and 50.0% lower than the hybrid and tree medians of 420 ticks. This result is consistent with the fixed-grid insertion path, which maps a price directly to an array index and appends one complete `Order` value. The SoA variant performs several vector appends for the separate fields, while the hybrid and tree designs usually perform a tree operation for uniformly distributed prices.

The fixed-tick AoS implementation records the lowest median cancellation latency, while SoA records the lowest p99 cancellation latency. With 10,000 orders distributed over 9,999 ticks and split evenly between the two sides, most occupied side/price levels contain only one order. The scenario therefore does not strongly exercise the proposed SoA advantage of scanning a densely packed identifier array at a deep price level. Instead, the local search within a level is usually short.

Market-order execution reverses the ranking. The hybrid and tree implementations both record a median of 546 ticks, compared with 8,778 ticks for fixed-tick AoS and 6,510 ticks for SoA. The fixed-tick AoS median is consequently 16.08 times the hybrid median, and its p99 is 21.38 times the hybrid p99. The main structural explanation is best-price discovery. Under a sparse uniform book, the fixed-grid variants scan many empty price levels to locate the lowest ask, whereas the tree can access its lowest key and the hybrid can compare the best hot-zone and cold-zone candidates. This result demonstrates that direct indexing is beneficial for insertion but does not guarantee fast ordered traversal in a sparse price space.

The complete percentile profiles in Figure 6.3 show that the same ranking persists into the measured tail. Maximum values are intentionally omitted from this plot because isolated interruptions and scheduling events can dominate a single maximum; p99.9 and p99.99 provide more stable descriptions of tail behavior.

![Uniform-workload latency percentile profiles](../figures/uniform_latency_percentiles.svg)

*Figure 6.3: Latency percentile profiles for the uniform scenario. The vertical axis is logarithmic and each operation is displayed separately.*

Overall, no implementation dominates every operation. Fixed-tick AoS is best for uniform insertion and cancellation, while the hybrid and tree designs are substantially better for market orders in a sparse, widely distributed book. This is the central result of the uniform scenario: the performance effect of a data layout depends on the access pattern induced by the operation, and contiguous storage alone does not remove the cost of discovering the next occupied level.

## Reproducing the uniform results and figures

Regenerate the CSV and then the three SVG figures from the repository root:

```bash
cargo run --release -- scenario_uniform
python3 scripts/generate_thesis_plots.py --scenario uniform
```

The CSV contains aggregate percentiles rather than raw latency observations, so it cannot produce a latency histogram, violin plot, or empirical cumulative-distribution function.
