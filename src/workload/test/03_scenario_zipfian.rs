/// Scenario 4.1c: Zipfian Distribution
///
/// Power-law distribution: some prices are very popular, most are rare
/// Tests cache effectiveness on realistic data
///
/// Run with: cargo run --release --example scenario_zipfian
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
use rand_distr::Zipf;

const NUM_SAMPLES: usize = 10_000;
const MID_PRICE: u32 = 5_000;
const NUM_PRICE_LEVELS: f64 = 200.0; // Number of distinct price levels around mid
const ZIPF_EXPONENT: f64 = 1.0; // Classic Zipf distribution (s=1)

// ============================================================================
// Scenario 4.1c: Zipfian Distribution
// ============================================================================
//
// PURPOSE: Test cache effectiveness on realistic data
//
// WHAT IS ZIPFIAN?
// Zipf's law: In many datasets, the k-th most common item appears with
// frequency proportional to 1/k^s. With s=1:
//   - Rank 1: frequency 1.0 (most popular)
//   - Rank 2: frequency 0.5
//   - Rank 3: frequency 0.33
//   - Rank 10: frequency 0.1
//   - Rank 100: frequency 0.01
//
// This means a few prices get LOTS of orders, while most prices are rarely used.
//
// WHY IT'S REALISTIC:
// Real markets exhibit Zipfian-like behavior:
// - Best bid/ask prices get the most activity
// - Prices 1-2 ticks away get moderate activity
// - Prices far from mid are rarely touched
// - "Round" prices (e.g., 5000, 5100) may get more attention
//
// WHAT THIS TESTS:
// 1. Cache Efficiency: Hot prices should stay cached. Cold prices cause misses.
//    Good implementations exploit temporal locality.
//
// 2. Data Structure Adaptation: Some structures naturally handle skewed access
//    better than others. Trees might suffer from unbalanced access patterns.
//
// 3. Memory Allocation: Frequently accessed price levels may trigger different
//    allocation patterns than uniform access.
//
// EXPECTED RESULTS:
// - All implementations should benefit vs uniform random (hot prices cached)
// - Fixed-tick: O(1) regardless of popularity, but cache helps
// - Hybrid: Hot zone covers popular prices well
// - Tree: Hot nodes may cause uneven tree structure
// ============================================================================

fn main() {
    println!("=== Scenario 4.1c: Zipfian Distribution ===\n");

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
    println!("  Price levels: {} (around mid)", NUM_PRICE_LEVELS);
    println!("  Zipf exponent: {} (classic Zipf)", ZIPF_EXPONENT);
    println!("  Samples: {}", NUM_SAMPLES);

    // Show distribution preview
    println!("\nDistribution preview (expected hits per rank):");
    let num_levels = NUM_PRICE_LEVELS as u64;
    let total_weight: f64 = (1..=num_levels).map(|k| 1.0 / (k as f64)).sum();
    for rank in [1u64, 2, 5, 10, 50, 100, 200].iter() {
        if *rank <= num_levels {
            let prob = (1.0 / (*rank as f64)) / total_weight;
            let expected_hits = (prob * NUM_SAMPLES as f64) as u32;
            println!("  Rank {:>3}: ~{:>4} orders ({:.1}%)", rank, expected_hits, prob * 100.0);
        }
    }
    println!();

    let seed: u64 = 42;

    println!("--- Fixed-Tick Array ---");
    let fixed = scenario_zipfian::<FixedTickOrderbook>(seed);
    print_results(&fixed, cpu_ghz);

    println!("\n--- Structure-of-Arrays (SoA) ---");
    let soa = scenario_zipfian::<SoAOrderbook>(seed);
    print_results(&soa, cpu_ghz);

    println!("\n--- Hybrid (Hot/Cold) ---");
    let hybrid = scenario_zipfian::<HybridOrderbook>(seed);
    print_results(&hybrid, cpu_ghz);

    println!("\n--- Tree-Based ---");
    let tree = scenario_zipfian::<TreeOrderbook>(seed);
    print_results(&tree, cpu_ghz);

    println!("\n--- Comparison (p50 latency in cycles) ---");
    print_comparison(&fixed, &soa, &hybrid, &tree);
}

struct ScenarioResults {
    add_order: Percentiles,
    cancel_order: Percentiles,
    market_order: Percentiles,
}

/// Generate a price following Zipfian distribution around mid-price
/// Rank 1 = mid price, Rank 2 = mid±1, etc.
fn generate_zipfian_price(rng: &mut impl Rng, zipf: &Zipf<f64>) -> u32 {
    // Sample a rank (1 to NUM_PRICE_LEVELS)
    let rank = zipf.sample(rng) as u32;

    // Convert rank to price offset from mid
    // Rank 1 -> offset 0 (mid price)
    // Rank 2 -> offset 1 (mid + 1)
    // Rank 3 -> offset -1 (mid - 1)
    // Rank 4 -> offset 2 (mid + 2)
    // etc. (alternating sides)
    let offset = if rank == 1 {
        0i32
    } else {
        let half = ((rank - 1) / 2 + 1) as i32;
        if rank % 2 == 0 {
            half
        } else {
            -half
        }
    };

    let price = (MID_PRICE as i32 + offset).clamp(1, 9999) as u32;
    price
}

fn scenario_zipfian<O: OrderbookTrait>(seed: u64) -> ScenarioResults {
    let mut rng = StdRng::seed_from_u64(seed);
    let zipf = Zipf::new(NUM_PRICE_LEVELS, ZIPF_EXPONENT).expect("Invalid Zipf parameters");

    let mut add_tracker = LatencyTracker::new(NUM_SAMPLES);
    let mut cancel_tracker = LatencyTracker::new(NUM_SAMPLES);
    let mut market_tracker = LatencyTracker::new(NUM_SAMPLES);

    // Phase 1: Benchmark add_order with Zipfian prices
    let mut book = O::new();
    let mut id_counter = IdCounter::new();
    let mut order_ids = Vec::with_capacity(NUM_SAMPLES);

    for i in 0..NUM_SAMPLES {
        let side = if i % 2 == 0 { Side::Bid } else { Side::Ask };
        let price_value = generate_zipfian_price(&mut rng, &zipf);

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

    // Phase 3: Market orders on Zipfian-populated book
    let mut book = O::new();
    let mut id_counter = IdCounter::new();

    for _ in 0..200 {
        let price_value = generate_zipfian_price(&mut rng, &zipf);
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
