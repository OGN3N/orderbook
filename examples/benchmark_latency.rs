/// Latency benchmark for orderbook implementations
///
/// Measures the latency of three core operations:
/// 1. add_order() - Adding a limit order to the book
/// 2. cancel_order() - Canceling an existing order
/// 3. execute_market_order() - Executing a market order
///
/// Run with: cargo run --release --example benchmark_latency

use orderbook::orderbook::fixed_tick::orderbook::Orderbook as FixedTickOrderbook;
use orderbook::orderbook::hybrid::orderbook::Orderbook as HybridOrderbook;
use orderbook::orderbook::tree::orderbook::Orderbook as TreeOrderbook;
use orderbook::orderbook::SoA::orderbook::Orderbook as SoAOrderbook;
use orderbook::orderbook::OrderbookTrait;
use orderbook::perf::latency::{LatencyTracker, Percentiles};
use orderbook::types::order::{IdCounter, Order, OrderId, Side};
use orderbook::types::price::Price;
use orderbook::types::quantity::Quantity;

const NUM_SAMPLES: usize = 10_000;

fn main() {
    println!("=== Orderbook Latency Benchmark ===\n");
    println!("Measuring {} samples per operation\n", NUM_SAMPLES);

    // Benchmark each implementation
    println!("--- Fixed-Tick Array Orderbook ---");
    let fixed_stats = benchmark_orderbook::<FixedTickOrderbook>();
    print_results(&fixed_stats);

    println!("\n--- Structure-of-Arrays (SoA) Orderbook ---");
    let soa_stats = benchmark_orderbook::<SoAOrderbook>();
    print_results(&soa_stats);

    println!("\n--- Hybrid (Hot/Cold) Orderbook ---");
    let hybrid_stats = benchmark_orderbook::<HybridOrderbook>();
    print_results(&hybrid_stats);

    println!("\n--- Tree-Based Orderbook ---");
    let tree_stats = benchmark_orderbook::<TreeOrderbook>();
    print_results(&tree_stats);

    println!("\n=== Comparison Table ===\n");
    compare_all_implementations(&fixed_stats, &soa_stats, &hybrid_stats, &tree_stats);
}

struct BenchmarkResults {
    add_order: Percentiles,
    cancel_order: Percentiles,
    market_order: Percentiles,
}

