use orderbook::analysis::{CsvExporter, ResultRow};
use orderbook::orderbook::OrderbookTrait;
use orderbook::orderbook::SoA::orderbook::Orderbook as SoAOrderbook;
/// Scenario 4.1d: Bursty Traffic
///
/// Alternating high-volume/local and low-volume/wide-access phases
/// Tests sensitivity to changing locality and operation mix
///
/// Run with: cargo run --release --example scenario_bursty
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

const MID_PRICE: u32 = 5_000;

// Burst parameters
const BURST_SIZE: usize = 500; // Orders per burst
const BURST_PRICE_RANGE: u32 = 20; // Width 20: offsets [-10, +9]
const QUIET_SIZE: usize = 50; // Orders during quiet period
const QUIET_PRICE_RANGE: u32 = 2000; // Width 2000: offsets [-1000, +999]
const CYCLES_PER_BOOK: usize = 10;
const ORDERS_PER_BOOK: usize = CYCLES_PER_BOOK * (BURST_SIZE + QUIET_SIZE);
const TOTAL_SAMPLES: usize = 1_000_000;
const MARKET_ORDERS_PER_BOOK: usize = 200;
const MARKET_SAMPLES_PER_BOOK: usize = 100;
const ORDER_QUANTITY: u32 = 100;
const MARKET_SEED_OFFSET: u64 = 1_000_000;

// ============================================================================
// Scenario 4.1d: Bursty Traffic
// ============================================================================
//
// PURPOSE: Test sensitivity to alternating locality and volume patterns
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
// - High-volume run of consecutive orders
// - Tight price clustering (offsets -10 through +9 from a drifting center)
// - Simulates everyone reacting to same event
//
// During QUIET (50 orders):
// - Lower-volume run of consecutive orders
// - Wide price spread (offsets -1000 through +999 from mid)
// - Simulates normal market-making activity
//
// This is an access-pattern model. It does not introduce wall-clock delays or
// model network arrival rate between operations.
//
// WHY IT MATTERS:
// 1. Locality Shift: Burst repeatedly accesses hot prices. The quiet phase
//    touches sparse prices, so the next burst may need to re-warm hot data.
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

    println!(
        "\nPattern per add/cancel book: {} cycles of [BURST({}) -> QUIET({})]",
        CYCLES_PER_BOOK, BURST_SIZE, QUIET_SIZE
    );
    println!("  Burst offsets: -10 through +9 ticks from drifting center");
    println!("  Quiet offsets: -1000 through +999 ticks from mid");
    println!("  Orders per add/cancel book: {}", ORDERS_PER_BOOK);
    println!("  Measurements per operation: {}", TOTAL_SAMPLES);
    println!(
        "  Market workload per book: {} asks, {} measured buys",
        MARKET_ORDERS_PER_BOOK, MARKET_SAMPLES_PER_BOOK
    );
    println!("  Timing model: consecutive operations; no artificial sleeps\n");

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

    // Export results to CSV
    let scenario_name = "scenario_bursty";
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

fn scenario_bursty<O: OrderbookTrait>(seed: u64) -> ScenarioResults {
    let mut add_tracker = LatencyTracker::new(TOTAL_SAMPLES);
    let mut cancel_tracker = LatencyTracker::new(TOTAL_SAMPLES);
    let mut market_tracker = LatencyTracker::new(TOTAL_SAMPLES);

    // Phases 1 and 2: repeat the original ten-cycle burst/quiet book. The last
    // book may contain a partial final pattern so the aggregate is exactly one
    // million samples without allowing a single book to grow artificially.
    let mut remaining_order_samples = TOTAL_SAMPLES;
    let mut order_batch_index = 0u64;

    while remaining_order_samples > 0 {
        let batch_size = remaining_order_samples.min(ORDERS_PER_BOOK);
        let batch_seed = seed.wrapping_add(order_batch_index);
        let mut rng = StdRng::seed_from_u64(batch_seed);
        let mut book = O::new();
        let mut id_counter = IdCounter::new();
        let mut order_ids = Vec::with_capacity(batch_size);
        let mut generated = 0usize;

        // Phase 1: alternate high-volume/local and low-volume/wide additions.
        for cycle in 0..CYCLES_PER_BOOK {
            let burst_center = MID_PRICE + (cycle as u32 * 10) % 100;
            let burst_count = (batch_size - generated).min(BURST_SIZE);

            for _ in 0..burst_count {
                let side = if generated % 2 == 0 {
                    Side::Bid
                } else {
                    Side::Ask
                };
                let offset = rng.random_range(0..BURST_PRICE_RANGE);
                let price_value = (burst_center - BURST_PRICE_RANGE / 2 + offset).clamp(1, 9999);
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
                generated += 1;
            }

            if generated == batch_size {
                break;
            }

            let quiet_count = (batch_size - generated).min(QUIET_SIZE);
            for _ in 0..quiet_count {
                let side = if generated % 2 == 0 {
                    Side::Bid
                } else {
                    Side::Ask
                };
                let offset = rng.random_range(0..QUIET_PRICE_RANGE);
                let price_value = (MID_PRICE - QUIET_PRICE_RANGE / 2 + offset).clamp(1, 9999);
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
                generated += 1;
            }

            if generated == batch_size {
                break;
            }
        }

        // Phase 2: cancel the same book in random order.
        order_ids.shuffle(&mut rng);
        for order_id in order_ids {
            cancel_tracker.record(|| {
                book.cancel_order(order_id).expect("Failed to cancel order");
            });
        }

        remaining_order_samples -= batch_size;
        order_batch_index += 1;
    }

    // Phase 3: retain the original tightly clustered market workload and reset
    // every 100 measured buys so every operation has sufficient liquidity.
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
            let price_value = MID_PRICE - 10 + rng.random_range(0..20);
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
        if p50 == 0 {
            0.0
        } else {
            p99 as f64 / p50 as f64
        }
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
