//! Scenario 4.2a: High Cancellation Ratio (10:1)
//!
//! 10 successful cancellations for every resting order that trades.
//! Stresses cancellation latency in a quote-heavy mixed workload.
//!
//! Run with: cargo run --release --example scenario_high_cancel

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
use rand::SeedableRng;
use rand::prelude::*;
use rand::rngs::StdRng;

const MID_PRICE: u32 = 5_000;
const PRICE_SPREAD: u32 = 50; // Orders within ±25 ticks of mid
const ORDER_QUANTITY: u32 = 100;

// High cancellation parameters
const ORDERS_PER_SIDE: usize = 55;
const CANCELS_PER_SIDE: usize = 50;
const MARKET_ORDERS_PER_SIDE: usize = ORDERS_PER_SIDE - CANCELS_PER_SIDE;
const ORDERS_PER_ROUND: usize = ORDERS_PER_SIDE * 2;
const CANCELS_PER_ROUND: usize = CANCELS_PER_SIDE * 2;
const TRADES_PER_ROUND: usize = MARKET_ORDERS_PER_SIDE * 2;

// Rebuild the book in batches so the workload has a repeatable steady state
// without growing one book indefinitely.
const BOOK_BATCHES: usize = 100;
const WARMUP_ROUNDS_PER_BOOK: usize = 10;
const MEASURED_ROUNDS_PER_BOOK: usize = 100;
const TOTAL_MEASURED_ROUNDS: usize = BOOK_BATCHES * MEASURED_ROUNDS_PER_BOOK;

// ============================================================================
// Scenario 4.2a: High Cancellation Ratio (10:1)
// ============================================================================
//
// PURPOSE: Stress cancellation performance in a quote-heavy workload.
//
// WHAT THIS SIMULATES:
// Market makers frequently update their quotes:
// - Place orders at various price levels
// - Cancel most orders before they execute (quote updates, risk management)
// - Only a small fraction actually trade
//
// PATTERN PER ROUND:
// 1. Add 55 bids and 55 asks near the midpoint.
// 2. Randomly select and cancel 50 bids and 50 asks.
// 3. Execute five market buys and five market sells. All ten must fill.
// 4. Verify that the round leaves the book empty.
//
// This produces exactly 100 cancels / 10 traded resting orders = 10:1.
//
// WHAT THIS TESTS:
// 1. Cancel Performance: The dominant operation. A hash index can find the
//    order metadata in expected O(1), but removing it can still require a price
//    lookup, an in-level search, and shifting elements in a Vec.
//
// 2. Memory Churn: Rapid allocation/deallocation of short-lived orders.
//    Tests allocator efficiency and potential fragmentation.
//
// 3. Mixed Workload: Interleaved add/cancel/execute, not isolated phases.
//    Tests how operations interact through book state and cache behavior.
//
// This benchmark is single-threaded. It does not model network arrival times,
// throughput, or lock contention. Prices stay near the midpoint, so Hybrid is
// intentionally measured on its hot path in this scenario.
// ============================================================================

