/// Phase 5.2: Software Prefetching
///
/// Tests whether manual prefetch hints improve orderbook access patterns.
/// Measures the effect of prefetching next price levels during array scans.
///
/// Run with: cargo run --release -- bench_prefetch
///
/// NOTE: x86_64 only (uses _mm_prefetch intrinsics)
use orderbook::analysis::{CsvExporter, ResultRow};
use orderbook::methodology::latency::{LatencyTracker, Percentiles};
use orderbook::methodology::{get_tsc_frequency, tsc_ticks_to_ns};
use rand::SeedableRng;
use rand::prelude::*;
use rand::rngs::StdRng;
use std::collections::HashMap;

const DEFAULT_NUM_SAMPLES: usize = 1_000;
const SAMPLE_OVERRIDE_ENV: &str = "ORDERBOOK_PREFETCH_SAMPLES";

// ============================================================================
// Phase 5.2: Software Prefetching
// ============================================================================
//
// BACKGROUND: Cache Prefetching
//
// When the CPU accesses memory, it fetches an entire cache line (64 bytes).
// If data is not cached close to the core, the access can stall execution.
//
// HARDWARE PREFETCHER:
//   The CPU can detect some sequential and strided access patterns and fetch
//   ahead. Random indices, pointer chasing, and irregular sparse strides are
//   generally less predictable than linear traversal.
//
// SOFTWARE PREFETCH:
//   We can manually issue prefetch instructions to bring data into cache
//   before we need it. On x86:
//     _mm_prefetch(ptr, _MM_HINT_T0) supplies a high-temporal-locality hint.
//   This experiment tests only _MM_HINT_T0. A prefetch is a hint: the CPU may
//   handle it differently depending on its microarchitecture and cache state.
//
// ORDERBOOK ACCESS PATTERNS WHERE PREFETCH MIGHT HELP:
//
//   1. LEVEL SCAN (execute_market_order):
//      The Fixed-Tick orderbook scans asks[0..10000] looking for non-empty levels.
//      Each Level is a Vec header (24 bytes). The array is contiguous, so the HW
//      prefetcher may handle this well. But: when we find a non-empty level,
//      we then chase the Vec's heap pointer to read actual orders — that's the
//      unpredictable part.
//
//   2. RANDOM PRICE LOOKUP (depth_at_price, add_order at random prices):
//      If prices arrive randomly, each access hits a different cache line.
//      Prefetching the next price while processing the current one could help.
//
//   3. ORDER DATA CHASE (pointer chase through Vec → heap):
//      Level.orders is a Vec. Its data lives on the heap, pointed to by the
//      Vec header in the array. When we scan levels and find a non-empty one,
//      we must follow the pointer to read/modify orders. Prefetching the
//      heap data of the NEXT non-empty level while processing the current
//      one could hide the pointer-chase latency.
//
// WHAT WE TEST:
//   Test 1: Sequential scan — prefetch N level headers ahead
//   Test 2: Random access — prefetch next random index while processing current
//   Test 3: Pointer chase — prefetch Vec heap data of next level during scan
//   Test 4: Market order simulation — prefetch next level's orders during matching
//
// Whether the hints help is an empirical question. Each prefetching loop also
// executes extra address calculations, conditions, and instructions.
// ============================================================================

/// Simulates a Fixed-Tick Level (Vec header = 24 bytes)
/// Each level has a pointer to heap-allocated order data
struct Level {
    orders: Vec<Order>,
}

impl Default for Level {
    fn default() -> Self {
        Level { orders: Vec::new() }
    }
}

#[derive(Clone, Copy)]
#[repr(C)]
struct Order {
    id: u64,
    price: u32,
    quantity: u32,
    _side: u8,
    _pad: [u8; 7],
}

const ELEMENT_NUM: usize = 10_000;

