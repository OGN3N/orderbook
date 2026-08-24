//! Scenario 4.2b: Market Order Sweeps
//!
//! Measures one aggressive market order consuming several consecutive price
//! levels. Every reported direction and sweep size receives one million
//! measurements by default.
//!
//! Run with: cargo run --release --example scenario_sweep

use orderbook::analysis::{CsvExporter, ResultRow};
use orderbook::orderbook::SoA::orderbook::Orderbook as SoAOrderbook;
use orderbook::orderbook::fixed_tick::orderbook::Orderbook as FixedTickOrderbook;
use orderbook::orderbook::hybrid::orderbook::Orderbook as HybridOrderbook;
use orderbook::orderbook::tree::orderbook::Orderbook as TreeOrderbook;
use orderbook::orderbook::{Fill, OrderbookTrait};
use orderbook::perf::latency::{LatencyTracker, Percentiles};
use orderbook::perf::{cycles_to_ns, get_cpu_frequency};
use orderbook::types::order::{IdCounter, Order, OrderId, Side};
use orderbook::types::price::Price;
use orderbook::types::quantity::Quantity;

const MID_PRICE: u32 = 5_000;

// Dense book: one order at each consecutive price. Asks begin at 5000 and
// bids at 4999, producing a one-tick spread and symmetric Hybrid zone coverage.
const BOOK_PRICE_LEVELS_PER_SIDE: u32 = 200;
const ORDERS_PER_LEVEL: u32 = 1;
const QTY_PER_ORDER: u32 = 100;

const SMALL_SWEEP_LEVELS: u32 = 5;
const MEDIUM_SWEEP_LEVELS: u32 = 20;
const LARGE_SWEEP_LEVELS: u32 = 50;
const CROSS_ZONE_SWEEP_LEVELS: u32 = 150;

const DEFAULT_SAMPLES_PER_DIRECTION: usize = 1_000_000;
const SAMPLES_PER_BOOK_BATCH: usize = 10_000;
const WARMUP_ROUNDS_PER_BOOK: usize = 10;
const SAMPLE_OVERRIDE_ENV: &str = "ORDERBOOK_SWEEP_SAMPLES_PER_DIRECTION";

const SWEEP_CASES_PER_DIRECTION: usize = 4;
const DIRECTIONS: usize = 2;

// ============================================================================
// Scenario 4.2b: Market Order Sweeps
// ============================================================================
//
// PURPOSE:
// Measure the latency of walking and removing liquidity from several price
// levels in one execute_market_order() call.
//
// DENSE CASES:
// - Small:  5 consecutive levels
// - Medium: 20 consecutive levels
// - Large:  50 consecutive levels
//
// HYBRID BOUNDARY CASE:
// The Hybrid hot zone is [4900, 5100). With asks beginning at 5000 and bids
// beginning at 4999, the first 100 resting levels on either side are hot. A
// 150-level sweep therefore consumes 100 hot levels followed by 50 cold levels.
//
// TIMING MODEL:
// Only execute_market_order() is timed. Result unwrapping, fill validation,
// destruction of the fill Vec, and replenishment are outside the timed region.
// Each consumed level is restored before the next sample, so this is a warm,
// steady-state replenished-book benchmark rather than a cold-cache benchmark.
// Buy-first and sell-first measurement order alternates every round.
//
// CORRECTNESS MODEL:
// Each resting order and market-order slice use equal exact quantities. The
// benchmark tests complete resting-order fills; partial resting-order fills are
// not supported by the current order-book implementations.
//
// This is a synthetic single-order sweep. It does not model stop cascades,
// feedback between traders, network arrival time, or a flash crash.
// ============================================================================