fn main() {
    println!("=== Scenario 4.2a: High Cancellation Ratio (10:1) ===\n");

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

    println!("\nHFT Pattern Simulation:");
    println!(
        "  Orders per measured round: {} ({} bids + {} asks)",
        ORDERS_PER_ROUND, ORDERS_PER_SIDE, ORDERS_PER_SIDE
    );
    println!(
        "  Cancels per measured round: {} ({} per side)",
        CANCELS_PER_ROUND, CANCELS_PER_SIDE
    );
    println!(
        "  Successful market orders per measured round: {} ({} buys + {} sells)",
        TRADES_PER_ROUND, MARKET_ORDERS_PER_SIDE, MARKET_ORDERS_PER_SIDE
    );
    println!(
        "  Book batches: {} ({} warm-up + {} measured rounds each)",
        BOOK_BATCHES, WARMUP_ROUNDS_PER_BOOK, MEASURED_ROUNDS_PER_BOOK
    );
    println!("  Measured rounds: {}", TOTAL_MEASURED_ROUNDS);
    println!(
        "  Cancel:Trade ratio: {}:1",
        CANCELS_PER_ROUND / TRADES_PER_ROUND
    );
    println!(
        "  Measurements: {} adds, {} cancels, {} successful market orders\n",
        ORDERS_PER_ROUND * TOTAL_MEASURED_ROUNDS,
        CANCELS_PER_ROUND * TOTAL_MEASURED_ROUNDS,
        TRADES_PER_ROUND * TOTAL_MEASURED_ROUNDS
    );

    let seed: u64 = 42;

    println!("--- Fixed-Tick Array ---");
    let fixed = scenario_high_cancel::<FixedTickOrderbook>(seed);
    print_results(&fixed, cpu_ghz);

    println!("\n--- Structure-of-Arrays (SoA) ---");
    let soa = scenario_high_cancel::<SoAOrderbook>(seed);
    print_results(&soa, cpu_ghz);

    println!("\n--- Hybrid (Hot/Cold) ---");
    let hybrid = scenario_high_cancel::<HybridOrderbook>(seed);
    print_results(&hybrid, cpu_ghz);

    println!("\n--- Tree-Based ---");
    let tree = scenario_high_cancel::<TreeOrderbook>(seed);
    print_results(&tree, cpu_ghz);

    println!("\n--- Comparison (p50 latency in cycles) ---");
    print_comparison(&fixed, &soa, &hybrid, &tree);

    println!("\n--- Cancel Performance Focus (most critical for HFT) ---");
    print_cancel_focus(&fixed, &soa, &hybrid, &tree, cpu_ghz);

    // Export results to CSV
    let scenario_name = "scenario_high_cancel";
    let impls = [
        ("fixed_tick", &fixed),
        ("soa", &soa),
        ("hybrid", &hybrid),
        ("tree", &tree),
    ];
    match CsvExporter::create(scenario_name) {
        Ok(mut csv) => {
            for (impl_name, stats) in &impls {
                for (op, p) in [
                    ("add_order", &stats.add_order),
                    ("cancel_order", &stats.cancel_order),
                    ("market_order", &stats.market_order),
                ] {
                    let _ = csv.append(&ResultRow {
                        scenario: scenario_name,
                        implementation: impl_name,
                        operation: op,
                        cpu_ghz,
                        percentiles: p,
                    });
                }
            }
        }
        Err(e) => eprintln!("Warning: could not write CSV: {}", e),
    }
}

struct ScenarioResults {
    add_order: Percentiles,
    cancel_order: Percentiles,
    market_order: Percentiles,
}

struct ScenarioTrackers {
    add_order: LatencyTracker,
    cancel_order: LatencyTracker,
    market_order: LatencyTracker,
}

impl ScenarioTrackers {
    fn new(total_adds: usize, total_cancels: usize, total_trades: usize) -> Self {
        Self {
            add_order: LatencyTracker::new(total_adds),
            cancel_order: LatencyTracker::new(total_cancels),
            market_order: LatencyTracker::new(total_trades),
        }
    }
}

fn scenario_high_cancel<O: OrderbookTrait>(seed: u64) -> ScenarioResults {
    let total_adds = ORDERS_PER_ROUND * TOTAL_MEASURED_ROUNDS;
    let total_cancels = CANCELS_PER_ROUND * TOTAL_MEASURED_ROUNDS;
    let total_trades = TRADES_PER_ROUND * TOTAL_MEASURED_ROUNDS;
    let mut trackers = ScenarioTrackers::new(total_adds, total_cancels, total_trades);

    for book_batch in 0..BOOK_BATCHES {
        // Each implementation receives the same deterministic stream for every
        // batch. A fresh book bounds state growth; warm-up rounds populate
        // reusable allocations before measurement starts.
        let batch_seed = seed.wrapping_add(book_batch as u64);
        let mut rng = StdRng::seed_from_u64(batch_seed);
        let mut book = O::new();
        let mut id_counter = IdCounter::new();

        for _ in 0..WARMUP_ROUNDS_PER_BOOK {
            run_round(&mut book, &mut id_counter, &mut rng, None);
        }

        for _ in 0..MEASURED_ROUNDS_PER_BOOK {
            run_round(&mut book, &mut id_counter, &mut rng, Some(&mut trackers));
        }

        // Run the potentially expensive public best-price checks only after
        // the batch, so array scans cannot warm caches between timed rounds.
        assert!(
            book.best_bid().is_none() && book.best_ask().is_none(),
            "each book batch must finish empty"
        );
    }

    assert_eq!(trackers.add_order.len(), total_adds);
    assert_eq!(trackers.cancel_order.len(), total_cancels);
    assert_eq!(trackers.market_order.len(), total_trades);

    ScenarioResults {
        add_order: trackers
            .add_order
            .precentiles()
            .expect("No add_order samples"),
        cancel_order: trackers
            .cancel_order
            .precentiles()
            .expect("No cancel_order samples"),
        market_order: trackers
            .market_order
            .precentiles()
            .expect("No market_order samples"),
    }
}

