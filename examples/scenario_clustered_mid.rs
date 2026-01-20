/// Scenario 4.1b: Clustered Around Mid Distribution
///
/// 90% of orders within ±10 ticks of mid-price
/// Tests hot-path optimization and cache locality
///
/// Run with: cargo run --release --example scenario_clustered_mid
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

const NUM_SAMPLES: usize = 10_000;
const MID_PRICE: u32 = 5_000;
const CLUSTER_RADIUS: u32 = 10; // ±10 ticks from mid
const CLUSTER_PROBABILITY: f64 = 0.90; // 90% within cluster

// ============================================================================
// Scenario 4.1b: Clustered Around Mid Distribution
// ============================================================================
//
// PURPOSE: Test hot-path optimization and cache locality
//
// WHAT IT DOES:
// - 90% of orders fall within ±10 ticks of mid-price (4990-5010)
// - 10% of orders scattered across full price range
// - This mimics real market behavior where most activity is near the spread
//
// WHY IT MATTERS:
// 1. Cache Locality: Tight price clustering means the same memory pages are
//    accessed repeatedly. CPU caches (L1/L2/L3) stay warm. Prefetching works.
//
// 2. Hot Zone Test: Hybrid orderbook has a "hot zone" around mid-price using
//    a fast array. This scenario directly tests that optimization path.
//
// 3. Realistic Baseline: Real markets cluster activity near the current price.
//    Traders place orders near the spread, not at extreme prices. This is the
//    "normal" workload that orderbooks should optimize for.
//
// 4. Branch Prediction: Repeated similar access patterns help CPU branch
//    predictors. The hot path becomes very predictable.
//
// EXPECTED RESULTS:
// - Hybrid: Should EXCEL - hot zone is designed exactly for this pattern
// - Fixed-Tick: Good - benefits from cache locality on small region
// - SoA: Good - cache-friendly layout benefits from locality
// - Tree: May show overhead from tree balancing, but still benefits from
//   locality in node traversal
// ============================================================================

fn main() {
    println!("=== Scenario 4.1b: Clustered Around Mid ===\n");

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

    println!("\nParameters:");
    println!("  Mid price: {}", MID_PRICE);
    println!("  Cluster radius: ±{} ticks", CLUSTER_RADIUS);
    println!("  Cluster probability: {}%", CLUSTER_PROBABILITY * 100.0);
    println!("  Samples: {}\n", NUM_SAMPLES);

    let seed: u64 = 42;

    println!("--- Fixed-Tick Array ---");
    let fixed = scenario_clustered_mid::<FixedTickOrderbook>(seed);
    print_results(&fixed, cpu_ghz);

    println!("\n--- Structure-of-Arrays (SoA) ---");
    let soa = scenario_clustered_mid::<SoAOrderbook>(seed);
    print_results(&soa, cpu_ghz);

    println!("\n--- Hybrid (Hot/Cold) ---");
    let hybrid = scenario_clustered_mid::<HybridOrderbook>(seed);
    print_results(&hybrid, cpu_ghz);

    println!("\n--- Tree-Based ---");
    let tree = scenario_clustered_mid::<TreeOrderbook>(seed);
    print_results(&tree, cpu_ghz);

    println!("\n--- Comparison (p50 latency in cycles) ---");
    print_comparison(&fixed, &soa, &hybrid, &tree);
}

struct ScenarioResults {
    add_order: Percentiles,
    cancel_order: Percentiles,
    market_order: Percentiles,
}

/// Generate a price following the clustered distribution:
/// - 90% chance: within ±10 ticks of mid (4990-5010)
/// - 10% chance: anywhere in full range (1-9999)
fn generate_clustered_price(rng: &mut impl Rng) -> u32 {
    if rng.random_bool(CLUSTER_PROBABILITY) {
        // Clustered: mid ± radius
        let offset = rng.random_range(0..=CLUSTER_RADIUS * 2);
        (MID_PRICE - CLUSTER_RADIUS + offset).clamp(1, 9999)
    } else {
        // Scattered: full range
        rng.random_range(1..10000)
    }
}

fn scenario_clustered_mid<O: OrderbookTrait>(seed: u64) -> ScenarioResults {
    let mut rng = StdRng::seed_from_u64(seed);

    let mut add_tracker = LatencyTracker::new(NUM_SAMPLES);
    let mut cancel_tracker = LatencyTracker::new(NUM_SAMPLES);
    let mut market_tracker = LatencyTracker::new(NUM_SAMPLES);

    // Phase 1: Benchmark add_order with clustered prices
    let mut book = O::new();
    let mut id_counter = IdCounter::new();
    let mut order_ids = Vec::with_capacity(NUM_SAMPLES);

    for i in 0..NUM_SAMPLES {
        let side = if i % 2 == 0 { Side::Bid } else { Side::Ask };
        let price_value = generate_clustered_price(&mut rng);

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

    // Phase 2: Cancel in random order
    order_ids.shuffle(&mut rng);

    for &order_id in &order_ids {
        cancel_tracker.record(|| {
            book.cancel_order(order_id).expect("Failed to cancel order");
        });
    }

    // Phase 3: Market orders on clustered book
    let mut book = O::new();
    let mut id_counter = IdCounter::new();

    // Populate with clustered asks
    for _ in 0..200 {
        let price_value = generate_clustered_price(&mut rng);
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
