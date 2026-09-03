//! Scenario 4.2c: Order Book Build-Up
//!
//! Measures how add_order() latency changes as a logically fresh book grows
//! from empty to and beyond a 10,000-order reference depth.
//!
//! Run with: cargo run --release --example scenario_buildup

use orderbook::analysis::{CsvExporter, ResultRow};
use orderbook::methodology::latency::{LatencyTracker, Percentiles};
use orderbook::methodology::{get_tsc_frequency, tsc_ticks_to_ns};
use orderbook::orderbook::OrderbookTrait;
use orderbook::orderbook::SoA::orderbook::Orderbook as SoAOrderbook;
use orderbook::orderbook::fixed_tick::orderbook::Orderbook as FixedTickOrderbook;
use orderbook::orderbook::hybrid::orderbook::Orderbook as HybridOrderbook;
use orderbook::orderbook::tree::orderbook::Orderbook as TreeOrderbook;
use orderbook::types::order::{IdCounter, Order, Side};
use orderbook::types::price::Price;
use orderbook::types::quantity::Quantity;
use rand::SeedableRng;
use rand::prelude::*;
use rand::rngs::StdRng;

const MIN_PRICE: u32 = 1;
const MID_PRICE: u32 = 5_000;
const MAX_PRICE_EXCLUSIVE: u32 = 10_000;
const ORDER_QUANTITY: u32 = 100;

const REFERENCE_BOOK_ORDERS: usize = 10_000;
const MEASUREMENT_PERCENTAGES: [usize; 5] = [0, 25, 50, 75, 100];
const MEASURED_ADDS_PER_BOOK: usize = 500;
const DEFAULT_SAMPLES_PER_POINT: usize = 1_000_000;
const SAMPLE_OVERRIDE_ENV: &str = "ORDERBOOK_BUILDUP_SAMPLES_PER_POINT";
const BASE_SEED: u64 = 42;

const CSV_OPERATIONS: [&str; 5] = [
    "add_depth_0_499",
    "add_depth_2500_2999",
    "add_depth_5000_5499",
    "add_depth_7500_7999",
    "add_depth_10000_10499",
];

// ============================================================================
// Scenario 4.2c: Order Book Build-Up
// ============================================================================
//
// PURPOSE:
// Sample add_order() latency at known depth windows while the same book grows.
// The windows begin at 0, 2,500, 5,000, 7,500, and 10,000 resting orders.
//
// REPEATED LIFECYCLE:
// 1. Pre-generate one deterministic logical order stream.
// 2. Construct a fresh empty book (construction itself is not timed).
// 3. At each checkpoint, prefill untimed to the exact target depth.
// 4. Time the next 500 additions, advancing one cursor exactly once per order.
// 5. Repeat the lifecycle 2,000 times by default, producing one million
//    samples for every depth window.
//
// PRICE MODEL:
// - Bids are uniform in [1, 5000), so every bid is below the spread.
// - Asks are uniform in [5000, 10000), so every ask is above the spread.
// - Every complete 500-add window contains 250 bids and 250 asks.
// This creates many distinct levels. Roughly 2% of prices fall in Hybrid's hot
// array and 98% use its cold tree.
//
// TIMING MODEL:
// Only add_order() is timed. RNG, Order construction, Result checking, prefill,
// and end-of-lifecycle book validation are outside the timed region. Allocation
// or rehash events that occur in untimed gaps are intentionally not sampled.
//
// A fresh lifecycle means a logically new book. The process, allocator, and
// operating-system page cache naturally remain warm across repeated books.
// ============================================================================

