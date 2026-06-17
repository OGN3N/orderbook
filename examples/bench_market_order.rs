/// Phase 5.1 (continued): execute_market_order — Current vs Optimized
///
/// Compares the CURRENT match_orders implementation (many jumps/allocs)
/// against an OPTIMIZED version that minimizes pointer chases and allocations.
///
/// Run with: cargo run --release --example bench_market_order
use orderbook::perf::latency::LatencyTracker;
use orderbook::perf::{cycles_to_ns, get_cpu_frequency};

const NUM_LEVELS: usize = 100;
const ORDERS_PER_LEVEL: usize = 1;
const ORDER_QTY: u32 = 100;
const NUM_RUNS: usize = 50;

// ============================================================================
// The Problem: Where the jumps are in current execute_market_order
// ============================================================================
//
// CURRENT FLOW for Fixed-Tick (market BUY sweeping 5 ask levels):
//
//   for i in 0..10,000 {                 ← SCAN all 10,000 slots
//     asks[i].is_empty()                 ← JUMP 1: read Vec header (ptr+len+cap)
//     if not empty:
//       match_orders(...)                ← JUMP 2: function call
//         Vec::new() fills              ← ALLOC 1: heap alloc for fills
//         Vec::new() orders_to_remove   ← ALLOC 2: heap alloc for remove list
//         level.orders.iter()           ← JUMP 3: follow Vec data ptr to heap
//           order.quantity()            ← access 24-byte Order struct
//           Fill { ... }               ← JUMP 4: push to fills Vec (may realloc)
//           orders_to_remove.push(idx) ← JUMP 5: push to remove Vec
//         for idx in removes.rev():
//           orders.remove(idx)          ← JUMP 6: memmove remaining elements
//           order_index.remove(id)      ← JUMP 7: HashMap random access
//       fills.extend(level_fills)       ← JUMP 8: copy fills, may realloc outer
//   }
//
//   Total per-level overhead:
//   - 2 heap allocations (fills + orders_to_remove)
//   - 1 function call boundary
//   - Vec::remove shifts (O(n) memmove per order)
//   - fills.extend copies all Fill structs again
//
// OPTIMIZED FLOW:
//
//   fills = Vec::with_capacity(estimated)  ← ONE alloc upfront
//   best_ask_idx = tracked                 ← SKIP the 10,000-slot scan
//   for i in best_ask_idx..ELEMENT_NUM {
//     if remaining == 0: break
//     if asks[i].is_empty(): continue
//     // Inline match — no function call, no intermediate Vecs
//     filled_count = 0
//     for order in &asks[i].orders:
//       fills.push(Fill { ... })           ← direct push, no copy
//       filled_count += 1
//     asks[i].orders.drain(..filled_count) ← ONE drain vs N removes
//     for removed in drained:
//       order_index.remove(removed.id())
//   }
//
//   Savings:
//   - 0 intermediate allocations (vs 2 per level)
//   - No function call boundary (inlined)
//   - drain(..n) instead of N × remove(0) — one memmove vs N memmoves
//   - No fills.extend copy — fills go directly into final Vec
//   - Skip empty slots via best_ask tracking
// ============================================================================

// --- Data types (self-contained, no dependency on orderbook crate types) ---

type OrderId = u64;

#[derive(Clone, Copy)]
struct Order {
    id: OrderId,
    quantity: u32,
}

struct Level {
    orders: Vec<Order>,
}

impl Level {
    fn new() -> Self {
        Level { orders: Vec::new() }
    }

    fn is_empty(&self) -> bool {
        self.orders.is_empty()
    }
}

#[derive(Debug)]
struct Fill {
    price: u32,
    quantity: u32,
    maker_order_id: OrderId,
}

// --- CURRENT implementation: mirrors the existing code exactly ---

fn execute_current(
    asks: &mut Vec<Level>,
    mut remaining: u32,
    order_index: &mut std::collections::HashMap<OrderId, u32>,
) -> Vec<Fill> {
    let mut fills = Vec::new();

    // Walk ALL slots from 0
    for i in 0..asks.len() {
        if remaining == 0 {
            break;
        }
        if asks[i].is_empty() {
            continue;
        }

        let price = i as u32;

        // match_orders — creates intermediate Vecs, uses remove()
        let level_fills = match_orders_current(&mut asks[i], &mut remaining, price, order_index);
        fills.extend(level_fills);
    }

    fills
}

fn match_orders_current(
    level: &mut Level,
    remaining: &mut u32,
    price: u32,
    order_index: &mut std::collections::HashMap<OrderId, u32>,
) -> Vec<Fill> {
    let mut fills = Vec::new();
    let mut orders_to_remove = Vec::new();

    for (idx, order) in level.orders.iter().enumerate() {
        if *remaining == 0 {
            break;
        }

        let fill_qty = (*remaining).min(order.quantity);

        fills.push(Fill {
            price,
            quantity: fill_qty,
            maker_order_id: order.id,
        });

        *remaining -= fill_qty;

        if fill_qty == order.quantity {
            orders_to_remove.push(idx);
        }
    }

    // Remove filled orders in reverse order
    for &idx in orders_to_remove.iter().rev() {
        let removed = level.orders.remove(idx);
        order_index.remove(&removed.id);
    }

    fills
}