fn main() {
    println!("=== Scenario 4.2b: Market Order Sweeps ===\n");

    let cpu_ghz = get_cpu_frequency();
    println!("CPU frequency: {:.3} GHz", cpu_ghz);

    #[cfg(target_os = "linux")]
    {
        if let Ok(cpuinfo) = std::fs::read_to_string("/proc/cpuinfo") {
            for line in cpuinfo.lines() {
                if line.starts_with("model name") {
                    if let Some(model) = line.split(':').nth(1) {
                        println!("CPU model: {}", model.trim());
                        break;
                    }
                }
            }
        }
    }

    let samples_per_direction = samples_per_direction();
    let book_batches = samples_per_direction.div_ceil(SAMPLES_PER_BOOK_BATCH);

    println!("\nDense Book Setup:");
    println!("  Price levels per side: {}", BOOK_PRICE_LEVELS_PER_SIDE);
    println!("  Orders per level: {}", ORDERS_PER_LEVEL);
    println!("  Quantity per order: {}", QTY_PER_ORDER);
    println!(
        "  Total liquidity per side: {}",
        BOOK_PRICE_LEVELS_PER_SIDE * ORDERS_PER_LEVEL * QTY_PER_ORDER
    );
    println!("  Ask prices: 5000 through 5199");
    println!("  Bid prices: 4999 through 4800");

    println!("\nSweep Cases:");
    println!("  Small:      {} levels", SMALL_SWEEP_LEVELS);
    println!("  Medium:     {} levels", MEDIUM_SWEEP_LEVELS);
    println!("  Large:      {} levels", LARGE_SWEEP_LEVELS);
    println!(
        "  Cross-zone: {} levels (100 Hybrid hot + 50 cold)",
        CROSS_ZONE_SWEEP_LEVELS
    );
    println!("  Directions: market buy and market sell (reported separately)");
    println!(
        "  Measurements per direction and case: {}",
        samples_per_direction
    );
    println!(
        "  Measured sweep calls per implementation: {}",
        samples_per_direction * SWEEP_CASES_PER_DIRECTION * DIRECTIONS
    );
    println!(
        "  Book batches per case: {} (up to {} measured rounds each)",
        book_batches, SAMPLES_PER_BOOK_BATCH
    );
    println!(
        "  Untimed warm-up rounds per book batch: {}",
        WARMUP_ROUNDS_PER_BOOK
    );
    println!("  Timing: execute_market_order only; validation and replenishment are untimed\n");

    println!("--- Fixed-Tick Array ---");
    let fixed = run_sweep_benchmark::<FixedTickOrderbook>(samples_per_direction);
    print_results(&fixed, cpu_ghz);

    println!("\n--- Structure-of-Arrays (SoA) ---");
    let soa = run_sweep_benchmark::<SoAOrderbook>(samples_per_direction);
    print_results(&soa, cpu_ghz);

    println!("\n--- Hybrid (Hot/Cold) ---");
    let hybrid = run_sweep_benchmark::<HybridOrderbook>(samples_per_direction);
    print_results(&hybrid, cpu_ghz);

    println!("\n--- Tree-Based ---");
    let tree = run_sweep_benchmark::<TreeOrderbook>(samples_per_direction);
    print_results(&tree, cpu_ghz);

    println!("\n--- Comparison: p50 latency by sweep and direction ---");
    print_comparison(&fixed, &soa, &hybrid, &tree);

    println!("\n--- Marginal Scaling (additional p50 cycles per added level) ---");
    print_marginal_scaling(&fixed, &soa, &hybrid, &tree);

    export_csv(cpu_ghz, &fixed, &soa, &hybrid, &tree);
}

fn samples_per_direction() -> usize {
    match std::env::var(SAMPLE_OVERRIDE_ENV) {
        Ok(value) => value
            .parse::<usize>()
            .ok()
            .filter(|&samples| samples > 0)
            .unwrap_or_else(|| {
                panic!("{SAMPLE_OVERRIDE_ENV} must be a positive integer, got {value:?}")
            }),
        Err(std::env::VarError::NotPresent) => DEFAULT_SAMPLES_PER_DIRECTION,
        Err(error) => panic!("could not read {SAMPLE_OVERRIDE_ENV}: {error}"),
    }
}

struct SweepResults {
    small: DirectionalResults,
    medium: DirectionalResults,
    large: DirectionalResults,
    cross_zone: DirectionalResults,
}

