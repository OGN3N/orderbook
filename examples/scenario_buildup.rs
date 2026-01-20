/// Scenario 4.2c: Order Book Build-Up
///
/// Starting from empty book, measure latency as book fills
/// Tests allocation patterns and cold-start behavior
///
/// Run with: cargo run --release --example scenario_buildup
use orderbook::orderbook::fixed_tick::orderbook::Orderbook as FixedTickOrderbook;
use orderbook::orderbook::hybrid::orderbook::Orderbook as HybridOrderbook;
use orderbook::orderbook::tree::orderbook::Orderbook as TreeOrderbook;
use orderbook::orderbook::OrderbookTrait;
use orderbook::orderbook::SoA::orderbook::Orderbook as SoAOrderbook;
use orderbook::perf::latency::LatencyTracker;
use orderbook::perf::{cycles_to_ns, get_cpu_frequency};
use orderbook::types::order::{IdCounter, Order, Side};
use orderbook::types::price::Price;
use orderbook::types::quantity::Quantity;
use rand::prelude::*;
use rand::rngs::StdRng;
use rand::SeedableRng;

const MID_PRICE: u32 = 5_000;
const PRICE_SPREAD: u32 = 100; // Orders within ±50 ticks
const TOTAL_ORDERS: usize = 10_000;

// Measure at these fill percentages
const MEASUREMENT_POINTS: [usize; 5] = [0, 25, 50, 75, 100];
const ORDERS_PER_MEASUREMENT: usize = 500; // Sample size at each point

// ============================================================================
// Scenario 4.2c: Order Book Build-Up
// ============================================================================
//
// PURPOSE: Test allocation patterns and cold-start behavior
//
// WHAT THIS SIMULATES:
// - Market open (9:30 AM equity markets)
// - New instrument listing (IPO, new futures contract)
// - System restart after maintenance or outage
// - Recovery from crash
//
// HOW IT WORKS:
// 1. Start with empty orderbook
// 2. Add orders sequentially, measuring latency
// 3. Report latency at different fill levels (0%, 25%, 50%, 75%, 100%)
//
// WHAT THIS TESTS:
// 1. Cold Start Performance: First orders hit cold caches and may trigger
//    initial memory allocation. Expect higher latency early.
//
// 2. Allocation Patterns:
//    - Fixed-tick array: Pre-allocated, should be consistent
//    - Tree: Allocates nodes on demand, may show allocation spikes
//    - Hash maps: May rehash as they grow
//
// 3. Warm-up Effects: As book fills, caches warm up. Later operations
//    may be faster due to better cache locality.
//
// 4. Data Structure Growth:
//    - Trees may rebalance as they grow
//    - Dynamic arrays may reallocate and copy
//    - Hash maps rehash at load factor thresholds
//
// EXPECTED RESULTS:
// - Early orders (0-25%): Higher latency, cold caches, allocations
// - Mid orders (25-75%): Stabilizing latency, warm caches
// - Late orders (75-100%): May see pressure from full data structures
// - Fixed-tick: Most consistent (pre-allocated array)
// - Tree: May show spikes during rebalancing
// ============================================================================

