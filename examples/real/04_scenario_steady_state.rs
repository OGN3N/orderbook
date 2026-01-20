/// Scenario 4.2d: Steady-State Operations
///
/// Pre-populated book with mixed operation workload
/// Tests typical operation performance during normal trading
///
/// Run with: cargo run --release --example scenario_steady_state
use orderbook::orderbook::fixed_tick::orderbook::Orderbook as FixedTickOrderbook;
use orderbook::orderbook::hybrid::orderbook::Orderbook as HybridOrderbook;
use orderbook::orderbook::tree::orderbook::Orderbook as TreeOrderbook;
use orderbook::orderbook::OrderbookTrait;
use orderbook::orderbook::SoA::orderbook::Orderbook as SoAOrderbook;
use orderbook::perf::latency::{LatencyTracker, Percentiles};
use orderbook::perf::{cycles_to_ns, get_cpu_frequency};
use orderbook::types::order::{IdCounter, Order, OrderId, Side};
use orderbook::types::price::Price;
use orderbook::types::quantity::Quantity;
use rand::prelude::*;
use rand::rngs::StdRng;
use rand::SeedableRng;

const MID_PRICE: u32 = 5_000;
const PRICE_SPREAD: u32 = 100; // Orders within ±50 ticks

// Initial book state
const INITIAL_ORDERS: usize = 1_000;

// Steady-state workload
const NUM_OPERATIONS: usize = 10_000;
const ADD_RATIO: f64 = 0.60; // 60% adds
const CANCEL_RATIO: f64 = 0.30; // 30% cancels
const MARKET_RATIO: f64 = 0.10; // 10% market orders

// ============================================================================
// Scenario 4.2d: Steady-State Operations
// ============================================================================
//
// PURPOSE: Test typical operation performance on a pre-populated book
//
// WHAT THIS SIMULATES:
// Normal trading hours on an active instrument:
// - Book already has depth (market makers, resting orders)
// - Continuous flow of new orders, cancellations, and executions
// - Caches are warm, data structures are populated
//
// WORKLOAD MIX:
// - 60% add_order: New limit orders arriving
// - 30% cancel_order: Orders being pulled (quote updates, risk management)
// - 10% market_order: Aggressive orders taking liquidity
//
// This ratio is typical for liquid instruments. Less liquid instruments
// would have lower market order percentage.
//
// WHY THIS MATTERS:
// 1. Typical Case: Most of trading day is steady-state, not edge cases
//
// 2. Warm Cache: No cold-start effects, pure algorithm performance
//
// 3. Balanced View: Tests all operations together, not in isolation
//
// 4. Marketing Numbers: "Typical latency" you'd put in a datasheet
//
// 5. Regression Testing: Good baseline for performance regression detection
//
// EXPECTED RESULTS:
// - Lower latency than cold-start scenarios
// - Consistent performance across operations
// - This is the "happy path" for all implementations
// ============================================================================

fn main() {
    println!("=== Scenario 4.2d: Steady-State Operations ===\n");

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

    println!("\nSteady-State Parameters:");
    println!("  Initial book depth: {} orders", INITIAL_ORDERS);
    println!("  Total operations: {}", NUM_OPERATIONS);
    println!("  Workload mix:");
    println!("    - Add orders:    {:.0}% ({} ops)", ADD_RATIO * 100.0, (NUM_OPERATIONS as f64 * ADD_RATIO) as usize);
    println!("    - Cancel orders: {:.0}% ({} ops)", CANCEL_RATIO * 100.0, (NUM_OPERATIONS as f64 * CANCEL_RATIO) as usize);
    println!("    - Market orders: {:.0}% ({} ops)", MARKET_RATIO * 100.0, (NUM_OPERATIONS as f64 * MARKET_RATIO) as usize);
    println!();

    let seed: u64 = 42;

    println!("--- Fixed-Tick Array ---");
    let fixed = run_steady_state::<FixedTickOrderbook>(seed);
    print_results(&fixed, cpu_ghz);

    println!("\n--- Structure-of-Arrays (SoA) ---");
    let soa = run_steady_state::<SoAOrderbook>(seed);
    print_results(&soa, cpu_ghz);

    println!("\n--- Hybrid (Hot/Cold) ---");
    let hybrid = run_steady_state::<HybridOrderbook>(seed);
    print_results(&hybrid, cpu_ghz);

    println!("\n--- Tree-Based ---");
    let tree = run_steady_state::<TreeOrderbook>(seed);
    print_results(&tree, cpu_ghz);

    println!("\n--- Comparison: p50 latency (cycles) ---");
    print_comparison(&fixed, &soa, &hybrid, &tree);

    println!("\n--- Comparison: p50 latency (nanoseconds) ---");
    print_comparison_ns(&fixed, &soa, &hybrid, &tree, cpu_ghz);

    println!("\n--- Tail Latency Analysis (p99/p50 ratio) ---");
    print_tail_analysis(&fixed, &soa, &hybrid, &tree);

    println!("\n--- Summary: Best Implementation per Operation ---");
    print_summary(&fixed, &soa, &hybrid, &tree, cpu_ghz);
}