struct DirectionalResults {
    buy: Percentiles,
    sell: Percentiles,
}

fn run_sweep_benchmark<O: OrderbookTrait>(samples: usize) -> SweepResults {
    SweepResults {
        small: run_directional_case::<O>(SMALL_SWEEP_LEVELS, samples),
        medium: run_directional_case::<O>(MEDIUM_SWEEP_LEVELS, samples),
        large: run_directional_case::<O>(LARGE_SWEEP_LEVELS, samples),
        cross_zone: run_directional_case::<O>(CROSS_ZONE_SWEEP_LEVELS, samples),
    }
}

fn run_directional_case<O: OrderbookTrait>(levels: u32, samples: usize) -> DirectionalResults {
    assert!(levels < BOOK_PRICE_LEVELS_PER_SIDE);

    let mut buy_tracker = LatencyTracker::new(samples);
    let mut sell_tracker = LatencyTracker::new(samples);
    let mut remaining_samples = samples;
    let mut measured_so_far = 0_usize;

    while remaining_samples > 0 {
        let measured_in_batch = remaining_samples.min(SAMPLES_PER_BOOK_BATCH);
        let mut state = SweepBook::<O>::new();

        for warmup_round in 0..WARMUP_ROUNDS_PER_BOOK {
            let buy_first = warmup_round % 2 == 0;
            state.execute_pair(levels, None, None, buy_first, warmup_round == 0);
        }

        for local_round in 0..measured_in_batch {
            let buy_first = (measured_so_far + local_round) % 2 == 0;
            state.execute_pair(
                levels,
                Some(&mut buy_tracker),
                Some(&mut sell_tracker),
                buy_first,
                false,
            );
        }

        state.assert_fully_restored();
        measured_so_far += measured_in_batch;
        remaining_samples -= measured_in_batch;
    }

    assert_eq!(buy_tracker.len(), samples);
    assert_eq!(sell_tracker.len(), samples);

    DirectionalResults {
        buy: buy_tracker
            .precentiles()
            .expect("buy sweep tracker has no samples"),
        sell: sell_tracker
            .precentiles()
            .expect("sell sweep tracker has no samples"),
    }
}

struct SweepBook<O: OrderbookTrait> {
    book: O,
    id_counter: IdCounter,
    bid_prices: Vec<u32>,
    ask_prices: Vec<u32>,
    expected_bid_ids: Vec<Vec<OrderId>>,
    expected_ask_ids: Vec<Vec<OrderId>>,
}

impl<O: OrderbookTrait> SweepBook<O> {
    fn new() -> Self {
        let bid_prices = resting_prices(Side::Bid);
        let ask_prices = resting_prices(Side::Ask);
        let mut book = O::new();
        let mut id_counter = IdCounter::new();
        let expected_bid_ids = add_levels(
            &mut book,
            &mut id_counter,
            Side::Bid,
            bid_prices.iter().copied(),
        );
        let expected_ask_ids = add_levels(
            &mut book,
            &mut id_counter,
            Side::Ask,
            ask_prices.iter().copied(),
        );

        let state = Self {
            book,
            id_counter,
            bid_prices,
            ask_prices,
            expected_bid_ids,
            expected_ask_ids,
        };
        state.assert_fully_restored();
        state
    }

    fn execute_pair(
        &mut self,
        levels: u32,
        buy_tracker: Option<&mut LatencyTracker>,
        sell_tracker: Option<&mut LatencyTracker>,
        buy_first: bool,
        check_next_price: bool,
    ) {
        if buy_first {
            self.execute_validate_restore(levels, Side::Bid, buy_tracker, check_next_price);
            self.execute_validate_restore(levels, Side::Ask, sell_tracker, check_next_price);
        } else {
            self.execute_validate_restore(levels, Side::Ask, sell_tracker, check_next_price);
            self.execute_validate_restore(levels, Side::Bid, buy_tracker, check_next_price);
        }
    }