fn main() {
    println!("=== Phase 5.2: Software Prefetching ===\n");

    let tsc_ghz = get_tsc_frequency();
    println!("TSC frequency (calibrated): {:.3} GHz", tsc_ghz);
    let num_samples = sample_count();
    println!("Samples per variant and operation: {}", num_samples);

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
        "\nLevel size: {} bytes (Vec header)",
        std::mem::size_of::<Level>()
    );
    println!("Order size: {} bytes", std::mem::size_of::<Order>());
    println!(
        "Array: {} levels × {} = {} bytes\n",
        ELEMENT_NUM,
        std::mem::size_of::<Level>(),
        ELEMENT_NUM * std::mem::size_of::<Level>(),
    );

    let seed: u64 = 42;

    let sequential = bench_sequential_scan(tsc_ghz, num_samples);
    let random = bench_random_access(seed, tsc_ghz, num_samples);
    let pointer = bench_pointer_chase(seed, tsc_ghz, num_samples);
    let market = bench_market_order_sim(seed, tsc_ghz, num_samples);

    export_csv(tsc_ghz, &sequential, &random, &pointer, &market);
}

fn sample_count() -> usize {
    match std::env::var(SAMPLE_OVERRIDE_ENV) {
        Ok(value) => value
            .parse::<usize>()
            .ok()
            .filter(|&samples| samples > 0)
            .unwrap_or_else(|| {
                panic!("{SAMPLE_OVERRIDE_ENV} must be a positive integer, got {value:?}")
            }),
        Err(std::env::VarError::NotPresent) => DEFAULT_NUM_SAMPLES,
        Err(error) => panic!("could not read {SAMPLE_OVERRIDE_ENV}: {error}"),
    }
}

// ============================================================================
// Test 1: Sequential Level Scan
// ============================================================================
// Scan all 10,000 levels sequentially, reading the Vec length (simulating
// the is_empty() check in execute_market_order).
// Compare: no prefetch vs prefetch N levels ahead.

fn bench_sequential_scan(tsc_ghz: f64, num_samples: usize) -> [Percentiles; 3] {
    println!("--- Test 1: Sequential Level Scan ---");
    println!("(Scan all 10K levels, check is_empty — simulates market order walk)\n");

    let mut levels: Vec<Level> = (0..ELEMENT_NUM).map(|_| Level::default()).collect();
    // Sprinkle some orders at random levels to make is_empty() checks non-trivial
    let mut rng = StdRng::seed_from_u64(42);
    for _ in 0..500 {
        let idx = rng.random_range(0..ELEMENT_NUM);
        levels[idx].orders.push(Order {
            id: idx as u64,
            price: idx as u32,
            quantity: 100,
            _side: 1,
            _pad: [0; 7],
        });
    }

    // No prefetch
    let mut tracker_none = LatencyTracker::new(num_samples);
    for _ in 0..num_samples {
        tracker_none.record(|| {
            let mut count = 0u64;
            for level in levels.iter() {
                if !level.orders.is_empty() {
                    count += 1;
                }
            }
            std::hint::black_box(count);
        });
    }

    // Prefetch 4 levels ahead
    let mut tracker_pf4 = LatencyTracker::new(num_samples);
    for _ in 0..num_samples {
        tracker_pf4.record(|| {
            let mut count = 0u64;
            for i in 0..ELEMENT_NUM {
                // Prefetch 4 levels ahead
                if i + 4 < ELEMENT_NUM {
                    #[cfg(target_arch = "x86_64")]
                    unsafe {
                        let ptr = &levels[i + 4] as *const Level as *const i8;
                        core::arch::x86_64::_mm_prefetch(ptr, core::arch::x86_64::_MM_HINT_T0);
                    }
                }
                if !levels[i].orders.is_empty() {
                    count += 1;
                }
            }
            std::hint::black_box(count);
        });
    }

    // Prefetch 16 levels ahead
    let mut tracker_pf16 = LatencyTracker::new(num_samples);
    for _ in 0..num_samples {
        tracker_pf16.record(|| {
            let mut count = 0u64;
            for i in 0..ELEMENT_NUM {
                if i + 16 < ELEMENT_NUM {
                    #[cfg(target_arch = "x86_64")]
                    unsafe {
                        let ptr = &levels[i + 16] as *const Level as *const i8;
                        core::arch::x86_64::_mm_prefetch(ptr, core::arch::x86_64::_MM_HINT_T0);
                    }
                }
                if !levels[i].orders.is_empty() {
                    count += 1;
                }
            }
            std::hint::black_box(count);
        });
    }

    let p_none = tracker_none.precentiles().unwrap();
    let p_pf4 = tracker_pf4.precentiles().unwrap();
    let p_pf16 = tracker_pf16.precentiles().unwrap();

    println!("{:<20} | {:>14} | {:>12}", "Variant", "p50", "Latency/None");
    println!("{:-<50}", "");
    println!(
        "{:<20} | {:>8} TSC {:>5.1}ns | {:>6}",
        "No prefetch",
        p_none.p50,
        tsc_ticks_to_ns(p_none.p50, tsc_ghz),
        "—"
    );
    println!(
        "{:<20} | {:>8} TSC {:>5.1}ns | {:>5.2}x",
        "Prefetch +4",
        p_pf4.p50,
        tsc_ticks_to_ns(p_pf4.p50, tsc_ghz),
        p_pf4.p50 as f64 / p_none.p50.max(1) as f64,
    );
    println!(
        "{:<20} | {:>8} TSC {:>5.1}ns | {:>5.2}x",
        "Prefetch +16",
        p_pf16.p50,
        tsc_ticks_to_ns(p_pf16.p50, tsc_ghz),
        p_pf16.p50 as f64 / p_none.p50.max(1) as f64,
    );
    println!();
    [p_none, p_pf4, p_pf16]
}

