/// Scenario 4.2b: Market Order Sweeps
///
/// Large market orders walking the book across multiple price levels
/// Tests depth traversal performance
///
/// Run with: cargo run --release --example scenario_sweep
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

const MID_PRICE: u32 = 5_000;

// Book setup parameters
const NUM_PRICE_LEVELS: u32 = 100; // 100 price levels on each side
const ORDERS_PER_LEVEL: u32 = 1; // 1 order per level for simplicity
const QTY_PER_ORDER: u32 = 100; // 100 qty per order

// Sweep parameters - different sweep sizes to test
const SMALL_SWEEP_LEVELS: u32 = 5; // Sweeps 5 levels
const MEDIUM_SWEEP_LEVELS: u32 = 20; // Sweeps 20 levels
const LARGE_SWEEP_LEVELS: u32 = 50; // Sweeps 50 levels

const NUM_SWEEPS: usize = 20; // Number of sweeps to measure per size

// ============================================================================
// Scenario 4.2b: Market Order Sweeps
// ============================================================================
//
// PURPOSE: Test depth traversal performance
//
// WHAT THIS SIMULATES:
// Large market orders that consume liquidity across multiple price levels:
// - Institutional orders (pension funds, ETFs rebalancing)
// - Stop-loss cascades (price drops trigger stops, causing more drops)
// - Flash crash dynamics
// - Aggressive HFT strategies taking liquidity
//
// HOW IT WORKS:
// 1. Populate book with 100 price levels, 100 qty each = 10,000 total liquidity
// 2. Execute market orders of varying sizes:
//    - Small: 500 qty (sweeps ~5 levels)
//    - Medium: 2000 qty (sweeps ~20 levels)
//    - Large: 5000 qty (sweeps ~50 levels)
// 3. Measure how latency scales with sweep depth
//
// WHAT THIS TESTS:
// 1. Depth Traversal: How efficiently can we iterate through price levels?
//    - Tree: O(1) per level via in-order traversal
//    - Array: O(1) per level, but may scan empty slots
//
// 2. Partial Fill Handling: Each level may only partially fill the order.
//    Tests the fill loop efficiency.
//
// 3. Book Depletion: As we sweep, levels become empty. Tests cleanup.
//
// 4. Scaling: Does latency scale linearly with sweep depth? Or worse?
//
// EXPECTED RESULTS:
// - Tree should scale well (O(levels) traversal)
// - Hybrid should be fast for sweeps within hot zone
// - Fixed-tick may struggle if sweep crosses many empty slots
// - Latency should roughly scale with sweep size
// ============================================================================

fn main() {
    println!("=== Scenario 4.2b: Market Order Sweeps ===\n");

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

    println!("\nBook Setup:");
    println!("  Price levels per side: {}", NUM_PRICE_LEVELS);
    println!("  Orders per level: {}", ORDERS_PER_LEVEL);
    println!("  Qty per order: {}", QTY_PER_ORDER);
    println!(
        "  Total liquidity per side: {}",
        NUM_PRICE_LEVELS * ORDERS_PER_LEVEL * QTY_PER_ORDER
    );

    println!("\nSweep Sizes:");
    println!(
        "  Small:  {} qty ({} levels)",
        SMALL_SWEEP_LEVELS * QTY_PER_ORDER,
        SMALL_SWEEP_LEVELS
    );
    println!(
        "  Medium: {} qty ({} levels)",
        MEDIUM_SWEEP_LEVELS * QTY_PER_ORDER,
        MEDIUM_SWEEP_LEVELS
    );
    println!(
        "  Large:  {} qty ({} levels)",
        LARGE_SWEEP_LEVELS * QTY_PER_ORDER,
        LARGE_SWEEP_LEVELS
    );
    println!("  Sweeps per size: {}\n", NUM_SWEEPS);

    println!("--- Fixed-Tick Array ---");
    let fixed = run_sweep_benchmark::<FixedTickOrderbook>();
    print_results(&fixed, cpu_ghz);

    println!("\n--- Structure-of-Arrays (SoA) ---");
    let soa = run_sweep_benchmark::<SoAOrderbook>();
    print_results(&soa, cpu_ghz);

    println!("\n--- Hybrid (Hot/Cold) ---");
    let hybrid = run_sweep_benchmark::<HybridOrderbook>();
    print_results(&hybrid, cpu_ghz);

    println!("\n--- Tree-Based ---");
    let tree = run_sweep_benchmark::<TreeOrderbook>();
    print_results(&tree, cpu_ghz);

    println!("\n--- Comparison: p50 latency by sweep size ---");
    print_comparison(&fixed, &soa, &hybrid, &tree, cpu_ghz);

    println!("\n--- Scaling Analysis (latency per level) ---");
    print_scaling(&fixed, &soa, &hybrid, &tree, cpu_ghz);
}

struct SweepResults {
    small_sweep: Percentiles,
    medium_sweep: Percentiles,
    large_sweep: Percentiles,
}