    fn execute_validate_restore(
        &mut self,
        levels: u32,
        market_side: Side,
        tracker: Option<&mut LatencyTracker>,
        check_next_price: bool,
    ) {
        let quantity = Quantity::define(sweep_quantity(levels));
        let result = match tracker {
            Some(tracker) => {
                tracker.record(|| self.book.execute_market_order(market_side, quantity))
            }
            None => self.book.execute_market_order(market_side, quantity),
        };

        let fills = match result {
            Ok(fills) => fills,
            Err(error) => {
                panic!("{market_side:?} sweep of {levels} levels failed unexpectedly: {error}")
            }
        };

        self.assert_expected_fills(levels, market_side, &fills);
        if check_next_price {
            self.assert_next_best_price(levels, market_side);
        }

        // Vec destruction and all state restoration deliberately occur after
        // the timed execute_market_order() call.
        drop(fills);
        self.restore_consumed_levels(levels, market_side);
    }

    fn assert_expected_fills(&self, levels: u32, market_side: Side, fills: &[Fill]) {
        let expected_fill_count = (levels * ORDERS_PER_LEVEL) as usize;
        assert_eq!(
            fills.len(),
            expected_fill_count,
            "sweep returned an unexpected number of fills"
        );

        let (prices, expected_ids) = self.expected_side(market_side);
        let mut total_quantity = 0_u32;
        for level_index in 0..levels as usize {
            let expected_price = Price::define(prices[level_index]);
            let level_ids = &expected_ids[level_index];
            assert_eq!(level_ids.len(), ORDERS_PER_LEVEL as usize);

            for order_index in 0..ORDERS_PER_LEVEL as usize {
                let fill_index = level_index * ORDERS_PER_LEVEL as usize + order_index;
                let fill = &fills[fill_index];

                assert_eq!(
                    fill.price, expected_price,
                    "sweep violated global price priority"
                );
                assert_eq!(
                    fill.quantity.value(),
                    QTY_PER_ORDER,
                    "sweep returned an unexpected fill quantity"
                );
                assert_eq!(
                    fill.maker_order_id, level_ids[order_index],
                    "sweep violated FIFO order or consumed an unexpected maker"
                );
                total_quantity += fill.quantity.value();
            }
        }

        assert_eq!(
            total_quantity,
            sweep_quantity(levels),
            "sweep returned the wrong total quantity"
        );
    }

    fn expected_side(&self, market_side: Side) -> (&[u32], &[Vec<OrderId>]) {
        match market_side {
            Side::Bid => (&self.ask_prices, &self.expected_ask_ids),
            Side::Ask => (&self.bid_prices, &self.expected_bid_ids),
        }
    }

    fn assert_next_best_price(&self, consumed_levels: u32, market_side: Side) {
        let (prices, _) = self.expected_side(market_side);
        let expected = prices
            .get(consumed_levels as usize)
            .copied()
            .map(Price::define);
        let actual = match market_side {
            Side::Bid => self.book.best_ask(),
            Side::Ask => self.book.best_bid(),
        };
        assert_eq!(actual, expected, "sweep left an unexpected best price");
    }

    fn restore_consumed_levels(&mut self, levels: u32, market_side: Side) {
        let resting_side = resting_side_for_market(market_side);

        for level_index in 0..levels as usize {
            let price = match resting_side {
                Side::Bid => self.bid_prices[level_index],
                Side::Ask => self.ask_prices[level_index],
            };
            let new_ids = add_one_level(&mut self.book, &mut self.id_counter, resting_side, price);

            match resting_side {
                Side::Bid => self.expected_bid_ids[level_index] = new_ids,
                Side::Ask => self.expected_ask_ids[level_index] = new_ids,
            }
        }
    }

    fn assert_fully_restored(&self) {
        let expected_depth = ORDERS_PER_LEVEL * QTY_PER_ORDER;
        for (price, side) in self
            .bid_prices
            .iter()
            .map(|&price| (price, Side::Bid))
            .chain(self.ask_prices.iter().map(|&price| (price, Side::Ask)))
        {
            assert_eq!(
                self.book.depth_at_price(Price::define(price), side),
                expected_depth,
                "book was not fully restored after a sweep batch"
            );
        }

        assert_eq!(
            self.book.best_bid(),
            self.bid_prices.first().copied().map(Price::define)
        );
        assert_eq!(
            self.book.best_ask(),
            self.ask_prices.first().copied().map(Price::define)
        );
    }
}

