use orderbook::analysis::{CsvExporter, ResultRow};
use orderbook::orderbook::OrderbookTrait;
use orderbook::orderbook::SoA::orderbook::Orderbook as SoAOrderbook;
use orderbook::orderbook::fixed_tick::orderbook::Orderbook as FixedTickOrderbook;
use orderbook::orderbook::hybrid::orderbook::Orderbook as HybridOrderbook;
use orderbook::orderbook::tree::orderbook::Orderbook as TreeOrderbook;
use orderbook::perf::latency::{LatencyTracker, Percentiles};
use orderbook::perf::{get_tsc_frequency, tsc_ticks_to_ns};
use orderbook::types::order::{IdCounter, Order, Side};
use orderbook::types::price::Price;
use orderbook::types::quantity::Quantity;
use rand::SeedableRng;
use rand::prelude::*;
use rand::rngs::StdRng;

const TOTAL_SAMPLES: usize = 1_000_000;
const ORDERS_PER_BOOK: usize = 10_000;
const MARKET_ORDERS_PER_BOOK: usize = 200;
const MARKET_SAMPLES_PER_BOOK: usize = 100;
const ORDER_QUANTITY: u32 = 100;
const MARKET_SEED_OFFSET: u64 = 1_000_000;
const PRICE_RANGE_MIN: u32 = 1;
const PRICE_RANGE_MAX: u32 = 10_000;

fn main() {
    println!("=== Scenario 4.1a: Uniform Random Distribution ===\n");

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

    println!("\n");

    // Run scenario 4.1a: Uniform Random
    run_scenario_uniform_random(tsc_ghz);
}

// ============================================================================
// Scenario 4.1a: Uniform Random Distribution
// ============================================================================
//
// PURPOSE: Test worst-case TLB/cache behavior
//
// WHAT IT DOES:
// - Generates orders with prices uniformly distributed across the FULL valid
//   price range [1, 10,000)
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

