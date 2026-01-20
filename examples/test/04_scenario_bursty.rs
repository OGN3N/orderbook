/// Scenario 4.1d: Bursty Traffic
///
/// Periods of high activity followed by quiet periods
/// Tests cache eviction under pressure
///
/// Run with: cargo run --release --example scenario_bursty
use orderbook::orderbook::fixed_tick::orderbook::Orderbook as FixedTickOrderbook;
use orderbook::orderbook::hybrid::orderbook::Orderbook as HybridOrderbook;
use orderbook::orderbook::tree::orderbook::Orderbook as TreeOrderbook;
use orderbook::orderbook::OrderbookTrait;
use orderbook::orderbook::SoA::orderbook::Orderbook as SoAOrderbook;
use orderbook::perf::latency::{LatencyTracker, Percentiles};
use orderbook::perf::{cycles_to_ns, get_cpu_frequency};
use orderbook::types::order::{IdCounter, Order, Side};
use orderbook::types::price::Price;
use orderbook::types::quantity::Quantity;
use rand::prelude::*;
use rand::rngs::StdRng;
use rand::SeedableRng;

const MID_PRICE: u32 = 5_000;

// Burst parameters
const BURST_SIZE: usize = 500; // Orders per burst
const BURST_PRICE_RANGE: u32 = 20; // Tight range during burst (±10 ticks)
const QUIET_SIZE: usize = 50; // Orders during quiet period
const QUIET_PRICE_RANGE: u32 = 2000; // Wide range during quiet (±1000 ticks)
const NUM_CYCLES: usize = 10; // Number of burst-quiet cycles

// ============================================================================
// Scenario 4.1d: Bursty Traffic
// ============================================================================
//
// PURPOSE: Test cache eviction under pressure
//
// WHAT IT SIMULATES:
// Real markets have bursts of activity:
// - Market open/close
// - News announcements
// - Algorithmic trading triggers
// - Large order executions causing cascades
//
// PATTERN:
// [BURST] -> [QUIET] -> [BURST] -> [QUIET] -> ...
//
// During BURST (500 orders):
// - High rate of orders
// - Tight price clustering (±10 ticks from mid)
// - Simulates everyone reacting to same event
//
// During QUIET (50 orders):
// - Low rate of orders
// - Wide price spread (±1000 ticks)
// - Simulates normal market-making activity
//
// WHY IT MATTERS:
// 1. Cache Thrashing: Burst fills L1/L2 cache with hot prices. Quiet period
//    accesses cold prices, evicting the cached data. Next burst must re-warm.
//
// 2. Memory Allocator: Rapid allocations during burst may trigger different
//    allocator code paths than steady allocation.
//
// 3. Latency Variance: We expect higher p99/p50 ratio due to transitions
//    between burst and quiet phases.
//
// 4. Branch Prediction: CPU branch predictors optimize for one pattern, then
//    must re-learn when pattern changes.
//
// EXPECTED RESULTS:
// - Higher latency variance (Max and p99 relative to p50)
// - Implementations with good cache behavior may show bimodal latency
// - Tree may handle transitions better (consistent O(log n))
// ============================================================================

fn main() {
    println!("=== Scenario 4.1d: Bursty Traffic ===\n");

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

    println!("\nPattern: {} cycles of [BURST({}) -> QUIET({})]", NUM_CYCLES, BURST_SIZE, QUIET_SIZE);
    println!("  Burst: {} orders in ±{} ticks", BURST_SIZE, BURST_PRICE_RANGE / 2);
    println!("  Quiet: {} orders in ±{} ticks", QUIET_SIZE, QUIET_PRICE_RANGE / 2);
    println!("  Total: {} orders\n", NUM_CYCLES * (BURST_SIZE + QUIET_SIZE));

    let seed: u64 = 42;

    println!("--- Fixed-Tick Array ---");
    let fixed = scenario_bursty::<FixedTickOrderbook>(seed);
    print_results(&fixed, cpu_ghz);

    println!("\n--- Structure-of-Arrays (SoA) ---");
    let soa = scenario_bursty::<SoAOrderbook>(seed);
    print_results(&soa, cpu_ghz);

    println!("\n--- Hybrid (Hot/Cold) ---");
    let hybrid = scenario_bursty::<HybridOrderbook>(seed);
    print_results(&hybrid, cpu_ghz);

    println!("\n--- Tree-Based ---");
    let tree = scenario_bursty::<TreeOrderbook>(seed);
    print_results(&tree, cpu_ghz);

    println!("\n--- Comparison (p50 latency in cycles) ---");
    print_comparison(&fixed, &soa, &hybrid, &tree);

    println!("\n--- Latency Variance (p99/p50 ratio) ---");
    print_variance(&fixed, &soa, &hybrid, &tree);
}

struct ScenarioResults {
    add_order: Percentiles,
    cancel_order: Percentiles,
    market_order: Percentiles,
}

fn scenario_bursty<O: OrderbookTrait>(seed: u64) -> ScenarioResults {
    let mut rng = StdRng::seed_from_u64(seed);

    let total_orders = NUM_CYCLES * (BURST_SIZE + QUIET_SIZE);
    let mut add_tracker = LatencyTracker::new(total_orders);
    let mut cancel_tracker = LatencyTracker::new(total_orders);
    let mut market_tracker = LatencyTracker::new(200);

    let mut book = O::new();
    let mut id_counter = IdCounter::new();
    let mut order_ids = Vec::with_capacity(total_orders);

    // Phase 1: Add orders in burst-quiet cycles
    for cycle in 0..NUM_CYCLES {
        // BURST phase: tight clustering around mid
        let burst_center = MID_PRICE + (cycle as u32 * 10) % 100; // Slight drift each cycle
        for i in 0..BURST_SIZE {
            let side = if i % 2 == 0 { Side::Bid } else { Side::Ask };
            let offset = rng.random_range(0..BURST_PRICE_RANGE);
            let price_value = (burst_center - BURST_PRICE_RANGE / 2 + offset).clamp(1, 9999);

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

            order_ids.push(order_id);
        }

        // QUIET phase: wide spread, sparse
        for i in 0..QUIET_SIZE {
            let side = if i % 2 == 0 { Side::Bid } else { Side::Ask };
            let offset = rng.random_range(0..QUIET_PRICE_RANGE);
            let price_value = (MID_PRICE - QUIET_PRICE_RANGE / 2 + offset).clamp(1, 9999);

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

            order_ids.push(order_id);
        }
    }

    // Phase 2: Cancel in random order (simulates chaotic cancellation patterns)
    order_ids.shuffle(&mut rng);

    for &order_id in &order_ids {
        cancel_tracker.record(|| {
            book.cancel_order(order_id).expect("Failed to cancel order");
        });
    }

    // Phase 3: Market orders with burst pattern
    let mut book = O::new();
    let mut id_counter = IdCounter::new();

    // Populate with burst-like pattern
    for _ in 0..200 {
        let price_value = MID_PRICE - 10 + rng.random_range(0..20);
        let order = Order::new(
            Price::define(price_value),
            Quantity::define(100),
            Side::Ask,
            &mut id_counter,
        );
        book.add_order(order).expect("Failed to add order");
    }

    for _ in 0..100 {
        market_tracker.record(|| {
            let _ = book.execute_market_order(Side::Bid, Quantity::define(100));
        });
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

fn print_variance(
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

    let ratio = |p99: u64, p50: u64| -> f64 {
        if p50 == 0 { 0.0 } else { p99 as f64 / p50 as f64 }
    };

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
}