fn benchmark_orderbook<O: OrderbookTrait>() -> BenchmarkResults {
    // Create trackers for each operation
    let mut add_tracker = LatencyTracker::new(NUM_SAMPLES);
    let mut cancel_tracker = LatencyTracker::new(NUM_SAMPLES);
    let mut market_tracker = LatencyTracker::new(NUM_SAMPLES);

    // Benchmark add_order
    let mut book = O::new();
    let mut id_counter = IdCounter::new();
    let mut order_ids = Vec::with_capacity(NUM_SAMPLES);

    for i in 0..NUM_SAMPLES {
        let side = if i % 2 == 0 { Side::Bid } else { Side::Ask };

        // Spread orders across wide range [4000, 6000]
        let price_offset = (i % 2000) as u32;
        let price_value = 4000 + price_offset;

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

    // Benchmark cancel_order
    for &order_id in &order_ids {
        cancel_tracker.record(|| {
            book.cancel_order(order_id)
                .expect("Failed to cancel order");
        });
    }

    // Benchmark execute_market_order
    // First, repopulate the book
    let mut book = O::new();
    let mut id_counter = IdCounter::new();

    // Add 200 ask orders with NO GAPS - fills every slot in hot zone [4900, 5100)
    // This tests Hybrid's best case: fully populated hot zone
    for i in 0..200 {
        let price_value = 4900 + i; // Prices: 4900, 4901, 4902, ..., 5099
        let order = Order::new(
            Price::define(price_value),
            Quantity::define(100),
            Side::Ask,
            &mut id_counter,
        );
        book.add_order(order).expect("Failed to add order");
    }

    // Execute 100 market buy orders
    for _ in 0..100 {
        market_tracker.record(|| {
            book.execute_market_order(Side::Bid, Quantity::define(100))
                .expect("Failed to execute market order");
        });
    }

    BenchmarkResults {
        add_order: add_tracker.precentiles().expect("No add_order samples"),
        cancel_order: cancel_tracker.precentiles().expect("No cancel_order samples"),
        market_order: market_tracker.precentiles().expect("No market_order samples"),
    }
}

fn print_results(results: &BenchmarkResults) {
    println!("add_order():");
    print_percentiles(&results.add_order);

    println!("\ncancel_order():");
    print_percentiles(&results.cancel_order);

    println!("\nexecute_market_order():");
    print_percentiles(&results.market_order);
}

fn print_percentiles(p: &Percentiles) {
    println!("  Min:    {:>8} cycles", p.min);
    println!("  p50:    {:>8} cycles (median)", p.p50);
    println!("  Mean:   {:>8.2} cycles", p.mean);
    println!("  p95:    {:>8} cycles", p.p95);
    println!("  p99:    {:>8} cycles", p.p99);
    println!("  p99.9:  {:>8} cycles", p.p999);
    println!("  p99.99: {:>8} cycles", p.p9999);
    println!("  Max:    {:>8} cycles", p.max);
}

fn compare_all_implementations(
    fixed: &BenchmarkResults,
    soa: &BenchmarkResults,
    hybrid: &BenchmarkResults,
    tree: &BenchmarkResults,
) {
    println!("Median (p50) Latencies:");
    println!("{:-<80}", "");
    println!(
        "{:<20} | {:>12} | {:>12} | {:>12} | {:>12}",
        "Operation", "Fixed-Tick", "SoA", "Hybrid", "Tree"
    );
    println!("{:-<80}", "");
    print_comparison_row(
        "add_order",
        fixed.add_order.p50,
        soa.add_order.p50,
        hybrid.add_order.p50,
        tree.add_order.p50,
    );
    print_comparison_row(
        "cancel_order",
        fixed.cancel_order.p50,
        soa.cancel_order.p50,
        hybrid.cancel_order.p50,
        tree.cancel_order.p50,
    );
    print_comparison_row(
        "market_order",
        fixed.market_order.p50,
        soa.market_order.p50,
        hybrid.market_order.p50,
        tree.market_order.p50,
    );
    println!("{:-<80}", "");

    println!("\np99 Latencies:");
    println!("{:-<80}", "");
    println!(
        "{:<20} | {:>12} | {:>12} | {:>12} | {:>12}",
        "Operation", "Fixed-Tick", "SoA", "Hybrid", "Tree"
    );
    println!("{:-<80}", "");
    print_comparison_row(
        "add_order",
        fixed.add_order.p99,
        soa.add_order.p99,
        hybrid.add_order.p99,
        tree.add_order.p99,
    );
    print_comparison_row(
        "cancel_order",
        fixed.cancel_order.p99,
        soa.cancel_order.p99,
        hybrid.cancel_order.p99,
        tree.cancel_order.p99,
    );
    print_comparison_row(
        "market_order",
        fixed.market_order.p99,
        soa.market_order.p99,
        hybrid.market_order.p99,
        tree.market_order.p99,
    );
    println!("{:-<80}", "");

    // Find and highlight the winner for each operation
    println!("\nWinners (lowest latency):");
    print_winner("add_order (p50)", &[
        ("Fixed-Tick", fixed.add_order.p50),
        ("SoA", soa.add_order.p50),
        ("Hybrid", hybrid.add_order.p50),
        ("Tree", tree.add_order.p50),
    ]);
    print_winner("cancel_order (p50)", &[
        ("Fixed-Tick", fixed.cancel_order.p50),
        ("SoA", soa.cancel_order.p50),
        ("Hybrid", hybrid.cancel_order.p50),
        ("Tree", tree.cancel_order.p50),
    ]);
    print_winner("market_order (p50)", &[
        ("Fixed-Tick", fixed.market_order.p50),
        ("SoA", soa.market_order.p50),
        ("Hybrid", hybrid.market_order.p50),
        ("Tree", tree.market_order.p50),
    ]);
}

fn print_comparison_row(name: &str, fixed: u64, soa: u64, hybrid: u64, tree: u64) {
    println!(
        "{:<20} | {:>10} cy | {:>10} cy | {:>10} cy | {:>10} cy",
        name, fixed, soa, hybrid, tree
    );
}

fn print_winner(operation: &str, results: &[(&str, u64)]) {
    let (winner_name, winner_cycles) = results
        .iter()
        .min_by_key(|(_, cycles)| cycles)
        .unwrap();

    let second_best_cycles = results
        .iter()
        .filter(|(name, _)| name != winner_name)
        .min_by_key(|(_, cycles)| cycles)
        .map(|(_, cycles)| *cycles)
        .unwrap();

    let speedup = second_best_cycles as f64 / *winner_cycles as f64;
    println!(
        "  {:<20} : {} ({} cycles, {:.2}x faster than 2nd best)",
        operation, winner_name, winner_cycles, speedup
    );
}