fn run_scenario_uniform_random(tsc_ghz: f64) {
    println!("=== Scenario 4.1a: Uniform Random Distribution ===");
    println!("Random prices across valid range [1, 10000)");
    println!("Tests worst-case TLB/cache behavior");
    println!("Measurements per operation: {}", TOTAL_SAMPLES);
    println!("Orders per add/cancel book: {}", ORDERS_PER_BOOK);
    println!(
        "Market workload per book: {} asks, {} measured buys\n",
        MARKET_ORDERS_PER_BOOK, MARKET_SAMPLES_PER_BOOK
    );

    // Use fixed seed for reproducibility
    let seed: u64 = 42;

    println!("--- Fixed-Tick Array ---");
    let fixed = scenario_uniform_random::<FixedTickOrderbook>(seed);
    print_scenario_results(&fixed, tsc_ghz);

    println!("\n--- Structure-of-Arrays (SoA) ---");
    let soa = scenario_uniform_random::<SoAOrderbook>(seed);
    print_scenario_results(&soa, tsc_ghz);

    println!("\n--- Hybrid (Hot/Cold) ---");
    let hybrid = scenario_uniform_random::<HybridOrderbook>(seed);
    print_scenario_results(&hybrid, tsc_ghz);

    println!("\n--- Tree-Based ---");
    let tree = scenario_uniform_random::<TreeOrderbook>(seed);
    print_scenario_results(&tree, tsc_ghz);

    println!("\n--- Comparison (p50 latency in cycles) ---");
    print_comparison_table(&fixed, &soa, &hybrid, &tree);

    // Export results to CSV
    let scenario_name = "scenario_uniform";
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
                        tsc_ghz,
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

fn scenario_uniform_random<O: OrderbookTrait>(seed: u64) -> ScenarioResults {
    let mut add_tracker = LatencyTracker::new(TOTAL_SAMPLES);
    let mut cancel_tracker = LatencyTracker::new(TOTAL_SAMPLES);
    let mut market_tracker = LatencyTracker::new(TOTAL_SAMPLES);

    // Phases 1 and 2: repeatedly build and empty a 10,000-order book. Keeping
    // the book size fixed preserves the original sparse uniform workload while
    // collecting enough observations for stable tail percentiles.
    let mut remaining_order_samples = TOTAL_SAMPLES;
    let mut order_batch_index = 0u64;

    while remaining_order_samples > 0 {
        let batch_size = remaining_order_samples.min(ORDERS_PER_BOOK);
        let batch_seed = seed.wrapping_add(order_batch_index);
        let mut rng = StdRng::seed_from_u64(batch_seed);
        let mut book = O::new();
        let mut id_counter = IdCounter::new();
        let mut order_ids = Vec::with_capacity(batch_size);

        // Phase 1: measure additions at uniformly random prices. Side still
        // alternates, producing equal bid and ask counts in every full batch.
        for i in 0..batch_size {
            let side = if i % 2 == 0 { Side::Bid } else { Side::Ask };
            let price_value = rng.random_range(PRICE_RANGE_MIN..PRICE_RANGE_MAX);
            let order = Order::new(
                Price::define(price_value),
                Quantity::define(ORDER_QUANTITY),
                side,
                &mut id_counter,
            );
            let order_id = order.id();

            add_tracker.record(|| {
                book.add_order(order).expect("Failed to add order");
            });
            order_ids.push(order_id);
        }

        // Phase 2: measure cancellation of the same orders in random order.
        order_ids.shuffle(&mut rng);
        for order_id in order_ids {
            cancel_tracker.record(|| {
                book.cancel_order(order_id).expect("Failed to cancel order");
            });
        }

        remaining_order_samples -= batch_size;
        order_batch_index += 1;
    }

    // Phase 3: each independent book starts with 200 uniformly distributed
    // asks. We time 100 buys so every measured order has sufficient liquidity,
    // then create a fresh book to preserve the original workload density.
    let mut remaining_market_samples = TOTAL_SAMPLES;
    let mut market_batch_index = 0u64;

    while remaining_market_samples > 0 {
        let batch_seed = seed
            .wrapping_add(MARKET_SEED_OFFSET)
            .wrapping_add(market_batch_index);
        let mut rng = StdRng::seed_from_u64(batch_seed);
        let mut book = O::new();
        let mut id_counter = IdCounter::new();

        for _ in 0..MARKET_ORDERS_PER_BOOK {
            let price_value = rng.random_range(PRICE_RANGE_MIN..PRICE_RANGE_MAX);
            let order = Order::new(
                Price::define(price_value),
                Quantity::define(ORDER_QUANTITY),
                Side::Ask,
                &mut id_counter,
            );
            book.add_order(order).expect("Failed to add order");
        }

        let batch_size = remaining_market_samples.min(MARKET_SAMPLES_PER_BOOK);
        for _ in 0..batch_size {
            market_tracker.record(|| {
                book.execute_market_order(Side::Bid, Quantity::define(ORDER_QUANTITY))
                    .expect("Failed to execute market order");
            });
        }

        remaining_market_samples -= batch_size;
        market_batch_index += 1;
    }

    debug_assert_eq!(add_tracker.len(), TOTAL_SAMPLES);
    debug_assert_eq!(cancel_tracker.len(), TOTAL_SAMPLES);
    debug_assert_eq!(market_tracker.len(), TOTAL_SAMPLES);

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

fn print_scenario_results(results: &ScenarioResults, tsc_ghz: f64) {
    println!("add_order():");
    print_percentiles(&results.add_order, tsc_ghz);

    println!("\ncancel_order():");
    print_percentiles(&results.cancel_order, tsc_ghz);

    println!("\nexecute_market_order():");
    print_percentiles(&results.market_order, tsc_ghz);
}

fn print_percentiles(p: &Percentiles, tsc_ghz: f64) {
    println!(
        "  p50:  {:>8} cycles  ({:>7.1} ns)",
        p.p50,
        tsc_ticks_to_ns(p.p50, tsc_ghz)
    );
    println!(
        "  p99:  {:>8} cycles  ({:>7.1} ns)",
        p.p99,
        tsc_ticks_to_ns(p.p99, tsc_ghz)
    );
    println!(
        "  Max:  {:>8} cycles  ({:>7.1} ns)",
        p.max,
        tsc_ticks_to_ns(p.max, tsc_ghz)
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