fn resting_prices(side: Side) -> Vec<u32> {
    (0..BOOK_PRICE_LEVELS_PER_SIDE)
        .map(|offset| match side {
            Side::Ask => MID_PRICE + offset,
            Side::Bid => MID_PRICE - 1 - offset,
        })
        .collect()
}

fn add_levels<O, I>(
    book: &mut O,
    id_counter: &mut IdCounter,
    side: Side,
    prices: I,
) -> Vec<Vec<OrderId>>
where
    O: OrderbookTrait,
    I: IntoIterator<Item = u32>,
{
    prices
        .into_iter()
        .map(|price| add_one_level(book, id_counter, side, price))
        .collect()
}

fn add_one_level<O: OrderbookTrait>(
    book: &mut O,
    id_counter: &mut IdCounter,
    side: Side,
    price: u32,
) -> Vec<OrderId> {
    let mut ids = Vec::with_capacity(ORDERS_PER_LEVEL as usize);
    for _ in 0..ORDERS_PER_LEVEL {
        let order = Order::new(
            Price::define(price),
            Quantity::define(QTY_PER_ORDER),
            side,
            id_counter,
        );
        ids.push(order.id());
        book.add_order(order)
            .unwrap_or_else(|error| panic!("failed to restore order at {price}: {error}"));
    }
    ids
}

fn resting_side_for_market(market_side: Side) -> Side {
    match market_side {
        Side::Bid => Side::Ask,
        Side::Ask => Side::Bid,
    }
}

fn sweep_quantity(levels: u32) -> u32 {
    levels
        .checked_mul(ORDERS_PER_LEVEL)
        .and_then(|quantity| quantity.checked_mul(QTY_PER_ORDER))
        .expect("sweep quantity overflowed u32")
}

fn print_results(results: &SweepResults, cpu_ghz: f64) {
    print_case_results("Small", SMALL_SWEEP_LEVELS, &results.small, cpu_ghz);
    println!();
    print_case_results("Medium", MEDIUM_SWEEP_LEVELS, &results.medium, cpu_ghz);
    println!();
    print_case_results("Large", LARGE_SWEEP_LEVELS, &results.large, cpu_ghz);
    println!();
    print_case_results(
        "Cross-zone",
        CROSS_ZONE_SWEEP_LEVELS,
        &results.cross_zone,
        cpu_ghz,
    );
}

fn print_case_results(name: &str, levels: u32, results: &DirectionalResults, cpu_ghz: f64) {
    println!("{name} sweep ({levels} levels):");
    print_percentiles("market buy", &results.buy, cpu_ghz);
    print_percentiles("market sell", &results.sell, cpu_ghz);
}

fn print_percentiles(name: &str, percentiles: &Percentiles, cpu_ghz: f64) {
    println!("  {name}:");
    println!(
        "    p50:    {:>8} cycles  ({:>8.1} ns)",
        percentiles.p50,
        cycles_to_ns(percentiles.p50, cpu_ghz)
    );
    println!(
        "    p99:    {:>8} cycles  ({:>8.1} ns)",
        percentiles.p99,
        cycles_to_ns(percentiles.p99, cpu_ghz)
    );
    println!(
        "    p99.9:  {:>8} cycles  ({:>8.1} ns)",
        percentiles.p999,
        cycles_to_ns(percentiles.p999, cpu_ghz)
    );
    println!(
        "    p99.99: {:>8} cycles  ({:>8.1} ns)",
        percentiles.p9999,
        cycles_to_ns(percentiles.p9999, cpu_ghz)
    );
    println!(
        "    Max:    {:>8} cycles  ({:>8.1} ns)",
        percentiles.max,
        cycles_to_ns(percentiles.max, cpu_ghz)
    );
}

