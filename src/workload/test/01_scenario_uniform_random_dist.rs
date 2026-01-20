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
const PRICE_RANGE_MIN: u32 = 1;
const PRICE_RANGE_MAX: u32 = 10_000;


fn main() {
    println!("=== Scenario 4.1a: Uniform Random Distribution ===\n");

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

    println!("\n");

    // Run scenario 4.1a: Uniform Random
    run_scenario_uniform_random(cpu_ghz);
}

// ============================================================================
// Scenario 4.1a: Uniform Random Distribution
// ============================================================================
//
// PURPOSE: Test worst-case TLB/cache behavior
//
// WHAT IT DOES:
// - Generates orders with prices uniformly distributed across the FULL price
//   range (0 to 10,000)
// - Each price has equal probability of being selected
// - Cancellations happen in random order (not FIFO)
//
// WHY IT MATTERS:
// 1. TLB Stress: Wide price spread means accessing many different memory pages.
//    The TLB (Translation Lookaside Buffer) caches virtual-to-physical address
//    translations. Random access causes TLB misses, forcing expensive page
//    table walks.
//
// 2. Cache Misses: L1/L2/L3 caches rely on spatial locality (accessing nearby
//    memory). Uniform random access defeats cache prefetching. Each access
//    likely misses cache, going to slower memory levels.
//
// 3. This is a STRESS TEST: Real markets cluster around mid-price, so uniform
//    random represents worst-case behavior. Implementations that perform well
//    here are robust under pathological conditions.
//
// EXPECTED RESULTS:
// - Tree-based: Should perform relatively well (O(log n) regardless of spread)
// - Fixed-tick array: May suffer from cache misses across wide range
// - Hybrid: Cold zone will be heavily exercised
// ============================================================================

fn run_scenario_uniform_random(cpu_ghz: f64) {
    println!("=== Scenario 4.1a: Uniform Random Distribution ===");
    println!("Random prices across full range [0, 10000]");
    println!("Tests worst-case TLB/cache behavior\n");

    // Use fixed seed for reproducibility
    let seed: u64 = 42;

    println!("--- Fixed-Tick Array ---");
    let fixed = scenario_uniform_random::<FixedTickOrderbook>(seed);
    print_scenario_results(&fixed, cpu_ghz);

    println!("\n--- Structure-of-Arrays (SoA) ---");
    let soa = scenario_uniform_random::<SoAOrderbook>(seed);
    print_scenario_results(&soa, cpu_ghz);

    println!("\n--- Hybrid (Hot/Cold) ---");
    let hybrid = scenario_uniform_random::<HybridOrderbook>(seed);
    print_scenario_results(&hybrid, cpu_ghz);

    println!("\n--- Tree-Based ---");
    let tree = scenario_uniform_random::<TreeOrderbook>(seed);
    print_scenario_results(&tree, cpu_ghz);

    println!("\n--- Comparison (p50 latency in cycles) ---");
    print_comparison_table(&fixed, &soa, &hybrid, &tree);
}

struct ScenarioResults {
    add_order: Percentiles,
    cancel_order: Percentiles,
    market_order: Percentiles,
}

fn scenario_uniform_random<O: OrderbookTrait>(seed: u64) -> ScenarioResults {
    let mut rng = StdRng::seed_from_u64(seed);

    let mut add_tracker = LatencyTracker::new(NUM_SAMPLES);
    let mut cancel_tracker = LatencyTracker::new(NUM_SAMPLES);
    let mut market_tracker = LatencyTracker::new(NUM_SAMPLES);

    // Phase 1: Benchmark add_order with uniform random prices
    let mut book = O::new();
    let mut id_counter = IdCounter::new();
    let mut order_ids = Vec::with_capacity(NUM_SAMPLES);

    for i in 0..NUM_SAMPLES {
        let side = if i % 2 == 0 { Side::Bid } else { Side::Ask };

        // Uniform random price across full range
        let price_value = rng.random_range(PRICE_RANGE_MIN..PRICE_RANGE_MAX);

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

    // Phase 2: Benchmark cancel_order in RANDOM order (not FIFO)
    // This stresses the order lookup mechanism
    order_ids.shuffle(&mut rng);

    for &order_id in &order_ids {
        cancel_tracker.record(|| {
            book.cancel_order(order_id).expect("Failed to cancel order");
        });
    }

    // Phase 3: Benchmark market orders
    // Repopulate the book with uniform random asks
    // Each order has quantity 100, and market orders request exactly 100
    // to avoid partial fill issues
    let mut book = O::new();
    let mut id_counter = IdCounter::new();

    for _ in 0..200 {
        let price_value = rng.random_range(PRICE_RANGE_MIN..PRICE_RANGE_MAX);
        let order = Order::new(
            Price::define(price_value),
            Quantity::define(100),
            Side::Ask,
            &mut id_counter,
        );
        book.add_order(order).expect("Failed to add order");
    }

    // Execute market orders - request exactly 100 to match one order fully
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

fn print_scenario_results(results: &ScenarioResults, cpu_ghz: f64) {
    println!("add_order():");
    print_percentiles(&results.add_order, cpu_ghz);

    println!("\ncancel_order():");
    print_percentiles(&results.cancel_order, cpu_ghz);

    println!("\nexecute_market_order():");
    print_percentiles(&results.market_order, cpu_ghz);
}

fn print_percentiles(p: &Percentiles, cpu_ghz: f64) {
    println!(
        "  p50:  {:>8} cycles  ({:>7.1} ns)",
        p.p50,
        cycles_to_ns(p.p50, cpu_ghz)
    );
    println!(
        "  p99:  {:>8} cycles  ({:>7.1} ns)",
        p.p99,
        cycles_to_ns(p.p99, cpu_ghz)
    );
    println!(
        "  Max:  {:>8} cycles  ({:>7.1} ns)",
        p.max,
        cycles_to_ns(p.max, cpu_ghz)
    );
}

fn print_comparison_table(
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
        "add_order", fixed.add_order.p50, soa.add_order.p50, hybrid.add_order.p50, tree.add_order.p50
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