fn populate_book<O: OrderbookTrait>(book: &mut O, id_counter: &mut IdCounter, side: Side) {
    // Populate one side of the book with orders at consecutive price levels
    for i in 0..NUM_PRICE_LEVELS {
        let price_value = if side == Side::Ask {
            // Asks above mid: 5001, 5002, 5003, ...
            MID_PRICE + 1 + i
        } else {
            // Bids below mid: 4999, 4998, 4997, ...
            MID_PRICE - 1 - i
        };

        for _ in 0..ORDERS_PER_LEVEL {
            let order = Order::new(
                Price::define(price_value),
                Quantity::define(QTY_PER_ORDER),
                side,
                id_counter,
            );
            book.add_order(order).expect("Failed to add order");
        }
    }
}

fn run_sweep_benchmark<O: OrderbookTrait>() -> SweepResults {
    let mut small_tracker = LatencyTracker::new(NUM_SWEEPS);
    let mut medium_tracker = LatencyTracker::new(NUM_SWEEPS);
    let mut large_tracker = LatencyTracker::new(NUM_SWEEPS);

    // Small sweeps
    for _ in 0..NUM_SWEEPS {
        let mut book = O::new();
        let mut id_counter = IdCounter::new();
        populate_book(&mut book, &mut id_counter, Side::Ask);

        let sweep_qty = SMALL_SWEEP_LEVELS * QTY_PER_ORDER;
        small_tracker.record(|| {
            book.execute_market_order(Side::Bid, Quantity::define(sweep_qty))
                .expect("Failed to execute sweep");
        });
    }

    // Medium sweeps
    for _ in 0..NUM_SWEEPS {
        let mut book = O::new();
        let mut id_counter = IdCounter::new();
        populate_book(&mut book, &mut id_counter, Side::Ask);

        let sweep_qty = MEDIUM_SWEEP_LEVELS * QTY_PER_ORDER;
        medium_tracker.record(|| {
            book.execute_market_order(Side::Bid, Quantity::define(sweep_qty))
                .expect("Failed to execute sweep");
        });
    }

    // Large sweeps
    for _ in 0..NUM_SWEEPS {
        let mut book = O::new();
        let mut id_counter = IdCounter::new();
        populate_book(&mut book, &mut id_counter, Side::Ask);

        let sweep_qty = LARGE_SWEEP_LEVELS * QTY_PER_ORDER;
        large_tracker.record(|| {
            book.execute_market_order(Side::Bid, Quantity::define(sweep_qty))
                .expect("Failed to execute sweep");
        });
    }

    SweepResults {
        small_sweep: small_tracker.precentiles().expect("No small sweep samples"),
        medium_sweep: medium_tracker
            .precentiles()
            .expect("No medium sweep samples"),
        large_sweep: large_tracker.precentiles().expect("No large sweep samples"),
    }
}

fn print_results(results: &SweepResults, cpu_ghz: f64) {
    println!(
        "Small sweep ({} levels):",
        SMALL_SWEEP_LEVELS
    );
    println!(
        "  p50:  {:>8} cycles  ({:>7.1} ns)",
        results.small_sweep.p50,
        cycles_to_ns(results.small_sweep.p50, cpu_ghz)
    );
    println!(
        "  p99:  {:>8} cycles  ({:>7.1} ns)",
        results.small_sweep.p99,
        cycles_to_ns(results.small_sweep.p99, cpu_ghz)
    );

    println!(
        "\nMedium sweep ({} levels):",
        MEDIUM_SWEEP_LEVELS
    );
    println!(
        "  p50:  {:>8} cycles  ({:>7.1} ns)",
        results.medium_sweep.p50,
        cycles_to_ns(results.medium_sweep.p50, cpu_ghz)
    );
    println!(
        "  p99:  {:>8} cycles  ({:>7.1} ns)",
        results.medium_sweep.p99,
        cycles_to_ns(results.medium_sweep.p99, cpu_ghz)
    );

    println!(
        "\nLarge sweep ({} levels):",
        LARGE_SWEEP_LEVELS
    );
    println!(
        "  p50:  {:>8} cycles  ({:>7.1} ns)",
        results.large_sweep.p50,
        cycles_to_ns(results.large_sweep.p50, cpu_ghz)
    );
    println!(
        "  p99:  {:>8} cycles  ({:>7.1} ns)",
        results.large_sweep.p99,
        cycles_to_ns(results.large_sweep.p99, cpu_ghz)
    );
}