fn print_comparison(
    fixed: &SweepResults,
    soa: &SweepResults,
    hybrid: &SweepResults,
    tree: &SweepResults,
) {
    println!(
        "{:<23} | {:>12} | {:>12} | {:>12} | {:>12}",
        "Sweep", "Fixed-Tick", "SoA", "Hybrid", "Tree"
    );
    println!("{:-<83}", "");

    for (label, fixed_p, soa_p, hybrid_p, tree_p) in comparison_rows(fixed, soa, hybrid, tree) {
        println!(
            "{:<23} | {:>10} cy | {:>10} cy | {:>10} cy | {:>10} cy",
            label, fixed_p.p50, soa_p.p50, hybrid_p.p50, tree_p.p50
        );
    }
}

type ComparisonRow<'a> = (
    &'static str,
    &'a Percentiles,
    &'a Percentiles,
    &'a Percentiles,
    &'a Percentiles,
);

fn comparison_rows<'a>(
    fixed: &'a SweepResults,
    soa: &'a SweepResults,
    hybrid: &'a SweepResults,
    tree: &'a SweepResults,
) -> [ComparisonRow<'a>; 8] {
    [
        (
            "Small buy (5)",
            &fixed.small.buy,
            &soa.small.buy,
            &hybrid.small.buy,
            &tree.small.buy,
        ),
        (
            "Small sell (5)",
            &fixed.small.sell,
            &soa.small.sell,
            &hybrid.small.sell,
            &tree.small.sell,
        ),
        (
            "Medium buy (20)",
            &fixed.medium.buy,
            &soa.medium.buy,
            &hybrid.medium.buy,
            &tree.medium.buy,
        ),
        (
            "Medium sell (20)",
            &fixed.medium.sell,
            &soa.medium.sell,
            &hybrid.medium.sell,
            &tree.medium.sell,
        ),
        (
            "Large buy (50)",
            &fixed.large.buy,
            &soa.large.buy,
            &hybrid.large.buy,
            &tree.large.buy,
        ),
        (
            "Large sell (50)",
            &fixed.large.sell,
            &soa.large.sell,
            &hybrid.large.sell,
            &tree.large.sell,
        ),
        (
            "Cross-zone buy (150)",
            &fixed.cross_zone.buy,
            &soa.cross_zone.buy,
            &hybrid.cross_zone.buy,
            &tree.cross_zone.buy,
        ),
        (
            "Cross-zone sell (150)",
            &fixed.cross_zone.sell,
            &soa.cross_zone.sell,
            &hybrid.cross_zone.sell,
            &tree.cross_zone.sell,
        ),
    ]
}