struct SteadyStateResults {
    add_order: Percentiles,
    cancel_order: Percentiles,
    market_order: Percentiles,
}

fn run_steady_state<O: OrderbookTrait>(seed: u64) -> SteadyStateResults {
    let mut rng = StdRng::seed_from_u64(seed);

    // Phase 1: Pre-populate the book
    let mut book = O::new();
    let mut id_counter = IdCounter::new();
    let mut active_order_ids: Vec<OrderId> = Vec::with_capacity(INITIAL_ORDERS * 2);

    for _ in 0..INITIAL_ORDERS {
        let side = if rng.random_bool(0.5) { Side::Bid } else { Side::Ask };
        let offset = rng.random_range(0..PRICE_SPREAD);
        let price_value = (MID_PRICE - PRICE_SPREAD / 2 + offset).clamp(1, 9999);

        let order = Order::new(
            Price::define(price_value),
            Quantity::define(100),
            side,
            &mut id_counter,
        );
        let order_id = order.id();
        book.add_order(order).expect("Failed to add initial order");
        active_order_ids.push(order_id);
    }

    // Phase 2: Run mixed workload
    let expected_adds = (NUM_OPERATIONS as f64 * ADD_RATIO) as usize;
    let expected_cancels = (NUM_OPERATIONS as f64 * CANCEL_RATIO) as usize;
    let expected_markets = (NUM_OPERATIONS as f64 * MARKET_RATIO) as usize;

    let mut add_tracker = LatencyTracker::new(expected_adds);
    let mut cancel_tracker = LatencyTracker::new(expected_cancels);
    let mut market_tracker = LatencyTracker::new(expected_markets);

    for _ in 0..NUM_OPERATIONS {
        let op_choice: f64 = rng.random();

        if op_choice < ADD_RATIO {
            // Add order
            let side = if rng.random_bool(0.5) { Side::Bid } else { Side::Ask };
            let offset = rng.random_range(0..PRICE_SPREAD);
            let price_value = (MID_PRICE - PRICE_SPREAD / 2 + offset).clamp(1, 9999);

            let order = Order::new(
                Price::define(price_value),
                Quantity::define(100),
                side,
                &mut id_counter,
            );
            let order_id = order.id();

            add_tracker.record(|| {
                book.add_order(order).expect("Failed to add order");
            });

            active_order_ids.push(order_id);
        } else if op_choice < ADD_RATIO + CANCEL_RATIO {
            // Cancel order (if we have any)
            if !active_order_ids.is_empty() {
                let idx = rng.random_range(0..active_order_ids.len());
                let order_id = active_order_ids.swap_remove(idx);

                cancel_tracker.record(|| {
                    let _ = book.cancel_order(order_id); // May fail if already executed
                });
            }
        } else {
            // Market order
            let side = if rng.random_bool(0.5) { Side::Bid } else { Side::Ask };

            market_tracker.record(|| {
                let _ = book.execute_market_order(side, Quantity::define(100));
            });
        }
    }

    SteadyStateResults {
        add_order: add_tracker.precentiles().expect("No add_order samples"),
        cancel_order: cancel_tracker
            .precentiles()
            .expect("No cancel_order samples"),
        market_order: market_tracker
            .precentiles()
            .expect("No market_order samples"),
    }
}

