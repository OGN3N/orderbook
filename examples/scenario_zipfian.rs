use orderbook::analysis::{CsvExporter, ResultRow};
use orderbook::orderbook::OrderbookTrait;
use orderbook::orderbook::SoA::orderbook::Orderbook as SoAOrderbook;
/// Scenario 4.1c: Zipfian Distribution
///
/// Power-law distribution: some prices are very popular, most are rare
/// Tests cache effectiveness under a highly skewed synthetic workload
///
/// Run with: cargo run --release --example scenario_zipfian
use orderbook::orderbook::fixed_tick::orderbook::Orderbook as FixedTickOrderbook;
use orderbook::orderbook::hybrid::orderbook::Orderbook as HybridOrderbook;
use orderbook::orderbook::tree::orderbook::Orderbook as TreeOrderbook;
use orderbook::perf::latency::{LatencyTracker, Percentiles};
use orderbook::perf::{cycles_to_ns, get_cpu_frequency};
use orderbook::types::order::{IdCounter, Order, Side};
use orderbook::types::price::Price;
use orderbook::types::quantity::Quantity;
use rand::SeedableRng;
use rand::prelude::*;
use rand::rngs::StdRng;
use rand_distr::Zipf;

const TOTAL_SAMPLES: usize = 1_000_000;
const ORDERS_PER_BOOK: usize = 10_000;
const MARKET_ORDERS_PER_BOOK: usize = 200;
const MARKET_SAMPLES_PER_BOOK: usize = 100;
const ORDER_QUANTITY: u32 = 100;
const MARKET_SEED_OFFSET: u64 = 1_000_000;
const MID_PRICE: u32 = 5_000;
const NUM_PRICE_LEVELS: f64 = 200.0; // Number of distinct price levels around mid
const ZIPF_EXPONENT: f64 = 1.0; // Classic Zipf distribution (s=1)