fn main() {
    println!("=== Scenario 4.2c: Order Book Build-Up ===\n");

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

    println!("\nBuild-up Parameters:");
    println!("  Total orders: {}", TOTAL_ORDERS);
    println!("  Price spread: ±{} ticks around mid", PRICE_SPREAD / 2);
    println!("  Measurement points: {:?}%", MEASUREMENT_POINTS);
    println!("  Samples per point: {}\n", ORDERS_PER_MEASUREMENT);

    let seed: u64 = 42;

    println!("--- Fixed-Tick Array ---");
    let fixed = run_buildup_benchmark::<FixedTickOrderbook>(seed);
    print_results(&fixed, cpu_ghz);

    println!("\n--- Structure-of-Arrays (SoA) ---");
    let soa = run_buildup_benchmark::<SoAOrderbook>(seed);
    print_results(&soa, cpu_ghz);

    println!("\n--- Hybrid (Hot/Cold) ---");
    let hybrid = run_buildup_benchmark::<HybridOrderbook>(seed);
    print_results(&hybrid, cpu_ghz);

    println!("\n--- Tree-Based ---");
    let tree = run_buildup_benchmark::<TreeOrderbook>(seed);
    print_results(&tree, cpu_ghz);

    println!("\n--- Comparison: p50 latency by fill level (cycles) ---");
    print_comparison(&fixed, &soa, &hybrid, &tree);

    println!("\n--- Warm-up Analysis (latency change from 0% to 100%) ---");
    print_warmup_analysis(&fixed, &soa, &hybrid, &tree, cpu_ghz);
}

struct BuildupResults {
    // Latency at each measurement point (0%, 25%, 50%, 75%, 100%)
    p50_at_level: [u64; 5],
    p99_at_level: [u64; 5],
    max_at_level: [u64; 5],
}

fn run_buildup_benchmark<O: OrderbookTrait>(seed: u64) -> BuildupResults {
    let mut rng = StdRng::seed_from_u64(seed);

    let mut book = O::new();
    let mut id_counter = IdCounter::new();

    let mut p50_at_level = [0u64; 5];
    let mut p99_at_level = [0u64; 5];
    let mut max_at_level = [0u64; 5];

    let mut orders_added = 0usize;
    let mut measurement_idx = 0usize;

    // Pre-generate all prices for consistency across implementations
    let prices: Vec<u32> = (0..TOTAL_ORDERS)
        .map(|_| {
            let offset = rng.random_range(0..PRICE_SPREAD);
            (MID_PRICE - PRICE_SPREAD / 2 + offset).clamp(1, 9999)
        })
        .collect();

    // Reset RNG for consistent side selection
    let mut rng = StdRng::seed_from_u64(seed + 1);

    for (i, &price_value) in prices.iter().enumerate() {
        let side = if rng.random_bool(0.5) { Side::Bid } else { Side::Ask };

        // Check if we're at a measurement point
        let current_pct = (orders_added * 100) / TOTAL_ORDERS;
        let target_pct = MEASUREMENT_POINTS[measurement_idx];

        if current_pct >= target_pct && measurement_idx < MEASUREMENT_POINTS.len() {
            // Measure the next ORDERS_PER_MEASUREMENT orders
            let mut tracker = LatencyTracker::new(ORDERS_PER_MEASUREMENT);

            let measure_end = (i + ORDERS_PER_MEASUREMENT).min(TOTAL_ORDERS);
            for j in i..measure_end {
                let measure_side = if rng.random_bool(0.5) { Side::Bid } else { Side::Ask };
                let order = Order::new(
                    Price::define(prices[j]),
                    Quantity::define(100),
                    measure_side,
                    &mut id_counter,
                );

                tracker.record(|| {
                    book.add_order(order).expect("Failed to add order");
                });

                orders_added += 1;
            }

            if let Some(p) = tracker.precentiles() {
                p50_at_level[measurement_idx] = p.p50;
                p99_at_level[measurement_idx] = p.p99;
                max_at_level[measurement_idx] = p.max;
            }

            measurement_idx += 1;
            if measurement_idx >= MEASUREMENT_POINTS.len() {
                break;
            }
        } else {
            // Just add the order without measuring
            let order = Order::new(
                Price::define(price_value),
                Quantity::define(100),
                side,
                &mut id_counter,
            );
            book.add_order(order).expect("Failed to add order");
            orders_added += 1;
        }
    }

    BuildupResults {
        p50_at_level,
        p99_at_level,
        max_at_level,
    }
}