fn print_results(results: &SteadyStateResults, cpu_ghz: f64) {
    println!("add_order():");
    println!(
        "  p50:  {:>8} cycles  ({:>7.1} ns)",
        results.add_order.p50,
        cycles_to_ns(results.add_order.p50, cpu_ghz)
    );
    println!(
        "  p99:  {:>8} cycles  ({:>7.1} ns)",
        results.add_order.p99,
        cycles_to_ns(results.add_order.p99, cpu_ghz)
    );
    println!(
        "  Max:  {:>8} cycles  ({:>7.1} ns)",
        results.add_order.max,
        cycles_to_ns(results.add_order.max, cpu_ghz)
    );

    println!("\ncancel_order():");
    println!(
        "  p50:  {:>8} cycles  ({:>7.1} ns)",
        results.cancel_order.p50,
        cycles_to_ns(results.cancel_order.p50, cpu_ghz)
    );
    println!(
        "  p99:  {:>8} cycles  ({:>7.1} ns)",
        results.cancel_order.p99,
        cycles_to_ns(results.cancel_order.p99, cpu_ghz)
    );
    println!(
        "  Max:  {:>8} cycles  ({:>7.1} ns)",
        results.cancel_order.max,
        cycles_to_ns(results.cancel_order.max, cpu_ghz)
    );

    println!("\nexecute_market_order():");
    println!(
        "  p50:  {:>8} cycles  ({:>7.1} ns)",
        results.market_order.p50,
        cycles_to_ns(results.market_order.p50, cpu_ghz)
    );
    println!(
        "  p99:  {:>8} cycles  ({:>7.1} ns)",
        results.market_order.p99,
        cycles_to_ns(results.market_order.p99, cpu_ghz)
    );
    println!(
        "  Max:  {:>8} cycles  ({:>7.1} ns)",
        results.market_order.max,
        cycles_to_ns(results.market_order.max, cpu_ghz)
    );
}