// ============================================================================
// Test 2: Random Price Access
// ============================================================================
// Access levels at random indices (simulates random add_order / depth_at_price).
// We know the sequence ahead of time, so we can prefetch the next index.

fn bench_random_access(seed: u64, tsc_ghz: f64, num_samples: usize) -> [Percentiles; 3] {
    println!("--- Test 2: Random Price Lookup ---");
    println!("(Access levels at random indices, prefetch next while processing current)\n");

    let mut levels: Vec<Level> = (0..ELEMENT_NUM).map(|_| Level::default()).collect();
    // Fill all levels with 1 order so reads are non-trivial
    for i in 0..ELEMENT_NUM {
        levels[i].orders.push(Order {
            id: i as u64,
            price: i as u32,
            quantity: 100,
            _side: 1,
            _pad: [0; 7],
        });
    }

    let mut rng = StdRng::seed_from_u64(seed);
    let num_accesses = 10_000;
    let indices: Vec<usize> = (0..num_accesses)
        .map(|_| rng.random_range(0..ELEMENT_NUM))
        .collect();

    // No prefetch
    let mut tracker_none = LatencyTracker::new(num_samples);
    for _ in 0..num_samples {
        tracker_none.record(|| {
            let mut sum = 0u64;
            for &idx in &indices {
                sum += levels[idx].orders.len() as u64;
            }
            std::hint::black_box(sum);
        });
    }

    // Prefetch next index
    let mut tracker_pf1 = LatencyTracker::new(num_samples);
    for _ in 0..num_samples {
        tracker_pf1.record(|| {
            let mut sum = 0u64;
            for i in 0..indices.len() {
                // Prefetch next random level while processing current
                if i + 1 < indices.len() {
                    #[cfg(target_arch = "x86_64")]
                    unsafe {
                        let ptr = &levels[indices[i + 1]] as *const Level as *const i8;
                        core::arch::x86_64::_mm_prefetch(ptr, core::arch::x86_64::_MM_HINT_T0);
                    }
                }
                sum += levels[indices[i]].orders.len() as u64;
            }
            std::hint::black_box(sum);
        });
    }

    // Prefetch 4 ahead
    let mut tracker_pf4 = LatencyTracker::new(num_samples);
    for _ in 0..num_samples {
        tracker_pf4.record(|| {
            let mut sum = 0u64;
            for i in 0..indices.len() {
                if i + 4 < indices.len() {
                    #[cfg(target_arch = "x86_64")]
                    unsafe {
                        let ptr = &levels[indices[i + 4]] as *const Level as *const i8;
                        core::arch::x86_64::_mm_prefetch(ptr, core::arch::x86_64::_MM_HINT_T0);
                    }
                }
                sum += levels[indices[i]].orders.len() as u64;
            }
            std::hint::black_box(sum);
        });
    }

    let p_none = tracker_none.precentiles().unwrap();
    let p_pf1 = tracker_pf1.precentiles().unwrap();
    let p_pf4 = tracker_pf4.precentiles().unwrap();

    println!("{:<20} | {:>14} | {:>12}", "Variant", "p50", "Latency/None");
    println!("{:-<50}", "");
    println!(
        "{:<20} | {:>8} TSC {:>5.1}ns | {:>6}",
        "No prefetch",
        p_none.p50,
        tsc_ticks_to_ns(p_none.p50, tsc_ghz),
        "—"
    );
    println!(
        "{:<20} | {:>8} TSC {:>5.1}ns | {:>5.2}x",
        "Prefetch +1",
        p_pf1.p50,
        tsc_ticks_to_ns(p_pf1.p50, tsc_ghz),
        p_pf1.p50 as f64 / p_none.p50.max(1) as f64,
    );
    println!(
        "{:<20} | {:>8} TSC {:>5.1}ns | {:>5.2}x",
        "Prefetch +4",
        p_pf4.p50,
        tsc_ticks_to_ns(p_pf4.p50, tsc_ghz),
        p_pf4.p50 as f64 / p_none.p50.max(1) as f64,
    );
    println!();
    [p_none, p_pf1, p_pf4]
}