// ============================================================================
// Scenario 4.1c: Zipfian Distribution
// ============================================================================
//
// PURPOSE: Test cache effectiveness under a highly skewed synthetic workload
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
// WHY IT IS USEFUL:
// Real markets often exhibit highly skewed activity:
// - Prices near the current market get the most activity
// - Prices 1-2 ticks away get moderate activity
// - Prices far from mid are rarely touched
// This generator is a controlled synthetic approximation, not a calibrated
// model of a particular real market.
//
// WHAT THIS TESTS:
// 1. Cache Efficiency: Hot prices should stay cached. Cold prices cause misses.
//    Good implementations exploit temporal locality.
//
// 2. Data Structure Adaptation: Some structures naturally handle skewed access
//    better than others. BTreeMap remains balanced but retains lookup overhead.
//
// 3. Memory Allocation: Frequently accessed price levels may trigger different
//    allocation patterns than uniform access.
//
// EXPECTED RESULTS:
// - All implementations should benefit vs uniform random (hot prices cached)
// - Fixed-tick: O(1) regardless of popularity, but cache helps
// - Hybrid: Hot zone covers popular prices well
// - Tree: Stays balanced and benefits from cached hot nodes, but retains tree
//   lookup overhead
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
    println!("  Measurements per operation: {}", TOTAL_SAMPLES);
    println!("  Orders per add/cancel book: {}", ORDERS_PER_BOOK);
    println!(
        "  Market workload per book: {} asks, {} measured buys",
        MARKET_ORDERS_PER_BOOK, MARKET_SAMPLES_PER_BOOK
    );

    // Show distribution preview
    println!("\nDistribution preview (expected hits per 10,000-order book):");
    let num_levels = NUM_PRICE_LEVELS as u64;
    let total_weight: f64 = (1..=num_levels).map(|k| 1.0 / (k as f64)).sum();
    for rank in [1u64, 2, 5, 10, 50, 100, 200].iter() {
        if *rank <= num_levels {
            let prob = (1.0 / (*rank as f64)) / total_weight;
            let expected_hits = (prob * ORDERS_PER_BOOK as f64) as u32;
            let price = zipf_rank_to_price(*rank as u32);
            println!(
                "  Rank {:>3} → price {}: ~{:>4} orders ({:.2}%)",
                rank,
                price,
                expected_hits,
                prob * 100.0
            );
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

    // Export results to CSV
    let scenario_name = "scenario_zipfian";
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

/// Generate a price following a Zipfian distribution around mid-price.
/// Rank 1 is mid; higher ranks alternate above and below mid by distance.
fn generate_zipfian_price(rng: &mut impl Rng, zipf: &Zipf<f64>) -> u32 {
    // Sample a rank (1 to NUM_PRICE_LEVELS)
    let rank = zipf.sample(rng) as u32;
    zipf_rank_to_price(rank)
}

fn zipf_rank_to_price(rank: u32) -> u32 {
    debug_assert!((1..=NUM_PRICE_LEVELS as u32).contains(&rank));

    // Convert rank to price offset from mid
    // Rank 1 -> offset 0 (mid price)
    // Rank 2 -> offset 1 (mid + 1)
    // Rank 3 -> offset -1 (mid - 1)
    // Rank 4 -> offset 2 (mid + 2)
    // etc. (alternating sides)
    let offset = if rank == 1 {
        0i32
    } else {
        let distance = (rank / 2) as i32;
        if rank % 2 == 0 { distance } else { -distance }
    };

    (MID_PRICE as i32 + offset).clamp(1, 9999) as u32
}

fn scenario_zipfian<O: OrderbookTrait>(seed: u64) -> ScenarioResults {
    let zipf = Zipf::new(NUM_PRICE_LEVELS, ZIPF_EXPONENT).expect("Invalid Zipf parameters");

    let mut add_tracker = LatencyTracker::new(TOTAL_SAMPLES);
    let mut cancel_tracker = LatencyTracker::new(TOTAL_SAMPLES);
    let mut market_tracker = LatencyTracker::new(TOTAL_SAMPLES);

    // Phases 1 and 2: repeatedly build and empty a 10,000-order book. Fixed
    // batch size preserves the original Zipfian density while collecting one
    // million observations for stable tail percentiles.
    let mut remaining_order_samples = TOTAL_SAMPLES;
    let mut order_batch_index = 0u64;

    while remaining_order_samples > 0 {
        let batch_size = remaining_order_samples.min(ORDERS_PER_BOOK);
        let batch_seed = seed.wrapping_add(order_batch_index);
        let mut rng = StdRng::seed_from_u64(batch_seed);
        let mut book = O::new();
        let mut id_counter = IdCounter::new();
        let mut order_ids = Vec::with_capacity(batch_size);

        // Phase 1: measure additions using the Zipfian price distribution.
        for i in 0..batch_size {
            let side = if i % 2 == 0 { Side::Bid } else { Side::Ask };
            let price_value = generate_zipfian_price(&mut rng, &zipf);
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

    // Phase 3: repeatedly populate an independent Zipfian ask book, measure
    // 100 market buys, and reset before its density changes materially.
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
            let price_value = generate_zipfian_price(&mut rng, &zipf);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn zipf_ranks_alternate_around_mid_without_skipping_ticks() {
        assert_eq!(zipf_rank_to_price(1), 5_000);
        assert_eq!(zipf_rank_to_price(2), 5_001);
        assert_eq!(zipf_rank_to_price(3), 4_999);
        assert_eq!(zipf_rank_to_price(4), 5_002);
        assert_eq!(zipf_rank_to_price(5), 4_998);
    }

    #[test]
    fn all_zipf_ranks_map_to_distinct_valid_prices() {
        let prices: HashSet<u32> = (1..=NUM_PRICE_LEVELS as u32)
            .map(zipf_rank_to_price)
            .collect();

        assert_eq!(prices.len(), NUM_PRICE_LEVELS as usize);
        assert!(prices.iter().all(|price| (1..10_000).contains(price)));
        assert_eq!(zipf_rank_to_price(199), 4_901);
        assert_eq!(zipf_rank_to_price(200), 5_100);
    }
}
