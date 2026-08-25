# Experimental Methodology

## 5.1 Experimental objective

The experiments evaluate how the internal representation of a limit order book affects the latency of three operations: inserting a limit order, cancelling a resting order, and executing a market order. Four implementations are compared: a fixed-tick Array-of-Structs (AoS) book, a fixed-tick Structure-of-Arrays (SoA) book, a hybrid hot/cold book, and a tree-based AoS book. Each implementation conforms to the same `OrderbookTrait` interface and is supplied to the benchmark as a generic type. Rust therefore resolves the implementation at compile time, avoiding virtual-dispatch overhead in the measured path.

The independent variables are the order-book implementation and the workload scenario. The dependent variable is operation latency, measured in timestamp-counter ticks and summarized using the minimum, arithmetic mean, maximum, and the p50, p95, p99, p99.9, and p99.99 percentiles. Throughput and hardware performance counters are not measured by the current distribution scenarios and should not be presented as measured outcomes of these experiments.

## 5.2 Controlled benchmark inputs

All implementations receive the same deterministic event sequences. The pseudo-random generator is `StdRng`, initialized from a fixed seed of 42. Subsequent batches use the batch number as a deterministic seed offset. This makes a workload repeatable and ensures that implementation comparisons within one benchmark run use identical prices and cancellation orders.

Unless a scenario specifies otherwise, prices are represented as integer ticks in the valid half-open interval $[1,10000)$, and every order has a quantity of 100 units. Bid and ask orders alternate during the insertion phase, producing equal side counts in every complete batch. Fixing quantity and side balance reduces unrelated variation and isolates the effects of the price distribution and data structure.

## 5.3 Benchmark phases

Each distribution scenario measures the three operations separately.

**Insertion and cancellation.** A new book is created and populated with 10,000 limit orders. Each call to `add_order` is timed individually. The identifiers of the inserted orders are then shuffled, after which the same orders are cancelled in random order and each `cancel_order` call is timed. The book is therefore returned to an empty state. This build-and-empty procedure is repeated until 1,000,000 insertion observations and 1,000,000 cancellation observations have been collected for each implementation.

**Market-order execution.** A separate book is pre-populated with 200 ask orders. This initialization is outside the timed region. The benchmark then measures 100 buy market orders, each for 100 units. Because every resting ask also contains 100 units, each measured market order consumes exactly one complete resting order and partial fills are avoided. A new book is created after every 100 measurements, and the process continues until 1,000,000 market-order observations have been collected per implementation.

This procedure controls book density across repeated batches. It does not, however, hold the book state perfectly constant within a batch: the insertion phase grows the book, the cancellation phase shrinks it, and successive market orders consume the lowest remaining asks. The reported latency distributions therefore characterize the complete scenario rather than one fixed book state.

## 5.4 Latency measurement

The timing method described in this section is global to the experimental study. All reported per-operation latency measurements—including the baseline, price-distribution scenarios, operational scenarios, and isolated optimization experiments—use the same `LatencyTracker` and therefore the same timestamp-reading and aggregation procedure. Later scenario sections define only the workload, controlled parameters, and measured operation; they do not redefine the clock or latency calculation. The wall-clock `Instant` measurements in the command-line runner are used only to report the total duration of a benchmark suite and are not used for the per-operation comparisons.

### 5.4.1 The processor time-stamp counter

Operation latency is measured using the x86-64 time-stamp counter (TSC). The TSC is a 64-bit processor register that increases monotonically, and the `RDTSC` instruction copies its current value into general-purpose registers. Reading this counter is substantially cheaper than making an operating-system timing call, which makes it suitable for measuring operations whose latency is on the order of tens or hundreds of nanoseconds.

The recorded values are described as **TSC ticks** in this thesis and in the result CSV columns (`min_tsc`, `p50_tsc`, and so forth). They are not GPU cycles, retired instruction counts, or necessarily cycles at the processor core's instantaneous turbo frequency. On processors that provide an invariant TSC, the counter advances at a constant reference rate independently of changes between processor power and frequency states. The invariant TSC therefore provides a stable time base, but one TSC tick must not automatically be equated with one current-frequency core cycle [1].