fn main() {
    println!("=== Scenario 4.2c: Order Book Build-Up ===\n");

    let tsc_ghz = get_tsc_frequency();
    println!("TSC frequency (calibrated): {:.3} GHz", tsc_ghz);

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

    let samples_per_point = samples_per_point();
    let books_required = samples_per_point.div_ceil(MEASURED_ADDS_PER_BOOK);

    println!("\nBuild-Up Parameters:");
    println!(
        "  Reference depth: {} resting orders",
        REFERENCE_BOOK_ORDERS
    );
    println!("  Starting checkpoints: {:?}%", MEASUREMENT_PERCENTAGES);
    for (percentage, target) in MEASUREMENT_PERCENTAGES.into_iter().zip(checkpoint_orders()) {
        println!(
            "    {:>3}%: measure additions at depths {} through {}",
            percentage,
            target,
            target + MEASURED_ADDS_PER_BOOK - 1
        );
    }
    println!(
        "  Measured additions per checkpoint per book: {}",
        MEASURED_ADDS_PER_BOOK
    );
    println!(
        "  Logical book lifecycles per implementation: {}",
        books_required
    );
    println!("  Measurements per depth window: {}", samples_per_point);
    println!(
        "  Measured add_order calls per implementation: {}",
        samples_per_point * MEASUREMENT_PERCENTAGES.len()
    );
    println!("  Quantity per order: {}", ORDER_QUANTITY);
    println!("  Bid prices: [1, 5000)");
    println!("  Ask prices: [5000, 10000)");
    println!("  Side mix per complete window: 250 bids + 250 asks");
    println!("  Hybrid locality: approximately 2% hot + 98% cold-tree adds");
    println!("  Timing: add_order only; prefill and validation are untimed\n");

    println!("--- Fixed-Tick Array ---");
    let fixed = run_buildup_benchmark::<FixedTickOrderbook>(BASE_SEED, samples_per_point);
    print_results(&fixed, tsc_ghz);

    println!("\n--- Structure-of-Arrays (SoA) ---");
    let soa = run_buildup_benchmark::<SoAOrderbook>(BASE_SEED, samples_per_point);
    print_results(&soa, tsc_ghz);

    println!("\n--- Hybrid (Hot/Cold) ---");
    let hybrid = run_buildup_benchmark::<HybridOrderbook>(BASE_SEED, samples_per_point);
    print_results(&hybrid, tsc_ghz);

    println!("\n--- Tree-Based ---");
    let tree = run_buildup_benchmark::<TreeOrderbook>(BASE_SEED, samples_per_point);
    print_results(&tree, tsc_ghz);

    println!("\n--- Comparison: p50 add latency by starting depth ---");
    print_comparison(&fixed, &soa, &hybrid, &tree);

    println!("\n--- Depth Effect: p50 change from early to mature book ---");
    print_depth_analysis(&fixed, &soa, &hybrid, &tree);

    export_csv(tsc_ghz, &fixed, &soa, &hybrid, &tree);
}

fn samples_per_point() -> usize {
    match std::env::var(SAMPLE_OVERRIDE_ENV) {
        Ok(value) => value
            .parse::<usize>()
            .ok()
            .filter(|&samples| samples > 0)
            .unwrap_or_else(|| {
                panic!("{SAMPLE_OVERRIDE_ENV} must be a positive integer, got {value:?}")
            }),
        Err(std::env::VarError::NotPresent) => DEFAULT_SAMPLES_PER_POINT,
        Err(error) => panic!("could not read {SAMPLE_OVERRIDE_ENV}: {error}"),
    }
}

fn checkpoint_orders() -> [usize; 5] {
    MEASUREMENT_PERCENTAGES.map(|percentage| REFERENCE_BOOK_ORDERS * percentage / 100)
}

#[derive(Clone, Copy)]
struct OrderSpec {
    price: u32,
    side: Side,
}

fn generate_order_stream(seed: u64, order_count: usize) -> Vec<OrderSpec> {
    let mut rng = StdRng::seed_from_u64(seed);
    (0..order_count)
        .map(|index| {
            // Every even-length depth window is exactly 50/50. Flip the first
            // side between books so neither side permanently leads the stream.
            let side = if (index + seed as usize) % 2 == 0 {
                Side::Bid
            } else {
                Side::Ask
            };
            let price = match side {
                Side::Bid => rng.random_range(MIN_PRICE..MID_PRICE),
                Side::Ask => rng.random_range(MID_PRICE..MAX_PRICE_EXCLUSIVE),
            };
            OrderSpec { price, side }
        })
        .collect()
}

struct BuildupResults {
    at_checkpoint: [Percentiles; 5],
}