// ============================================================================
// Test 3: Pointer Chase (Vec header → heap data)
// ============================================================================
// This is the most realistic orderbook scenario:
// We scan the level array (contiguous), but to read order data we must
// follow each Level's Vec pointer to heap-allocated Order data.
// The heap allocations are separate from the contiguous header array, so their
// addresses do not form the same simple header stride.
//
// Strategy: When processing level[i]'s orders, prefetch level[i+1]'s
// heap data (the pointer stored in the Vec header).

fn bench_pointer_chase(seed: u64, tsc_ghz: f64, num_samples: usize) -> [Percentiles; 3] {
    println!("--- Test 3: Pointer Chase (Vec header → heap orders) ---");
    println!("(Scan levels, read order quantities — prefetch next level's heap data)\n");

    let mut levels: Vec<Level> = (0..ELEMENT_NUM).map(|_| Level::default()).collect();

    // Populate ~500 random levels with 1-5 orders each
    // The heap allocations will be scattered in memory
    let mut rng = StdRng::seed_from_u64(seed);
    let mut populated_indices = Vec::new();
    for _ in 0..500 {
        let idx = rng.random_range(0..ELEMENT_NUM);
        let num_orders = rng.random_range(1..=5usize);
        for j in 0..num_orders {
            levels[idx].orders.push(Order {
                id: (idx * 10 + j) as u64,
                price: idx as u32,
                quantity: rng.random_range(1..=1000),
                _side: 1,
                _pad: [0; 7],
            });
        }
        populated_indices.push(idx);
    }
    populated_indices.sort();
    populated_indices.dedup();
    let populated_levels = populated_indices.len();
    let total_orders: usize = levels.iter().map(|level| level.orders.len()).sum();
    println!(
        "Populated levels: {}; heap orders read per scan: {}",
        populated_levels, total_orders
    );

    // No prefetch: scan and sum quantities
    let mut tracker_none = LatencyTracker::new(num_samples);
    for _ in 0..num_samples {
        tracker_none.record(|| {
            let mut total_qty = 0u64;
            for level in levels.iter() {
                for order in level.orders.iter() {
                    total_qty += order.quantity as u64;
                }
            }
            std::hint::black_box(total_qty);
        });
    }

    // Prefetch next level's orders Vec data pointer
    let mut tracker_pf = LatencyTracker::new(num_samples);
    for _ in 0..num_samples {
        tracker_pf.record(|| {
            let mut total_qty = 0u64;
            for i in 0..ELEMENT_NUM {
                // Prefetch the HEAP DATA of the level 2 steps ahead
                // This is the key insight: we prefetch where the Vec's pointer points to,
                // not the Vec header itself (which is contiguous and HW-prefetched)
                if i + 2 < ELEMENT_NUM && !levels[i + 2].orders.is_empty() {
                    #[cfg(target_arch = "x86_64")]
                    unsafe {
                        let ptr = levels[i + 2].orders.as_ptr() as *const i8;
                        core::arch::x86_64::_mm_prefetch(ptr, core::arch::x86_64::_MM_HINT_T0);
                    }
                }
                for order in levels[i].orders.iter() {
                    total_qty += order.quantity as u64;
                }
            }
            std::hint::black_box(total_qty);
        });
    }

    // Prefetch with larger distance (8 ahead)
    let mut tracker_pf8 = LatencyTracker::new(num_samples);
    for _ in 0..num_samples {
        tracker_pf8.record(|| {
            let mut total_qty = 0u64;
            for i in 0..ELEMENT_NUM {
                if i + 8 < ELEMENT_NUM && !levels[i + 8].orders.is_empty() {
                    #[cfg(target_arch = "x86_64")]
                    unsafe {
                        let ptr = levels[i + 8].orders.as_ptr() as *const i8;
                        core::arch::x86_64::_mm_prefetch(ptr, core::arch::x86_64::_MM_HINT_T0);
                    }
                }
                for order in levels[i].orders.iter() {
                    total_qty += order.quantity as u64;
                }
            }
            std::hint::black_box(total_qty);
        });
    }

    let p_none = tracker_none.precentiles().unwrap();
    let p_pf = tracker_pf.precentiles().unwrap();
    let p_pf8 = tracker_pf8.precentiles().unwrap();

    println!("{:<25} | {:>14} | {:>12}", "Variant", "p50", "Latency/None");
    println!("{:-<55}", "");
    println!(
        "{:<25} | {:>8} TSC {:>5.1}ns | {:>6}",
        "No prefetch",
        p_none.p50,
        tsc_ticks_to_ns(p_none.p50, tsc_ghz),
        "—"
    );
    println!(
        "{:<25} | {:>8} TSC {:>5.1}ns | {:>5.2}x",
        "Prefetch heap +2",
        p_pf.p50,
        tsc_ticks_to_ns(p_pf.p50, tsc_ghz),
        p_pf.p50 as f64 / p_none.p50.max(1) as f64,
    );
    println!(
        "{:<25} | {:>8} TSC {:>5.1}ns | {:>5.2}x",
        "Prefetch heap +8",
        p_pf8.p50,
        tsc_ticks_to_ns(p_pf8.p50, tsc_ghz),
        p_pf8.p50 as f64 / p_none.p50.max(1) as f64,
    );
    println!();
    [p_none, p_pf, p_pf8]
}