### 5.4.2 Ordering the measurement boundaries

Modern processors execute instructions out of order. A plain `RDTSC` instruction is not serializing, so earlier instructions may still be executing when the start timestamp is read, or instructions belonging to the measured operation may begin before that read has completed. Either effect can move work across the intended measurement boundary and distort a very short latency measurement [2].

The implementation reduces this problem by using separate start and end sequences:

```text
LFENCE
start = RDTSC
LFENCE

measured operation

end = RDTSCP
LFENCE
latency = end - start
```

At the start boundary, the first `LFENCE` prevents earlier instructions and loads from overlapping the measurement. `RDTSC` then reads the TSC, and the second `LFENCE` prevents the measured operation from beginning before the timestamp read has completed. At the end boundary, `RDTSCP` waits until earlier instructions have executed and earlier loads are globally visible before reading the counter. The final `LFENCE` prevents later instructions from being executed ahead of the timestamp read. This sequence follows the ordering guidance in Intel's instruction-set reference [2, 3].

`RDTSCP` does not guarantee that all previous stores have become globally visible. An `MFENCE` would also be required if global visibility of every store were part of the latency definition [3]. The present experiment instead measures completion according to the ordering guarantees of `RDTSCP`, which is appropriate for comparing the local execution path of the four implementations.

### 5.4.3 Measurement boundary in the benchmark harness

The `LatencyTracker::record` method reads the starting timestamp immediately before invoking the supplied operation and reads the ending timestamp immediately after that operation returns. For sample $j$, the recorded latency is therefore

```math
L_j = T_{j,\mathrm{end}} - T_{j,\mathrm{start}}.
```

Appending $L_j$ to the sample vector occurs after the end timestamp and is not included in the measured interval. Scenario setup, such as creating a new order book and inserting the 200 asks used to initialize the market-order workload, is also outside the timed interval. Work performed inside an order-book method—including validation, data-structure traversal, allocation, matching, and creation of fill records—is included. In the present market-order benchmark, the returned fill vector is discarded inside the measured closure, so its destruction is included as well.

The timing instructions and fences have their own non-zero cost. Because the implementation does not measure and subtract an empty timing interval, every observation includes this fixed measurement overhead. Applying the same harness to every implementation makes relative comparisons meaningful, but the shortest absolute latency values are slightly inflated by the timer itself.

### 5.4.4 Conversion from TSC ticks to nanoseconds

The benchmark records TSC ticks directly and calibrates their rate against Rust's monotonic `Instant` clock. For each calibration sample, it reads both clocks, waits for a 50 ms interval, reads them again, and calculates

```math
f_{\mathrm{TSC},k}
=
\frac{T_{k,\mathrm{end}}-T_{k,\mathrm{start}}}
{W_{k,\mathrm{end}}-W_{k,\mathrm{start}}},
```

where the numerator is measured in TSC ticks and the denominator is measured in nanoseconds. The result is expressed in ticks per nanosecond, which is numerically equivalent to gigahertz. Five calibration samples are collected and their median is used as the TSC frequency for the benchmark run:

```math
f_{\mathrm{TSC,GHz}}
=
\mathrm{median}\left(f_{\mathrm{TSC},1},\ldots,f_{\mathrm{TSC},5}\right).
```

Using the median reduces the influence of a scheduling interruption or timing disturbance during one calibration interval. Once the TSC frequency has been established, a measured latency is converted using

```math
L_{\mathrm{ns}} = \frac{L_{\mathrm{TSC}}}{f_{\mathrm{TSC,GHz}}}.
```

For example, if the calibrated rate is 3.000 TSC ticks per nanosecond, a measurement of 210 ticks corresponds to

```math
L_{\mathrm{ns}} = \frac{210}{3.000} = 70.0\ \mathrm{ns}.
```

This approach measures the rate of the same counter used to time the operations. It does not use the instantaneous `cpu MHz` value reported by `/proc/cpuinfo`, so dynamic changes in the processor core's operating frequency do not distort the conversion. The calibrated TSC frequency is stored in the `tsc_ghz` column of every result CSV. TSC ticks remain the directly measured unit, while nanoseconds are derived from the calibrated relationship between the TSC and the monotonic clock.