fn run_round<O: OrderbookTrait>(
    book: &mut O,
    id_counter: &mut IdCounter,
    rng: &mut StdRng,
    mut trackers: Option<&mut ScenarioTrackers>,
) {
    let mut bid_ids: Vec<OrderId> = Vec::with_capacity(ORDERS_PER_SIDE);
    let mut ask_ids: Vec<OrderId> = Vec::with_capacity(ORDERS_PER_SIDE);
    let half_spread = PRICE_SPREAD / 2;

    // Keep bids below the midpoint and asks at or above it. Alternate sides so
    // add samples see a mixed stream. Order construction and ID bookkeeping
    // are deliberately outside the measured add_order call.
    for _ in 0..ORDERS_PER_SIDE {
        let bid_price = MID_PRICE - half_spread + rng.random_range(0..half_spread);
        let bid_order = Order::new(
            Price::define(bid_price),
            Quantity::define(ORDER_QUANTITY),
            Side::Bid,
            id_counter,
        );
        let bid_id = bid_order.id();

        let result = match trackers.as_deref_mut() {
            Some(trackers) => trackers.add_order.record(|| book.add_order(bid_order)),
            None => book.add_order(bid_order),
        };
        result.expect("bid add_order must succeed");
        bid_ids.push(bid_id);

        let ask_price = MID_PRICE + rng.random_range(0..half_spread);
        let ask_order = Order::new(
            Price::define(ask_price),
            Quantity::define(ORDER_QUANTITY),
            Side::Ask,
            id_counter,
        );
        let ask_id = ask_order.id();

        let result = match trackers.as_deref_mut() {
            Some(trackers) => trackers.add_order.record(|| book.add_order(ask_order)),
            None => book.add_order(ask_order),
        };
        result.expect("ask add_order must succeed");
        ask_ids.push(ask_id);
    }

    // Select the same number of cancels from each side. This preserves exactly
    // five bids and five asks, unlike one combined random shuffle.
    bid_ids.shuffle(rng);
    ask_ids.shuffle(rng);
    let mut surviving_bid_ids = bid_ids[CANCELS_PER_SIDE..].to_vec();
    let mut surviving_ask_ids = ask_ids[CANCELS_PER_SIDE..].to_vec();

    let mut cancel_ids = Vec::with_capacity(CANCELS_PER_ROUND);
    cancel_ids.extend_from_slice(&bid_ids[..CANCELS_PER_SIDE]);
    cancel_ids.extend_from_slice(&ask_ids[..CANCELS_PER_SIDE]);
    cancel_ids.shuffle(rng);

    for order_id in cancel_ids {
        let result = match trackers.as_deref_mut() {
            Some(trackers) => trackers.cancel_order.record(|| book.cancel_order(order_id)),
            None => book.cancel_order(order_id),
        };
        result.expect("cancel_order must succeed");
    }

    // A market buy consumes one remaining ask; a market sell consumes one
    // remaining bid. All validation happens after the timed call.
    for market_side in [Side::Bid, Side::Ask] {
        let expected_maker_ids = match market_side {
            Side::Bid => &mut surviving_ask_ids,
            Side::Ask => &mut surviving_bid_ids,
        };

        for _ in 0..MARKET_ORDERS_PER_SIDE {
            let result = match trackers.as_deref_mut() {
                Some(trackers) => trackers.market_order.record(|| {
                    book.execute_market_order(market_side, Quantity::define(ORDER_QUANTITY))
                }),
                None => book.execute_market_order(market_side, Quantity::define(ORDER_QUANTITY)),
            };
            let fills = result.expect("every market order must be fully filled");
            assert_expected_fill(&fills, expected_maker_ids);
        }
    }

    assert!(surviving_bid_ids.is_empty());
    assert!(surviving_ask_ids.is_empty());
}

fn assert_expected_fill(fills: &[Fill], expected_maker_ids: &mut Vec<OrderId>) {
    let filled_quantity: u32 = fills.iter().map(|fill| fill.quantity.value()).sum();
    assert_eq!(
        filled_quantity, ORDER_QUANTITY,
        "market order returned an unexpected fill quantity"
    );
    assert_eq!(
        fills.len(),
        1,
        "one market order should consume exactly one resting order"
    );

    let maker_order_id = fills[0].maker_order_id;
    let position = expected_maker_ids
        .iter()
        .position(|&order_id| order_id == maker_order_id)
        .expect("market order filled an unexpected resting order");
    expected_maker_ids.swap_remove(position);
}