fn print_comparison(
    fixed: &SweepResults,
    soa: &SweepResults,
    hybrid: &SweepResults,
    tree: &SweepResults,
    cpu_ghz: f64,
) {
    println!(
        "{:<20} | {:>12} | {:>12} | {:>12} | {:>12}",
        "Sweep Size", "Fixed-Tick", "SoA", "Hybrid", "Tree"
    );
    println!("{:-<80}", "");
    println!(
        "{:<20} | {:>10} cy | {:>10} cy | {:>10} cy | {:>10} cy",
        format!("Small ({} lvl)", SMALL_SWEEP_LEVELS),
        fixed.small_sweep.p50,
        soa.small_sweep.p50,
        hybrid.small_sweep.p50,
        tree.small_sweep.p50
    );
    println!(
        "{:<20} | {:>10} cy | {:>10} cy | {:>10} cy | {:>10} cy",
        format!("Medium ({} lvl)", MEDIUM_SWEEP_LEVELS),
        fixed.medium_sweep.p50,
        soa.medium_sweep.p50,
        hybrid.medium_sweep.p50,
        tree.medium_sweep.p50
    );
    println!(
        "{:<20} | {:>10} cy | {:>10} cy | {:>10} cy | {:>10} cy",
        format!("Large ({} lvl)", LARGE_SWEEP_LEVELS),
        fixed.large_sweep.p50,
        soa.large_sweep.p50,
        hybrid.large_sweep.p50,
        tree.large_sweep.p50
    );

    // Also show in nanoseconds
    println!("\nIn nanoseconds:");
    println!(
        "{:<20} | {:>12} | {:>12} | {:>12} | {:>12}",
        "Sweep Size", "Fixed-Tick", "SoA", "Hybrid", "Tree"
    );
    println!("{:-<80}", "");
    println!(
        "{:<20} | {:>10.0} ns | {:>10.0} ns | {:>10.0} ns | {:>10.0} ns",
        format!("Small ({} lvl)", SMALL_SWEEP_LEVELS),
        cycles_to_ns(fixed.small_sweep.p50, cpu_ghz),
        cycles_to_ns(soa.small_sweep.p50, cpu_ghz),
        cycles_to_ns(hybrid.small_sweep.p50, cpu_ghz),
        cycles_to_ns(tree.small_sweep.p50, cpu_ghz)
    );
    println!(
        "{:<20} | {:>10.0} ns | {:>10.0} ns | {:>10.0} ns | {:>10.0} ns",
        format!("Medium ({} lvl)", MEDIUM_SWEEP_LEVELS),
        cycles_to_ns(fixed.medium_sweep.p50, cpu_ghz),
        cycles_to_ns(soa.medium_sweep.p50, cpu_ghz),
        cycles_to_ns(hybrid.medium_sweep.p50, cpu_ghz),
        cycles_to_ns(tree.medium_sweep.p50, cpu_ghz)
    );
    println!(
        "{:<20} | {:>10.0} ns | {:>10.0} ns | {:>10.0} ns | {:>10.0} ns",
        format!("Large ({} lvl)", LARGE_SWEEP_LEVELS),
        cycles_to_ns(fixed.large_sweep.p50, cpu_ghz),
        cycles_to_ns(soa.large_sweep.p50, cpu_ghz),
        cycles_to_ns(hybrid.large_sweep.p50, cpu_ghz),
        cycles_to_ns(tree.large_sweep.p50, cpu_ghz)
    );
}

fn print_scaling(
    fixed: &SweepResults,
    soa: &SweepResults,
    hybrid: &SweepResults,
    tree: &SweepResults,
    _cpu_ghz: f64,
) {
    // Calculate cycles per level for each implementation
    let calc_per_level = |small: u64, medium: u64, large: u64| -> (f64, f64, f64) {
        (
            small as f64 / SMALL_SWEEP_LEVELS as f64,
            medium as f64 / MEDIUM_SWEEP_LEVELS as f64,
            large as f64 / LARGE_SWEEP_LEVELS as f64,
        )
    };

    let fixed_per_level = calc_per_level(
        fixed.small_sweep.p50,
        fixed.medium_sweep.p50,
        fixed.large_sweep.p50,
    );
    let soa_per_level = calc_per_level(
        soa.small_sweep.p50,
        soa.medium_sweep.p50,
        soa.large_sweep.p50,
    );
    let hybrid_per_level = calc_per_level(
        hybrid.small_sweep.p50,
        hybrid.medium_sweep.p50,
        hybrid.large_sweep.p50,
    );
    let tree_per_level = calc_per_level(
        tree.small_sweep.p50,
        tree.medium_sweep.p50,
        tree.large_sweep.p50,
    );

    println!(
        "{:<20} | {:>12} | {:>12} | {:>12} | {:>12}",
        "Cycles/Level", "Fixed-Tick", "SoA", "Hybrid", "Tree"
    );
    println!("{:-<80}", "");
    println!(
        "{:<20} | {:>10.1} cy | {:>10.1} cy | {:>10.1} cy | {:>10.1} cy",
        "Small sweep", fixed_per_level.0, soa_per_level.0, hybrid_per_level.0, tree_per_level.0
    );
    println!(
        "{:<20} | {:>10.1} cy | {:>10.1} cy | {:>10.1} cy | {:>10.1} cy",
        "Medium sweep", fixed_per_level.1, soa_per_level.1, hybrid_per_level.1, tree_per_level.1
    );
    println!(
        "{:<20} | {:>10.1} cy | {:>10.1} cy | {:>10.1} cy | {:>10.1} cy",
        "Large sweep", fixed_per_level.2, soa_per_level.2, hybrid_per_level.2, tree_per_level.2
    );

    println!("\nInterpretation:");
    println!("  - Consistent cycles/level = good O(n) scaling");
    println!("  - Decreasing cycles/level = amortized overhead (good)");
    println!("  - Increasing cycles/level = cache pressure or algorithmic issues");
}