// --- OPTIMIZED implementation: fewer jumps, fewer allocs ---

fn execute_optimized(
    asks: &mut Vec<Level>,
    mut remaining: u32,
    order_index: &mut std::collections::HashMap<OrderId, u32>,
    best_ask_idx: &mut Option<usize>,
) -> Vec<Fill> {
    // ONE allocation upfront with estimated capacity
    let estimated_fills = (remaining / ORDER_QTY) as usize + 1;
    let mut fills = Vec::with_capacity(estimated_fills);

    // Start from tracked best ask — skip empty scan
    let start = best_ask_idx.unwrap_or(0);

    for i in start..asks.len() {
        if remaining == 0 {
            break;
        }
        if asks[i].is_empty() {
            continue;
        }

        let price = i as u32;

        // INLINE matching — no function call, no intermediate Vecs
        let mut filled_count = 0usize;

        for order in asks[i].orders.iter() {
            if remaining == 0 {
                break;
            }

            let fill_qty = remaining.min(order.quantity);

            // Push directly into final fills Vec — no intermediate copy
            fills.push(Fill {
                price,
                quantity: fill_qty,
                maker_order_id: order.id,
            });

            remaining -= fill_qty;

            if fill_qty == order.quantity {
                filled_count += 1;
            }
        }

        // drain(..n): ONE memmove to shift remaining elements
        // vs remove(0) × N which does N memmoves
        for removed in asks[i].orders.drain(..filled_count) {
            order_index.remove(&removed.id);
        }

        // Update best_ask tracking
        if asks[i].is_empty() {
            *best_ask_idx = None; // Will search next time
        }
    }

    // Update best_ask_idx to the next non-empty level
    if best_ask_idx.is_none() {
        *best_ask_idx = asks.iter().position(|l| !l.is_empty());
    }

    fills
}

// --- Benchmark harness ---

fn build_book(num_levels: usize, orders_per_level: usize) -> (Vec<Level>, std::collections::HashMap<OrderId, u32>) {
    let mut asks = Vec::with_capacity(num_levels + 5000); // Space for price slots
    let mut order_index = std::collections::HashMap::new();

    // Add empty levels before the populated ones (simulates mid-price gap)
    for _ in 0..5000 {
        asks.push(Level::new());
    }

    let mut id = 0u64;
    for _ in 0..num_levels {
        let mut level = Level::new();
        for _ in 0..orders_per_level {
            let order = Order { id, quantity: ORDER_QTY };
            order_index.insert(id, ORDER_QTY);
            level.orders.push(order);
            id += 1;
        }
        asks.push(level);
    }

    (asks, order_index)
}