// ============================================================================
// Test 4: Market Order Simulation
// ============================================================================
// Full market order sweep: scan levels, find non-empty, consume orders.
// This combines the sequential scan (level headers) with pointer chase
// (reading/modifying heap-allocated orders).
//
// Two prefetch strategies:
//   A) Prefetch the next level's Vec header (array is contiguous — probably useless)
//   B) Prefetch the next non-empty level's order data (heap pointer — possibly useful)

fn bench_market_order_sim(seed: u64, tsc_ghz: f64, num_samples: usize) -> [Percentiles; 2] {
    println!("--- Test 4: Market Order Sweep (full simulation) ---");
    println!("(Sweep 20 levels, consume orders — prefetch next level's heap orders)\n");

    let sweep_levels = 20;
    let orders_per_level = 3;

    // No prefetch
    let mut tracker_none = LatencyTracker::new(num_samples);
    for _ in 0..num_samples {
        let (mut levels, mut order_index) = build_sparse_book(seed, sweep_levels, orders_per_level);
        tracker_none.record(|| {
            let target_qty = (sweep_levels * orders_per_level * 100) as u64;
            let _fills = execute_no_prefetch(&mut levels, target_qty, &mut order_index);
            std::hint::black_box(&_fills);
        });
    }

    // Prefetch next level's heap data
    let mut tracker_pf = LatencyTracker::new(num_samples);
    for _ in 0..num_samples {
        let (mut levels, mut order_index) = build_sparse_book(seed, sweep_levels, orders_per_level);
        tracker_pf.record(|| {
            let target_qty = (sweep_levels * orders_per_level * 100) as u64;
            let _fills = execute_with_prefetch(&mut levels, target_qty, &mut order_index);
            std::hint::black_box(&_fills);
        });
    }

    let p_none = tracker_none.precentiles().unwrap();
    let p_pf = tracker_pf.precentiles().unwrap();

    println!("{:<25} | {:>14} | {:>12}", "Variant", "p50", "Latency/None");
    println!("{:-<55}", "");
    println!(
        "{:<25} | {:>8} TSC {:>5.1}ns | {:>6}",
        "No prefetch",
        p_none.p50,
        tsc_ticks_to_ns(p_none.p50, tsc_ghz),
        "—"
    );
    println!(
        "{:<25} | {:>8} TSC {:>5.1}ns | {:>5.2}x",
        "Prefetch heap ahead",
        p_pf.p50,
        tsc_ticks_to_ns(p_pf.p50, tsc_ghz),
        p_pf.p50 as f64 / p_none.p50.max(1) as f64,
    );
    println!();
    [p_none, p_pf]
}