fn print_comparison(
    fixed: &SteadyStateResults,
    soa: &SteadyStateResults,
    hybrid: &SteadyStateResults,
    tree: &SteadyStateResults,
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

fn print_comparison_ns(
    fixed: &SteadyStateResults,
    soa: &SteadyStateResults,
    hybrid: &SteadyStateResults,
    tree: &SteadyStateResults,
    cpu_ghz: f64,
) {
    println!(
        "{:<15} | {:>12} | {:>12} | {:>12} | {:>12}",
        "Operation", "Fixed-Tick", "SoA", "Hybrid", "Tree"
    );
    println!("{:-<75}", "");
    println!(
        "{:<15} | {:>10.0} ns | {:>10.0} ns | {:>10.0} ns | {:>10.0} ns",
        "add_order",
        cycles_to_ns(fixed.add_order.p50, cpu_ghz),
        cycles_to_ns(soa.add_order.p50, cpu_ghz),
        cycles_to_ns(hybrid.add_order.p50, cpu_ghz),
        cycles_to_ns(tree.add_order.p50, cpu_ghz)
    );
    println!(
        "{:<15} | {:>10.0} ns | {:>10.0} ns | {:>10.0} ns | {:>10.0} ns",
        "cancel_order",
        cycles_to_ns(fixed.cancel_order.p50, cpu_ghz),
        cycles_to_ns(soa.cancel_order.p50, cpu_ghz),
        cycles_to_ns(hybrid.cancel_order.p50, cpu_ghz),
        cycles_to_ns(tree.cancel_order.p50, cpu_ghz)
    );
    println!(
        "{:<15} | {:>10.0} ns | {:>10.0} ns | {:>10.0} ns | {:>10.0} ns",
        "market_order",
        cycles_to_ns(fixed.market_order.p50, cpu_ghz),
        cycles_to_ns(soa.market_order.p50, cpu_ghz),
        cycles_to_ns(hybrid.market_order.p50, cpu_ghz),
        cycles_to_ns(tree.market_order.p50, cpu_ghz)
    );
}

fn print_tail_analysis(
    fixed: &SteadyStateResults,
    soa: &SteadyStateResults,
    hybrid: &SteadyStateResults,
    tree: &SteadyStateResults,
) {
    let ratio = |p99: u64, p50: u64| -> f64 {
        if p50 == 0 { 0.0 } else { p99 as f64 / p50 as f64 }
    };

    println!(
        "{:<15} | {:>12} | {:>12} | {:>12} | {:>12}",
        "Operation", "Fixed-Tick", "SoA", "Hybrid", "Tree"
    );
    println!("{:-<75}", "");
    println!(
        "{:<15} | {:>10.1}x | {:>10.1}x | {:>10.1}x | {:>10.1}x",
        "add_order",
        ratio(fixed.add_order.p99, fixed.add_order.p50),
        ratio(soa.add_order.p99, soa.add_order.p50),
        ratio(hybrid.add_order.p99, hybrid.add_order.p50),
        ratio(tree.add_order.p99, tree.add_order.p50),
    );
    println!(
        "{:<15} | {:>10.1}x | {:>10.1}x | {:>10.1}x | {:>10.1}x",
        "cancel_order",
        ratio(fixed.cancel_order.p99, fixed.cancel_order.p50),
        ratio(soa.cancel_order.p99, soa.cancel_order.p50),
        ratio(hybrid.cancel_order.p99, hybrid.cancel_order.p50),
        ratio(tree.cancel_order.p99, tree.cancel_order.p50),
    );
    println!(
        "{:<15} | {:>10.1}x | {:>10.1}x | {:>10.1}x | {:>10.1}x",
        "market_order",
        ratio(fixed.market_order.p99, fixed.market_order.p50),
        ratio(soa.market_order.p99, soa.market_order.p50),
        ratio(hybrid.market_order.p99, hybrid.market_order.p50),
        ratio(tree.market_order.p99, tree.market_order.p50),
    );

    println!("\nInterpretation: Lower ratio = more predictable latency");
}

fn print_summary(
    fixed: &SteadyStateResults,
    soa: &SteadyStateResults,
    hybrid: &SteadyStateResults,
    tree: &SteadyStateResults,
    cpu_ghz: f64,
) {
    let implementations = [
        ("Fixed-Tick", fixed),
        ("SoA", soa),
        ("Hybrid", hybrid),
        ("Tree", tree),
    ];

    // Find best for each operation
    let best_add = implementations
        .iter()
        .min_by_key(|(_, r)| r.add_order.p50)
        .unwrap();
    let best_cancel = implementations
        .iter()
        .min_by_key(|(_, r)| r.cancel_order.p50)
        .unwrap();
    let best_market = implementations
        .iter()
        .min_by_key(|(_, r)| r.market_order.p50)
        .unwrap();

    println!(
        "  add_order:    {} ({} cy / {:.0} ns)",
        best_add.0,
        best_add.1.add_order.p50,
        cycles_to_ns(best_add.1.add_order.p50, cpu_ghz)
    );
    println!(
        "  cancel_order: {} ({} cy / {:.0} ns)",
        best_cancel.0,
        best_cancel.1.cancel_order.p50,
        cycles_to_ns(best_cancel.1.cancel_order.p50, cpu_ghz)
    );
    println!(
        "  market_order: {} ({} cy / {:.0} ns)",
        best_market.0,
        best_market.1.market_order.p50,
        cycles_to_ns(best_market.1.market_order.p50, cpu_ghz)
    );

    // Calculate weighted average based on workload mix
    println!("\n--- Weighted Average (by workload mix) ---");
    for (name, results) in implementations.iter() {
        let weighted_avg = (results.add_order.p50 as f64 * ADD_RATIO)
            + (results.cancel_order.p50 as f64 * CANCEL_RATIO)
            + (results.market_order.p50 as f64 * MARKET_RATIO);
        println!(
            "  {:<12}: {:>7.0} cy / {:>6.0} ns",
            name,
            weighted_avg,
            weighted_avg / cpu_ghz
        );
    }
}