fn main() {
    println!("=== Phase 5.1: execute_market_order — Current vs Optimized ===\n");

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
    println!("  Empty levels before liquidity: 5000 (simulates price gap)");
    println!("  Populated levels: {}", NUM_LEVELS);
    println!("  Orders per level: {}", ORDERS_PER_LEVEL);
    println!("  Qty per order: {}", ORDER_QTY);
    println!("  Runs per sweep size: {}", NUM_RUNS);

    // Sweep sizes to test
    let sweep_sizes: Vec<u32> = vec![1, 5, 20, 50, 100];

    println!("\n--- Sweep Results ---\n");
    println!(
        "{:<12} | {:>16} | {:>16} | {:>8}",
        "Sweep (lvl)", "Current (p50)", "Optimized (p50)", "Speedup"
    );
    println!("{:-<65}", "");

    for &sweep_levels in &sweep_sizes {
        let sweep_qty = sweep_levels * ORDER_QTY;

        // Benchmark CURRENT
        let mut current_tracker = LatencyTracker::new(NUM_RUNS);
        for _ in 0..NUM_RUNS {
            let (mut asks, mut order_index) = build_book(NUM_LEVELS, ORDERS_PER_LEVEL);
            current_tracker.record(|| {
                let _fills = execute_current(&mut asks, sweep_qty, &mut order_index);
                std::hint::black_box(&_fills);
            });
        }

        // Benchmark OPTIMIZED
        let mut optimized_tracker = LatencyTracker::new(NUM_RUNS);
        for _ in 0..NUM_RUNS {
            let (mut asks, mut order_index) = build_book(NUM_LEVELS, ORDERS_PER_LEVEL);
            let mut best_ask_idx = Some(5000usize); // Known starting position
            optimized_tracker.record(|| {
                let _fills = execute_optimized(&mut asks, sweep_qty, &mut order_index, &mut best_ask_idx);
                std::hint::black_box(&_fills);
            });
        }

        let current_p = current_tracker.precentiles().unwrap();
        let optimized_p = optimized_tracker.precentiles().unwrap();

        let speedup = current_p.p50 as f64 / optimized_p.p50 as f64;

        println!(
            "{:<12} | {:>8} cy {:>5.0}ns | {:>8} cy {:>5.0}ns | {:>6.1}x",
            format!("{} lvl", sweep_levels),
            current_p.p50,
            cycles_to_ns(current_p.p50, cpu_ghz),
            optimized_p.p50,
            cycles_to_ns(optimized_p.p50, cpu_ghz),
            speedup,
        );
    }

    // --- Isolate the scan cost ---
    println!("\n--- Isolated: Scan Cost (1-level sweep) ---");
    println!("How much does skipping 5000 empty slots save?\n");

    let sweep_qty_1 = ORDER_QTY; // sweep just 1 level

    let mut scan_current = LatencyTracker::new(NUM_RUNS);
    for _ in 0..NUM_RUNS {
        let (mut asks, mut oi) = build_book(NUM_LEVELS, ORDERS_PER_LEVEL);
        scan_current.record(|| {
            let _f = execute_current(&mut asks, sweep_qty_1, &mut oi);
            std::hint::black_box(&_f);
        });
    }

    let mut scan_optimized = LatencyTracker::new(NUM_RUNS);
    for _ in 0..NUM_RUNS {
        let (mut asks, mut oi) = build_book(NUM_LEVELS, ORDERS_PER_LEVEL);
        let mut idx = Some(5000usize);
        scan_optimized.record(|| {
            let _f = execute_optimized(&mut asks, sweep_qty_1, &mut oi, &mut idx);
            std::hint::black_box(&_f);
        });
    }

    let sc = scan_current.precentiles().unwrap();
    let so = scan_optimized.precentiles().unwrap();
    println!(
        "  Current  (scan 0..5100):  {:>6} cy ({:.0} ns)",
        sc.p50,
        cycles_to_ns(sc.p50, cpu_ghz)
    );
    println!(
        "  Optimized (skip to 5000): {:>6} cy ({:.0} ns)",
        so.p50,
        cycles_to_ns(so.p50, cpu_ghz)
    );
    println!(
        "  Scan savings:             {:>6} cy ({:.0} ns) — {:.1}x speedup",
        sc.p50.saturating_sub(so.p50),
        cycles_to_ns(sc.p50.saturating_sub(so.p50), cpu_ghz),
        sc.p50 as f64 / so.p50.max(1) as f64,
    );

    // --- Deep levels: many orders per level (amplifies drain vs remove) ---
    println!("\n--- Deep Levels: 10 orders per level, 20-level sweep ---");
    println!("Amplifies drain(..n) vs N×remove(0) difference\n");

    let deep_orders = 10;
    let deep_sweep = 20 * ORDER_QTY * deep_orders as u32;

    let mut deep_current = LatencyTracker::new(NUM_RUNS);
    for _ in 0..NUM_RUNS {
        let (mut asks, mut oi) = build_book(NUM_LEVELS, deep_orders);
        deep_current.record(|| {
            let _f = execute_current(&mut asks, deep_sweep, &mut oi);
            std::hint::black_box(&_f);
        });
    }

    let mut deep_optimized = LatencyTracker::new(NUM_RUNS);
    for _ in 0..NUM_RUNS {
        let (mut asks, mut oi) = build_book(NUM_LEVELS, deep_orders);
        let mut idx = Some(5000usize);
        deep_optimized.record(|| {
            let _f = execute_optimized(&mut asks, deep_sweep, &mut oi, &mut idx);
            std::hint::black_box(&_f);
        });
    }

    let dc = deep_current.precentiles().unwrap();
    let do_ = deep_optimized.precentiles().unwrap();

    println!(
        "{:<10} | {:>14} | {:>14}",
        "Metric", "Current", "Optimized"
    );
    println!("{:-<45}", "");
    println!(
        "{:<10} | {:>8} cy {:>3.0}ns | {:>8} cy {:>3.0}ns",
        "p50", dc.p50, cycles_to_ns(dc.p50, cpu_ghz), do_.p50, cycles_to_ns(do_.p50, cpu_ghz)
    );
    println!(
        "{:<10} | {:>8} cy {:>3.0}ns | {:>8} cy {:>3.0}ns",
        "p99", dc.p99, cycles_to_ns(dc.p99, cpu_ghz), do_.p99, cycles_to_ns(do_.p99, cpu_ghz)
    );
    let deep_speedup = dc.p50 as f64 / do_.p50.max(1) as f64;
    println!("  Speedup: {:.1}x", deep_speedup);

    println!("\n--- Optimization Summary ---\n");
    println!("Optimization               | What it fixes                          | Impact");
    println!("{:-<85}", "");
    println!("best_ask_idx tracking       | Skips scan of 5000 empty slots         | ~{:.0} cy saved on every call",
        sc.p50.saturating_sub(so.p50));
    println!("Pre-allocated fills Vec     | Eliminates 2 allocs per price level    | Reduces alloc overhead");
    println!("drain(..n) vs N×remove(0)   | ONE memmove vs N memmoves              | {:.1}x on deep levels",
        deep_speedup);
    println!("Inlined match logic         | No function call boundary              | Better register use");
}