// ---- Helpers for Test 4 ----

fn build_sparse_book(
    seed: u64,
    num_levels_with_orders: usize,
    orders_per_level: usize,
) -> (Vec<Level>, HashMap<u64, u32>) {
    let mut levels: Vec<Level> = (0..ELEMENT_NUM).map(|_| Level::default()).collect();
    let mut order_index: HashMap<u64, u32> = HashMap::new();

    let mut rng = StdRng::seed_from_u64(seed);
    let mut order_id = 1u64;

    // Place orders at consecutive levels starting from index 5000
    // (simulates asks starting at the best ask)
    let start = 5000;
    for level_offset in 0..num_levels_with_orders {
        let idx = start + level_offset;
        for _ in 0..orders_per_level {
            let order = Order {
                id: order_id,
                price: idx as u32,
                quantity: 100,
                _side: 1,
                _pad: [0; 7],
            };
            levels[idx].orders.push(order);
            order_index.insert(order_id, idx as u32);
            order_id += 1;
        }
    }

    // Also add some random noise levels further out
    for _ in 0..50 {
        let idx = rng.random_range(5100..ELEMENT_NUM);
        levels[idx].orders.push(Order {
            id: order_id,
            price: idx as u32,
            quantity: 100,
            _side: 1,
            _pad: [0; 7],
        });
        order_index.insert(order_id, idx as u32);
        order_id += 1;
    }

    (levels, order_index)
}