On non-x86-64 systems, the implementation does not use `RDTSC`; it falls back to Rust's monotonic `Instant` clock and stores elapsed nanoseconds. For this fallback, the conversion factor is one tick per nanosecond. Results from the fallback path should not be mixed directly with x86-64 TSC results without explicitly identifying the different timing method.

### 5.4.5 Statistical aggregation

One latency value is retained for every measured operation. After all observations have been collected, the sample vector is sorted. For percentile $q$, the implementation selects the observation at index

```math
i_q = \left\lfloor q(n-1) \right\rfloor,
```

where $n$ is the number of samples. This procedure is used for the p50, p95, p99, p99.9, and p99.99 values. The minimum, maximum, and arithmetic mean are also calculated. One CSV row is written for each combination of scenario, implementation, and operation, containing the directly measured TSC statistics, the calibrated TSC frequency, and the corresponding derived nanosecond values.

Percentiles are emphasized because matching-engine latency distributions are typically asymmetric and can contain rare but very large observations caused by allocation, cache misses, interrupts, pre-emption, or operating-system activity. The median describes the typical operation, while p99 and the higher percentiles describe tail latency. A maximum is reported for completeness but should not be interpreted as a stable performance guarantee.

### 5.4.6 Measurement limitations

The current harness does not pin the benchmark thread to one logical processor. Although `RDTSCP` also returns the `TSC_AUX` value that can be used to identify processor migration, the code discards this value. Interrupts, task pre-emption, migration, dynamic frequency behavior, and background processes can therefore contribute to the observed tail. These effects should be reduced by controlling the execution environment and quantified by repeating the complete experiment, rather than by deleting large observations after measurement.

## 5.5 Execution protocol and reproducibility

The scenarios are compiled and executed with Rust's optimized release profile. Running a named scenario through the benchmark runner builds the release examples, executes the selected binary, and overwrites its corresponding CSV in the `results/` directory.

All four price-distribution scenarios can be regenerated together with:

```bash
cargo run --release -- \
  scenario_uniform \
  scenario_clustered \
  scenario_zipfian \
  scenario_bursty
```

The complete set of CSV-producing benchmarks is regenerated with:

```bash
cargo run --release -- \
  bench_latency \
  scenario_uniform \
  scenario_clustered \
  scenario_zipfian \
  scenario_bursty \
  scenario_high_cancel \
  scenario_sweep \
  scenario_buildup \
  scenario_steady_state
```

Each benchmark performs a new TSC calibration before collecting operation latencies. The resulting CSV contains the calibration in `tsc_ghz`, direct measurements in the `*_tsc` columns, and converted values in the `*_ns` columns. Old CSV files created by the previous `/proc/cpuinfo` conversion must not be combined with the recalibrated results.

For the final reported experiment, the following environment information should also be recorded: processor model, operating system and kernel, Rust compiler version, power/performance governor, CPU affinity, and whether background services were minimized. The current benchmark does not pin its process to one logical CPU, perform an explicit warm-up phase, subtract timer overhead, or randomize implementation order. These factors should either be controlled in the final run or acknowledged as threats to validity. Repeating the complete benchmark several times would additionally allow run-to-run variability and confidence intervals to be reported; a single one-million-sample run estimates within-run percentiles but not between-run uncertainty.

[1]: https://www.intel.com/content/dam/www/public/us/en/documents/manuals/64-ia-32-architectures-software-developer-vol-3b-part-2-manual.pdf "Intel 64 and IA-32 Architectures Software Developer's Manual, Volume 3B: Time-Stamp Counter"
[2]: https://cdrdv2-public.intel.com/671110/325383-sdm-vol-2abcd.pdf "Intel 64 and IA-32 Architectures Software Developer's Manual, Volume 2: RDTSC"
[3]: https://cdrdv2-public.intel.com/782151/253667-sdm-vol-2b.pdf "Intel 64 and IA-32 Architectures Software Developer's Manual, Volume 2B: RDTSCP"