fn print_marginal_scaling(
    fixed: &SweepResults,
    soa: &SweepResults,
    hybrid: &SweepResults,
    tree: &SweepResults,
) {
    println!(
        "{:<23} | {:>12} | {:>12} | {:>12} | {:>12}",
        "Added levels", "Fixed-Tick", "SoA", "Hybrid", "Tree"
    );
    println!("{:-<83}", "");

    print_marginal_row(
        "Buy 5 -> 20",
        SMALL_SWEEP_LEVELS,
        MEDIUM_SWEEP_LEVELS,
        [
            &fixed.small.buy,
            &soa.small.buy,
            &hybrid.small.buy,
            &tree.small.buy,
        ],
        [
            &fixed.medium.buy,
            &soa.medium.buy,
            &hybrid.medium.buy,
            &tree.medium.buy,
        ],
    );
    print_marginal_row(
        "Sell 5 -> 20",
        SMALL_SWEEP_LEVELS,
        MEDIUM_SWEEP_LEVELS,
        [
            &fixed.small.sell,
            &soa.small.sell,
            &hybrid.small.sell,
            &tree.small.sell,
        ],
        [
            &fixed.medium.sell,
            &soa.medium.sell,
            &hybrid.medium.sell,
            &tree.medium.sell,
        ],
    );
    print_marginal_row(
        "Buy 20 -> 50",
        MEDIUM_SWEEP_LEVELS,
        LARGE_SWEEP_LEVELS,
        [
            &fixed.medium.buy,
            &soa.medium.buy,
            &hybrid.medium.buy,
            &tree.medium.buy,
        ],
        [
            &fixed.large.buy,
            &soa.large.buy,
            &hybrid.large.buy,
            &tree.large.buy,
        ],
    );
    print_marginal_row(
        "Sell 20 -> 50",
        MEDIUM_SWEEP_LEVELS,
        LARGE_SWEEP_LEVELS,
        [
            &fixed.medium.sell,
            &soa.medium.sell,
            &hybrid.medium.sell,
            &tree.medium.sell,
        ],
        [
            &fixed.large.sell,
            &soa.large.sell,
            &hybrid.large.sell,
            &tree.large.sell,
        ],
    );
    print_marginal_row(
        "Buy 50 -> 150*",
        LARGE_SWEEP_LEVELS,
        CROSS_ZONE_SWEEP_LEVELS,
        [
            &fixed.large.buy,
            &soa.large.buy,
            &hybrid.large.buy,
            &tree.large.buy,
        ],
        [
            &fixed.cross_zone.buy,
            &soa.cross_zone.buy,
            &hybrid.cross_zone.buy,
            &tree.cross_zone.buy,
        ],
    );
    print_marginal_row(
        "Sell 50 -> 150*",
        LARGE_SWEEP_LEVELS,
        CROSS_ZONE_SWEEP_LEVELS,
        [
            &fixed.large.sell,
            &soa.large.sell,
            &hybrid.large.sell,
            &tree.large.sell,
        ],
        [
            &fixed.cross_zone.sell,
            &soa.cross_zone.sell,
            &hybrid.cross_zone.sell,
            &tree.cross_zone.sell,
        ],
    );

    println!("\n* 50 -> 150 includes Hybrid's transition from hot array to cold tree.");
    println!("  These are differences between p50 values, not percentiles of per-level cost.");
}

fn print_marginal_row(
    label: &str,
    from_levels: u32,
    to_levels: u32,
    from: [&Percentiles; 4],
    to: [&Percentiles; 4],
) {
    let delta_levels = (to_levels - from_levels) as f64;
    let marginal = |index: usize| (to[index].p50 as f64 - from[index].p50 as f64) / delta_levels;

    println!(
        "{:<23} | {:>9.1} cy | {:>9.1} cy | {:>9.1} cy | {:>9.1} cy",
        label,
        marginal(0),
        marginal(1),
        marginal(2),
        marginal(3)
    );
}

fn export_csv(
    cpu_ghz: f64,
    fixed: &SweepResults,
    soa: &SweepResults,
    hybrid: &SweepResults,
    tree: &SweepResults,
) {
    let implementations = [
        ("fixed_tick", fixed),
        ("soa", soa),
        ("hybrid", hybrid),
        ("tree", tree),
    ];

    match CsvExporter::create("scenario_sweep") {
        Ok(mut csv) => {
            for (implementation, results) in implementations {
                for (operation, percentiles) in csv_rows(results) {
                    if let Err(error) = csv.append(&ResultRow {
                        scenario: "scenario_sweep",
                        implementation,
                        operation,
                        cpu_ghz,
                        percentiles,
                    }) {
                        eprintln!("Warning: could not append sweep CSV row: {error}");
                    }
                }
            }
        }
        Err(error) => eprintln!("Warning: could not write CSV: {error}"),
    }
}

fn csv_rows(results: &SweepResults) -> [(&'static str, &Percentiles); 8] {
    [
        ("small_buy_sweep", &results.small.buy),
        ("small_sell_sweep", &results.small.sell),
        ("medium_buy_sweep", &results.medium.buy),
        ("medium_sell_sweep", &results.medium.sell),
        ("large_buy_sweep", &results.large.buy),
        ("large_sell_sweep", &results.large.sell),
        ("cross_zone_buy_sweep", &results.cross_zone.buy),
        ("cross_zone_sell_sweep", &results.cross_zone.sell),
    ]
}