fn run_buildup_benchmark<O: OrderbookTrait>(seed: u64, samples: usize) -> BuildupResults {
    let mut trackers: [LatencyTracker; 5] = std::array::from_fn(|_| LatencyTracker::new(samples));
    let books_required = samples.div_ceil(MEASURED_ADDS_PER_BOOK);

    for book_index in 0..books_required {
        let samples_already_collected = trackers[0].len();
        let measured_this_book = (samples - samples_already_collected).min(MEASURED_ADDS_PER_BOOK);
        assert!(
            trackers
                .iter()
                .all(|tracker| tracker.len() == samples_already_collected)
        );

        let maximum_orders = REFERENCE_BOOK_ORDERS + measured_this_book;
        let stream_seed = seed.wrapping_add(book_index as u64);
        let order_stream = generate_order_stream(stream_seed, maximum_orders);

        // Generate the stream before O::new() so RNG work cannot disturb the
        // freshly created book immediately before its first measured window.
        let mut book = O::new();
        let mut id_counter = IdCounter::new();
        let mut cursor = 0_usize;

        for (checkpoint_index, target_orders) in checkpoint_orders().into_iter().enumerate() {
            assert!(cursor <= target_orders);

            while cursor < target_orders {
                add_spec_unmeasured(&mut book, &mut id_counter, order_stream[cursor]);
                cursor += 1;
            }
            assert_eq!(cursor, target_orders);

            for _ in 0..measured_this_book {
                let order = order_from_spec(order_stream[cursor], &mut id_counter);
                let result = trackers[checkpoint_index].record(|| book.add_order(order));
                match result {
                    Ok(()) => {}
                    Err(error) => panic!(
                        "add_order failed in {}% depth window: {error}",
                        MEASUREMENT_PERCENTAGES[checkpoint_index]
                    ),
                }
                cursor += 1;
            }
        }

        assert_eq!(cursor, REFERENCE_BOOK_ORDERS + measured_this_book);
        assert_non_crossed_book(&book);
        if book_index == 0 {
            assert_book_matches_stream(&book, &order_stream[..cursor]);
        }
    }

    for tracker in &trackers {
        assert_eq!(tracker.len(), samples);
    }

    BuildupResults {
        at_checkpoint: std::array::from_fn(|index| {
            trackers[index]
                .precentiles()
                .expect("buildup tracker has no samples")
        }),
    }
}

fn order_from_spec(spec: OrderSpec, id_counter: &mut IdCounter) -> Order {
    Order::new(
        Price::define(spec.price),
        Quantity::define(ORDER_QUANTITY),
        spec.side,
        id_counter,
    )
}

fn add_spec_unmeasured<O: OrderbookTrait>(
    book: &mut O,
    id_counter: &mut IdCounter,
    spec: OrderSpec,
) {
    let order = order_from_spec(spec, id_counter);
    book.add_order(order)
        .unwrap_or_else(|error| panic!("untimed buildup add failed: {error}"));
}

fn assert_non_crossed_book<O: OrderbookTrait>(book: &O) {
    if let (Some(best_bid), Some(best_ask)) = (book.best_bid(), book.best_ask()) {
        assert!(
            best_bid.value() < best_ask.value(),
            "buildup workload produced a crossed book"
        );
    }
}

fn assert_book_matches_stream<O: OrderbookTrait>(book: &O, stream: &[OrderSpec]) {
    let mut expected_depth = vec![0_u32; MAX_PRICE_EXCLUSIVE as usize];
    for spec in stream {
        expected_depth[spec.price as usize] += ORDER_QUANTITY;
    }

    for (price, &expected) in expected_depth.iter().enumerate().skip(MIN_PRICE as usize) {
        let price = price as u32;
        let side = if price < MID_PRICE {
            Side::Bid
        } else {
            Side::Ask
        };
        assert_eq!(
            book.depth_at_price(Price::define(price), side),
            expected,
            "buildup depth did not match the generated logical workload"
        );
    }

    let expected_best_bid = (MIN_PRICE..MID_PRICE)
        .rev()
        .find(|&price| expected_depth[price as usize] > 0)
        .map(Price::define);
    let expected_best_ask = (MID_PRICE..MAX_PRICE_EXCLUSIVE)
        .find(|&price| expected_depth[price as usize] > 0)
        .map(Price::define);
    assert_eq!(book.best_bid(), expected_best_bid);
    assert_eq!(book.best_ask(), expected_best_ask);
}

