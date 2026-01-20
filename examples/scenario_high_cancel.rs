/// Scenario 4.2a: High Cancellation Ratio (10:1)
///
/// 10 cancels for every 1 trade - typical HFT market maker pattern
/// Stresses cancel/update performance
///
/// Run with: cargo run --release --example scenario_high_cancel
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
const PRICE_SPREAD: u32 = 50; // Orders within ±25 ticks of mid

// High cancellation parameters
const ORDERS_PER_ROUND: usize = 100;
const CANCELS_PER_ROUND: usize = 90; // 90% cancel rate
const TRADES_PER_ROUND: usize = 10; // 10% trade rate
const NUM_ROUNDS: usize = 50;

// ============================================================================
// Scenario 4.2a: High Cancellation Ratio (10:1)
// ============================================================================
//
// PURPOSE: Stress cancel/update performance (typical HFT pattern)
//
// WHAT THIS SIMULATES:
// High-frequency trading market makers constantly update their quotes:
// - Place orders at various price levels
// - Cancel most orders before they execute (quote updates, risk management)
// - Only a small fraction actually trade
//
// REAL-WORLD CONTEXT:
// - HFT firms cancel 90-99% of their orders
// - Average order lifetime: milliseconds to seconds
// - Cancel-to-trade ratio of 10:1 is conservative; some firms are 100:1+
// - Regulators track this ratio as a market quality metric
//
// PATTERN PER ROUND:
// 1. Add 100 orders (spread around mid-price)
// 2. Cancel 90 of them (simulating quote updates)
// 3. Execute market orders against remaining 10
//
// WHAT THIS TESTS:
// 1. Cancel Performance: The dominant operation. O(1) lookup is critical.
//    Implementations using hash maps for order lookup should excel.
//
// 2. Memory Churn: Rapid allocation/deallocation of short-lived orders.
//    Tests allocator efficiency and potential fragmentation.
//
// 3. Mixed Workload: Interleaved add/cancel/execute, not isolated phases.
//    Tests how operations interact (cache pollution, lock contention).
//
// 4. Order Lookup: Every cancel needs to find the order by ID.
//    - HashMap: O(1)
//    - Linear search: O(n) - disaster
//    - Tree by ID: O(log n)
//
// EXPECTED RESULTS:
// - Implementations with O(1) order lookup should have low cancel latency
// - Memory allocator pressure may cause occasional spikes
// - Cancel p50 is the most important metric here
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
    println!("  Orders per round: {}", ORDERS_PER_ROUND);
    println!("  Cancels per round: {} ({}%)", CANCELS_PER_ROUND, CANCELS_PER_ROUND * 100 / ORDERS_PER_ROUND);
    println!("  Trades per round: {} ({}%)", TRADES_PER_ROUND, TRADES_PER_ROUND * 100 / ORDERS_PER_ROUND);
    println!("  Number of rounds: {}", NUM_ROUNDS);
    println!("  Cancel:Trade ratio: {}:1", CANCELS_PER_ROUND / TRADES_PER_ROUND);
    println!("  Total operations: {} adds, {} cancels, {} trades\n",
        ORDERS_PER_ROUND * NUM_ROUNDS,
        CANCELS_PER_ROUND * NUM_ROUNDS,
        TRADES_PER_ROUND * NUM_ROUNDS
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
}

struct ScenarioResults {
    add_order: Percentiles,
    cancel_order: Percentiles,
    market_order: Percentiles,
}

fn scenario_high_cancel<O: OrderbookTrait>(seed: u64) -> ScenarioResults {
    let mut rng = StdRng::seed_from_u64(seed);

    let total_adds = ORDERS_PER_ROUND * NUM_ROUNDS;
    let total_cancels = CANCELS_PER_ROUND * NUM_ROUNDS;
    let total_trades = TRADES_PER_ROUND * NUM_ROUNDS;

    let mut add_tracker = LatencyTracker::new(total_adds);
    let mut cancel_tracker = LatencyTracker::new(total_cancels);
    let mut market_tracker = LatencyTracker::new(total_trades);

    let mut book = O::new();
    let mut id_counter = IdCounter::new();

    for _round in 0..NUM_ROUNDS {
        let mut round_order_ids: Vec<OrderId> = Vec::with_capacity(ORDERS_PER_ROUND);

        // Phase 1: Add orders (mix of bids and asks around mid)
        for i in 0..ORDERS_PER_ROUND {
            let side = if i % 2 == 0 { Side::Bid } else { Side::Ask };
            let offset = rng.random_range(0..PRICE_SPREAD);
            let price_value = if side == Side::Bid {
                // Bids below mid
                (MID_PRICE - PRICE_SPREAD / 2 + offset / 2).clamp(1, 9999)
            } else {
                // Asks above mid
                (MID_PRICE + offset / 2).clamp(1, 9999)
            };

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

            round_order_ids.push(order_id);
        }

        // Shuffle to simulate random cancel pattern (not FIFO)
        round_order_ids.shuffle(&mut rng);

        // Phase 2: Cancel 90% of orders
        for &order_id in round_order_ids.iter().take(CANCELS_PER_ROUND) {
            cancel_tracker.record(|| {
                book.cancel_order(order_id).expect("Failed to cancel order");
            });
        }

        // Phase 3: Execute market orders against remaining 10%
        // The remaining orders are split roughly 50/50 bid/ask
        // Execute market orders to clear them
        for _ in 0..TRADES_PER_ROUND / 2 {
            // Buy (takes from asks)
            market_tracker.record(|| {
                let _ = book.execute_market_order(Side::Bid, Quantity::define(100));
            });
        }
        for _ in 0..TRADES_PER_ROUND / 2 {
            // Sell (takes from bids)
            market_tracker.record(|| {
                let _ = book.execute_market_order(Side::Ask, Quantity::define(100));
            });
        }
    }

    ScenarioResults {
        add_order: add_tracker.precentiles().expect("No add_order samples"),
        cancel_order: cancel_tracker
            .precentiles()
            .expect("No cancel_order samples"),
        market_order: market_tracker
            .precentiles()
            .expect("No market_order samples"),
    }
}

fn print_results(results: &ScenarioResults, cpu_ghz: f64) {
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
        "{:<12} | {:>7.0} ns | {:>7.0} ns | {:>7.0} ns | {:>7.0} ns",
        "p50 (ns)",
        cycles_to_ns(fixed.cancel_order.p50, cpu_ghz),
        cycles_to_ns(soa.cancel_order.p50, cpu_ghz),
        cycles_to_ns(hybrid.cancel_order.p50, cpu_ghz),
        cycles_to_ns(tree.cancel_order.p50, cpu_ghz),
    );
}


// For HFT market makers: Fixed-Tick or Hybrid would be preferred due to fast cancel latency, 
// despite slower market order execution (market makers rarely take liquidity).