fn print_results(results: &ScenarioResults, cpu_ghz: f64) {
    print_operation("add_order()", &results.add_order, cpu_ghz);
    println!();
    print_operation("cancel_order()", &results.cancel_order, cpu_ghz);
    println!();
    print_operation("execute_market_order()", &results.market_order, cpu_ghz);
}

fn print_operation(name: &str, percentiles: &Percentiles, cpu_ghz: f64) {
    println!("{name}:");
    println!(
        "  p50:  {:>8} cycles  ({:>7.1} ns)",
        percentiles.p50,
        cycles_to_ns(percentiles.p50, cpu_ghz)
    );
    println!(
        "  p99:  {:>8} cycles  ({:>7.1} ns)",
        percentiles.p99,
        cycles_to_ns(percentiles.p99, cpu_ghz)
    );
    println!(
        "  p99.9:{:>8} cycles  ({:>7.1} ns)",
        percentiles.p999,
        cycles_to_ns(percentiles.p999, cpu_ghz)
    );
    println!(
        "  p99.99:{:>7} cycles  ({:>7.1} ns)",
        percentiles.p9999,
        cycles_to_ns(percentiles.p9999, cpu_ghz)
    );
    println!(
        "  Max:  {:>8} cycles  ({:>7.1} ns)",
        percentiles.max,
        cycles_to_ns(percentiles.max, cpu_ghz)
    );
}

fn print_comparison(
    fixed: &ScenarioResults,
    soa: &ScenarioResults,
    hybrid: &ScenarioResults,
    tree: &ScenarioResults,
) {
    println!(
        "{:<15} | {:>12} | {:>12} | {:>12} | {:>12}",
        "Operation", "Fixed-Tick", "SoA", "Hybrid", "Tree"
    );
    println!("{:-<75}", "");
    println!(
        "{:<15} | {:>10} cy | {:>10} cy | {:>10} cy | {:>10} cy",
        "add_order",
        fixed.add_order.p50,
        soa.add_order.p50,
        hybrid.add_order.p50,
        tree.add_order.p50
    );
    println!(
        "{:<15} | {:>10} cy | {:>10} cy | {:>10} cy | {:>10} cy",
        "cancel_order",
        fixed.cancel_order.p50,
        soa.cancel_order.p50,
        hybrid.cancel_order.p50,
        tree.cancel_order.p50
    );
    println!(
        "{:<15} | {:>10} cy | {:>10} cy | {:>10} cy | {:>10} cy",
        "market_order",
        fixed.market_order.p50,
        soa.market_order.p50,
        hybrid.market_order.p50,
        tree.market_order.p50
    );
}

fn print_cancel_focus(
    fixed: &ScenarioResults,
    soa: &ScenarioResults,
    hybrid: &ScenarioResults,
    tree: &ScenarioResults,
    cpu_ghz: f64,
) {
    println!(
        "{:<12} | {:>10} | {:>10} | {:>10} | {:>10}",
        "Metric", "Fixed-Tick", "SoA", "Hybrid", "Tree"
    );
    println!("{:-<70}", "");
    println!(
        "{:<12} | {:>8} cy | {:>8} cy | {:>8} cy | {:>8} cy",
        "p50",
        fixed.cancel_order.p50,
        soa.cancel_order.p50,
        hybrid.cancel_order.p50,
        tree.cancel_order.p50
    );
    println!(
        "{:<12} | {:>8} cy | {:>8} cy | {:>8} cy | {:>8} cy",
        "p99",
        fixed.cancel_order.p99,
        soa.cancel_order.p99,
        hybrid.cancel_order.p99,
        tree.cancel_order.p99
    );
    println!(
        "{:<12} | {:>8} cy | {:>8} cy | {:>8} cy | {:>8} cy",
        "p99.9",
        fixed.cancel_order.p999,
        soa.cancel_order.p999,
        hybrid.cancel_order.p999,
        tree.cancel_order.p999
    );
    println!(
        "{:<12} | {:>8} cy | {:>8} cy | {:>8} cy | {:>8} cy",
        "p99.99",
        fixed.cancel_order.p9999,
        soa.cancel_order.p9999,
        hybrid.cancel_order.p9999,
        tree.cancel_order.p9999
    );
    println!(
        "{:<12} | {:>7.0} ns | {:>7.0} ns | {:>7.0} ns | {:>7.0} ns",
        "p50 (ns)",
        cycles_to_ns(fixed.cancel_order.p50, cpu_ghz),
        cycles_to_ns(soa.cancel_order.p50, cpu_ghz),
        cycles_to_ns(hybrid.cancel_order.p50, cpu_ghz),
        cycles_to_ns(tree.cancel_order.p50, cpu_ghz),
    );
}