fn print_results(results: &BuildupResults, tsc_ghz: f64) {
    println!("add_order() latency by starting depth window:");
    println!(
        "{:<11} | {:>10} | {:>10} | {:>10} | {:>10} | {:>10}",
        "Start", "p50", "p99", "p99.9", "p99.99", "Max"
    );
    println!("{:-<79}", "");

    for (index, percentage) in MEASUREMENT_PERCENTAGES.into_iter().enumerate() {
        let p = &results.at_checkpoint[index];
        println!(
            "{:<11} | {:>8}cy | {:>8}cy | {:>8}cy | {:>8}cy | {:>8}cy",
            format!("{}%", percentage),
            p.p50,
            p.p99,
            p.p999,
            p.p9999,
            p.max
        );
    }

    println!("\nMedian latency in nanoseconds:");
    for (index, percentage) in MEASUREMENT_PERCENTAGES.into_iter().enumerate() {
        let p50 = results.at_checkpoint[index].p50;
        println!(
            "  {:>3}% start: {:>8.1} ns",
            percentage,
            tsc_ticks_to_ns(p50, tsc_ghz)
        );
    }
}

fn print_comparison(
    fixed: &BuildupResults,
    soa: &BuildupResults,
    hybrid: &BuildupResults,
    tree: &BuildupResults,
) {
    println!(
        "{:<11} | {:>12} | {:>12} | {:>12} | {:>12}",
        "Start", "Fixed-Tick", "SoA", "Hybrid", "Tree"
    );
    println!("{:-<75}", "");

    for (index, percentage) in MEASUREMENT_PERCENTAGES.into_iter().enumerate() {
        println!(
            "{:<11} | {:>10} cy | {:>10} cy | {:>10} cy | {:>10} cy",
            format!("{}%", percentage),
            fixed.at_checkpoint[index].p50,
            soa.at_checkpoint[index].p50,
            hybrid.at_checkpoint[index].p50,
            tree.at_checkpoint[index].p50
        );
    }
}

fn print_depth_analysis(
    fixed: &BuildupResults,
    soa: &BuildupResults,
    hybrid: &BuildupResults,
    tree: &BuildupResults,
) {
    let early = 0;
    let mature = MEASUREMENT_PERCENTAGES.len() - 1;
    let change = |results: &BuildupResults| {
        percentage_change(
            results.at_checkpoint[early].p50,
            results.at_checkpoint[mature].p50,
        )
    };

    println!(
        "{:<15} | {:>12} | {:>12} | {:>12} | {:>12}",
        "Metric", "Fixed-Tick", "SoA", "Hybrid", "Tree"
    );
    println!("{:-<75}", "");
    println!(
        "{:<15} | {:>10} cy | {:>10} cy | {:>10} cy | {:>10} cy",
        "Early (0%)",
        fixed.at_checkpoint[early].p50,
        soa.at_checkpoint[early].p50,
        hybrid.at_checkpoint[early].p50,
        tree.at_checkpoint[early].p50
    );
    println!(
        "{:<15} | {:>10} cy | {:>10} cy | {:>10} cy | {:>10} cy",
        "Mature (100%)",
        fixed.at_checkpoint[mature].p50,
        soa.at_checkpoint[mature].p50,
        hybrid.at_checkpoint[mature].p50,
        tree.at_checkpoint[mature].p50
    );
    println!(
        "{:<15} | {:>10.1}% | {:>10.1}% | {:>10.1}% | {:>10.1}%",
        "Change",
        change(fixed),
        change(soa),
        change(hybrid),
        change(tree)
    );

    println!("\nInterpretation:");
    println!("  Negative change = median additions became faster as depth increased.");
    println!("  Positive change = median additions became slower as depth increased.");
    println!("  Near zero = median add latency was largely depth-independent.");
}

fn percentage_change(early: u64, mature: u64) -> f64 {
    if early == 0 {
        0.0
    } else {
        (mature as f64 - early as f64) * 100.0 / early as f64
    }
}

fn export_csv(
    tsc_ghz: f64,
    fixed: &BuildupResults,
    soa: &BuildupResults,
    hybrid: &BuildupResults,
    tree: &BuildupResults,
) {
    let implementations = [
        ("fixed_tick", fixed),
        ("soa", soa),
        ("hybrid", hybrid),
        ("tree", tree),
    ];

    match CsvExporter::create("scenario_buildup") {
        Ok(mut csv) => {
            for (implementation, results) in implementations {
                for (index, operation) in CSV_OPERATIONS.into_iter().enumerate() {
                    if let Err(error) = csv.append(&ResultRow {
                        scenario: "scenario_buildup",
                        implementation,
                        operation,
                        tsc_ghz,
                        percentiles: &results.at_checkpoint[index],
                    }) {
                        eprintln!("Warning: could not append buildup CSV row: {error}");
                    }
                }
            }
        }
        Err(error) => eprintln!("Warning: could not write CSV: {error}"),
    }
}