/// Execute without prefetch — mirrors current Fixed-Tick logic
fn execute_no_prefetch(
    levels: &mut Vec<Level>,
    mut target_qty: u64,
    order_index: &mut HashMap<u64, u32>,
) -> Vec<(u64, u32, u64)> {
    let mut fills = Vec::new();

    for i in 0..ELEMENT_NUM {
        if target_qty == 0 {
            break;
        }
        if levels[i].orders.is_empty() {
            continue;
        }

        // Consume orders at this level
        let mut filled = 0usize;
        for order in levels[i].orders.iter() {
            if target_qty == 0 {
                break;
            }
            let fill_qty = target_qty.min(order.quantity as u64);
            fills.push((order.id, order.price, fill_qty));
            target_qty -= fill_qty;
            filled += 1;
        }

        // Remove filled orders
        for j in (0..filled).rev() {
            let removed = levels[i].orders.remove(j);
            order_index.remove(&removed.id);
        }
    }

    fills
}

/// Execute with prefetch — look ahead for next non-empty level's heap data
fn execute_with_prefetch(
    levels: &mut Vec<Level>,
    mut target_qty: u64,
    order_index: &mut HashMap<u64, u32>,
) -> Vec<(u64, u32, u64)> {
    let mut fills = Vec::new();

    for i in 0..ELEMENT_NUM {
        if target_qty == 0 {
            break;
        }

        // Prefetch: look ahead for next non-empty level and prefetch its order data
        // We scan a small window ahead to find the next level with orders
        #[cfg(target_arch = "x86_64")]
        {
            for lookahead in 1..=4 {
                let next = i + lookahead;
                if next < ELEMENT_NUM && !levels[next].orders.is_empty() {
                    unsafe {
                        // Prefetch the heap-allocated order data
                        let ptr = levels[next].orders.as_ptr() as *const i8;
                        core::arch::x86_64::_mm_prefetch(ptr, core::arch::x86_64::_MM_HINT_T0);
                    }
                    break; // Only prefetch the first non-empty one
                }
            }
        }

        if levels[i].orders.is_empty() {
            continue;
        }

        // Consume orders at this level
        let mut filled = 0usize;
        for order in levels[i].orders.iter() {
            if target_qty == 0 {
                break;
            }
            let fill_qty = target_qty.min(order.quantity as u64);
            fills.push((order.id, order.price, fill_qty));
            target_qty -= fill_qty;
            filled += 1;
        }

        // Remove filled orders
        for j in (0..filled).rev() {
            let removed = levels[i].orders.remove(j);
            order_index.remove(&removed.id);
        }
    }

    fills
}

fn export_csv(
    tsc_ghz: f64,
    sequential: &[Percentiles; 3],
    random: &[Percentiles; 3],
    pointer: &[Percentiles; 3],
    market: &[Percentiles; 2],
) {
    let mut csv = CsvExporter::create("bench_prefetch").expect("failed to create prefetch CSV");
    let result_groups = [
        (
            "sequential_scan_10000_levels",
            vec![
                ("no_prefetch", &sequential[0]),
                ("prefetch_header_plus_4", &sequential[1]),
                ("prefetch_header_plus_16", &sequential[2]),
            ],
        ),
        (
            "random_access_10000_reads",
            vec![
                ("no_prefetch", &random[0]),
                ("prefetch_header_plus_1", &random[1]),
                ("prefetch_header_plus_4", &random[2]),
            ],
        ),
        (
            "pointer_chase_10000_levels",
            vec![
                ("no_prefetch", &pointer[0]),
                ("prefetch_heap_plus_2", &pointer[1]),
                ("prefetch_heap_plus_8", &pointer[2]),
            ],
        ),
        (
            "market_sweep_20_levels_60_orders",
            vec![
                ("no_prefetch", &market[0]),
                ("prefetch_heap_window_4", &market[1]),
            ],
        ),
    ];

    for (operation, results) in result_groups {
        for (implementation, percentiles) in results {
            csv.append(&ResultRow {
                scenario: "bench_prefetch",
                implementation,
                operation,
                tsc_ghz,
                percentiles,
            })
            .expect("failed to append prefetch CSV row");
        }
    }
    csv.flush().expect("failed to flush prefetch CSV");
}