fn print_results(results: &BuildupResults, cpu_ghz: f64) {
    println!("add_order() latency by fill level:");
    println!(
        "{:<12} | {:>10} | {:>10} | {:>10}",
        "Fill %", "p50 (cy)", "p99 (cy)", "Max (cy)"
    );
    println!("{:-<50}", "");

    for (i, &pct) in MEASUREMENT_POINTS.iter().enumerate() {
        println!(
            "{:<12} | {:>10} | {:>10} | {:>10}",
            format!("{}%", pct),
            results.p50_at_level[i],
            results.p99_at_level[i],
            results.max_at_level[i]
        );
    }

    println!("\nIn nanoseconds (p50):");
    for (i, &pct) in MEASUREMENT_POINTS.iter().enumerate() {
        println!(
            "  {:>3}%: {:>7.1} ns",
            pct,
            cycles_to_ns(results.p50_at_level[i], cpu_ghz)
        );
    }
}

fn print_comparison(
    fixed: &BuildupResults,
    soa: &BuildupResults,
    hybrid: &BuildupResults,
    tree: &BuildupResults,
) {
    println!(
        "{:<8} | {:>12} | {:>12} | {:>12} | {:>12}",
        "Fill %", "Fixed-Tick", "SoA", "Hybrid", "Tree"
    );
    println!("{:-<70}", "");

    for (i, &pct) in MEASUREMENT_POINTS.iter().enumerate() {
        println!(
            "{:<8} | {:>10} cy | {:>10} cy | {:>10} cy | {:>10} cy",
            format!("{}%", pct),
            fixed.p50_at_level[i],
            soa.p50_at_level[i],
            hybrid.p50_at_level[i],
            tree.p50_at_level[i]
        );
    }
}

fn print_warmup_analysis(
    fixed: &BuildupResults,
    soa: &BuildupResults,
    hybrid: &BuildupResults,
    tree: &BuildupResults,
    _cpu_ghz: f64,
) {
    // Compare 0% (cold) to 100% (warm)
    let cold_idx = 0;
    let warm_idx = MEASUREMENT_POINTS.len() - 1;

    println!(
        "{:<15} | {:>12} | {:>12} | {:>12} | {:>12}",
        "Metric", "Fixed-Tick", "SoA", "Hybrid", "Tree"
    );
    println!("{:-<75}", "");

    println!(
        "{:<15} | {:>10} cy | {:>10} cy | {:>10} cy | {:>10} cy",
        "Cold (0%)",
        fixed.p50_at_level[cold_idx],
        soa.p50_at_level[cold_idx],
        hybrid.p50_at_level[cold_idx],
        tree.p50_at_level[cold_idx]
    );
    println!(
        "{:<15} | {:>10} cy | {:>10} cy | {:>10} cy | {:>10} cy",
        "Warm (100%)",
        fixed.p50_at_level[warm_idx],
        soa.p50_at_level[warm_idx],
        hybrid.p50_at_level[warm_idx],
        tree.p50_at_level[warm_idx]
    );

    // Calculate change
    let calc_change = |cold: u64, warm: u64| -> f64 {
        if cold == 0 {
            0.0
        } else {
            ((warm as f64 - cold as f64) / cold as f64) * 100.0
        }
    };

    println!(
        "{:<15} | {:>10.1}% | {:>10.1}% | {:>10.1}% | {:>10.1}%",
        "Change",
        calc_change(fixed.p50_at_level[cold_idx], fixed.p50_at_level[warm_idx]),
        calc_change(soa.p50_at_level[cold_idx], soa.p50_at_level[warm_idx]),
        calc_change(hybrid.p50_at_level[cold_idx], hybrid.p50_at_level[warm_idx]),
        calc_change(tree.p50_at_level[cold_idx], tree.p50_at_level[warm_idx]),
    );

    println!("\nInterpretation:");
    println!("  - Negative change = faster when warm (good cache behavior)");
    println!("  - Positive change = slower when full (data structure pressure)");
    println!("  - Near zero = consistent performance regardless of fill level");
}
