/// Phase 5.3: Software Prefetching
///
/// Tests whether manual prefetch hints improve orderbook access patterns.
/// Measures the effect of prefetching next price levels during array scans.
///
/// Run with: cargo run --release --example bench_prefetch
///
/// NOTE: x86_64 only (uses _mm_prefetch intrinsics)
use orderbook::perf::latency::LatencyTracker;
use orderbook::perf::{cycles_to_ns, get_cpu_frequency};
use rand::prelude::*;
use rand::rngs::StdRng;
use rand::SeedableRng;
use std::collections::HashMap;

const NUM_SAMPLES: usize = 1_000;

// ============================================================================
// Phase 5.3: Software Prefetching
// ============================================================================
//
// BACKGROUND: Cache Prefetching
//
// When the CPU accesses memory, it fetches an entire cache line (64 bytes).
// If data isn't in cache, it stalls for ~100 cycles (L3) or ~200+ cycles (DRAM).
//
// HARDWARE PREFETCHER:
//   The CPU automatically detects sequential and strided access patterns and
//   prefetches ahead. This works great for linear scans but fails for:
//   - Random access (no pattern to detect)
//   - Pointer chasing (Vec<Order> heap data reached through Vec headers)
//   - Irregular strides (scattered non-empty levels in sparse array)
//
// SOFTWARE PREFETCH:
//   We can manually issue prefetch instructions to bring data into cache
//   before we need it. On x86:
//     _mm_prefetch(ptr, _MM_HINT_T0)  → prefetch into L1 (tightest, ~4 cycle hint)
//     _mm_prefetch(ptr, _MM_HINT_T1)  → prefetch into L2
//     _mm_prefetch(ptr, _MM_HINT_T2)  → prefetch into L3
//     _mm_prefetch(ptr, _MM_HINT_NTA) → prefetch non-temporal (bypass cache)
//
// ORDERBOOK ACCESS PATTERNS WHERE PREFETCH MIGHT HELP:
//
//   1. LEVEL SCAN (execute_market_order):
//      The Fixed-Tick orderbook scans asks[0..10000] looking for non-empty levels.
//      Each Level is a Vec header (24 bytes). The array is contiguous, so the HW
//      prefetcher should handle this well. But: when we find a non-empty level,
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
//   Test 1: Sequential scan — prefetch N levels ahead (HW should already do this)
//   Test 2: Random access — prefetch next random index while processing current
//   Test 3: Pointer chase — prefetch Vec heap data of next level during scan
//   Test 4: Market order simulation — prefetch next level's orders during matching
//
// EXPECTED RESULTS:
//   - Sequential: no improvement (HW prefetcher dominates)
//   - Random: possible improvement (HW can't predict random pattern)
//   - Pointer chase: possible improvement (HW can't follow heap pointers)
//   - Market order sim: possible improvement (heap data is the bottleneck)
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
    println!("=== Phase 5.3: Software Prefetching ===\n");

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

    println!("\nLevel size: {} bytes (Vec header)", std::mem::size_of::<Level>());
    println!("Order size: {} bytes", std::mem::size_of::<Order>());
    println!("Array: {} levels × {} = {} bytes\n",
        ELEMENT_NUM,
        std::mem::size_of::<Level>(),
        ELEMENT_NUM * std::mem::size_of::<Level>(),
    );

    let seed: u64 = 42;

    bench_sequential_scan(cpu_ghz);
    bench_random_access(seed, cpu_ghz);
    bench_pointer_chase(seed, cpu_ghz);
    bench_market_order_sim(seed, cpu_ghz);

    println!("\nInterpretation:");
    println!("  - Sequential scan: HW prefetcher handles contiguous arrays well");
    println!("  - Random access: SW prefetch helps if access pattern is known 1+ steps ahead");
    println!("  - Pointer chase: SW prefetch helps hide heap-pointer latency");
    println!("  - Market order: Combined effect — scan + pointer chase");
    println!("  - 7800X3D 96MB V-Cache may reduce all benefits (everything fits in L3)");
}

// ============================================================================
// Test 1: Sequential Level Scan
// ============================================================================
// Scan all 10,000 levels sequentially, reading the Vec length (simulating
// the is_empty() check in execute_market_order).
// Compare: no prefetch vs prefetch N levels ahead.

fn bench_sequential_scan(cpu_ghz: f64) {
    println!("--- Test 1: Sequential Level Scan ---");
    println!("(Scan all 10K levels, check is_empty — simulates market order walk)\n");

    let mut levels: Vec<Level> = (0..ELEMENT_NUM).map(|_| Level::default()).collect();
    // Sprinkle some orders at random levels to make is_empty() checks non-trivial
    let mut rng = StdRng::seed_from_u64(42);
    for _ in 0..500 {
        let idx = rng.random_range(0..ELEMENT_NUM);
        levels[idx].orders.push(Order {
            id: idx as u64, price: idx as u32, quantity: 100, _side: 1, _pad: [0; 7],
        });
    }

    // No prefetch
    let mut tracker_none = LatencyTracker::new(NUM_SAMPLES);
    for _ in 0..NUM_SAMPLES {
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
    let mut tracker_pf4 = LatencyTracker::new(NUM_SAMPLES);
    for _ in 0..NUM_SAMPLES {
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
    let mut tracker_pf16 = LatencyTracker::new(NUM_SAMPLES);
    for _ in 0..NUM_SAMPLES {
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

    println!(
        "{:<20} | {:>14} | {:>8}",
        "Variant", "p50", "vs None"
    );
    println!("{:-<50}", "");
    println!(
        "{:<20} | {:>8} cy {:>3.0}ns | {:>6}",
        "No prefetch", p_none.p50, cycles_to_ns(p_none.p50, cpu_ghz), "—"
    );
    println!(
        "{:<20} | {:>8} cy {:>3.0}ns | {:>5.2}x",
        "Prefetch +4", p_pf4.p50, cycles_to_ns(p_pf4.p50, cpu_ghz),
        p_none.p50 as f64 / p_pf4.p50.max(1) as f64,
    );
    println!(
        "{:<20} | {:>8} cy {:>3.0}ns | {:>5.2}x",
        "Prefetch +16", p_pf16.p50, cycles_to_ns(p_pf16.p50, cpu_ghz),
        p_none.p50 as f64 / p_pf16.p50.max(1) as f64,
    );
    println!();
}

// ============================================================================
// Test 2: Random Price Access
// ============================================================================
// Access levels at random indices (simulates random add_order / depth_at_price).
// We know the sequence ahead of time, so we can prefetch the next index.

fn bench_random_access(seed: u64, cpu_ghz: f64) {
    println!("--- Test 2: Random Price Lookup ---");
    println!("(Access levels at random indices, prefetch next while processing current)\n");

    let mut levels: Vec<Level> = (0..ELEMENT_NUM).map(|_| Level::default()).collect();
    // Fill all levels with 1 order so reads are non-trivial
    for i in 0..ELEMENT_NUM {
        levels[i].orders.push(Order {
            id: i as u64, price: i as u32, quantity: 100, _side: 1, _pad: [0; 7],
        });
    }

    let mut rng = StdRng::seed_from_u64(seed);
    let num_accesses = 10_000;
    let indices: Vec<usize> = (0..num_accesses)
        .map(|_| rng.random_range(0..ELEMENT_NUM))
        .collect();

    // No prefetch
    let mut tracker_none = LatencyTracker::new(NUM_SAMPLES);
    for _ in 0..NUM_SAMPLES {
        tracker_none.record(|| {
            let mut sum = 0u64;
            for &idx in &indices {
                sum += levels[idx].orders.len() as u64;
            }
            std::hint::black_box(sum);
        });
    }

    // Prefetch next index
    let mut tracker_pf1 = LatencyTracker::new(NUM_SAMPLES);
    for _ in 0..NUM_SAMPLES {
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
    let mut tracker_pf4 = LatencyTracker::new(NUM_SAMPLES);
    for _ in 0..NUM_SAMPLES {
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

    println!(
        "{:<20} | {:>14} | {:>8}",
        "Variant", "p50", "vs None"
    );
    println!("{:-<50}", "");
    println!(
        "{:<20} | {:>8} cy {:>3.0}ns | {:>6}",
        "No prefetch", p_none.p50, cycles_to_ns(p_none.p50, cpu_ghz), "—"
    );
    println!(
        "{:<20} | {:>8} cy {:>3.0}ns | {:>5.2}x",
        "Prefetch +1", p_pf1.p50, cycles_to_ns(p_pf1.p50, cpu_ghz),
        p_none.p50 as f64 / p_pf1.p50.max(1) as f64,
    );
    println!(
        "{:<20} | {:>8} cy {:>3.0}ns | {:>5.2}x",
        "Prefetch +4", p_pf4.p50, cycles_to_ns(p_pf4.p50, cpu_ghz),
        p_none.p50 as f64 / p_pf4.p50.max(1) as f64,
    );
    println!();
}

// ============================================================================
// Test 3: Pointer Chase (Vec header → heap data)
// ============================================================================
// This is the most realistic orderbook scenario:
// We scan the level array (contiguous), but to read order data we must
// follow each Level's Vec pointer to heap-allocated Order data.
// The heap allocations are scattered — HW prefetcher can't predict them.
//
// Strategy: When processing level[i]'s orders, prefetch level[i+1]'s
// heap data (the pointer stored in the Vec header).

fn bench_pointer_chase(seed: u64, cpu_ghz: f64) {
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

    // No prefetch: scan and sum quantities
    let mut tracker_none = LatencyTracker::new(NUM_SAMPLES);
    for _ in 0..NUM_SAMPLES {
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
    let mut tracker_pf = LatencyTracker::new(NUM_SAMPLES);
    for _ in 0..NUM_SAMPLES {
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
    let mut tracker_pf8 = LatencyTracker::new(NUM_SAMPLES);
    for _ in 0..NUM_SAMPLES {
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

    println!(
        "{:<25} | {:>14} | {:>8}",
        "Variant", "p50", "vs None"
    );
    println!("{:-<55}", "");
    println!(
        "{:<25} | {:>8} cy {:>3.0}ns | {:>6}",
        "No prefetch", p_none.p50, cycles_to_ns(p_none.p50, cpu_ghz), "—"
    );
    println!(
        "{:<25} | {:>8} cy {:>3.0}ns | {:>5.2}x",
        "Prefetch heap +2", p_pf.p50, cycles_to_ns(p_pf.p50, cpu_ghz),
        p_none.p50 as f64 / p_pf.p50.max(1) as f64,
    );
    println!(
        "{:<25} | {:>8} cy {:>3.0}ns | {:>5.2}x",
        "Prefetch heap +8", p_pf8.p50, cycles_to_ns(p_pf8.p50, cpu_ghz),
        p_none.p50 as f64 / p_pf8.p50.max(1) as f64,
    );
    println!();
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

fn bench_market_order_sim(seed: u64, cpu_ghz: f64) {
    println!("--- Test 4: Market Order Sweep (full simulation) ---");
    println!("(Sweep 20 levels, consume orders — prefetch next level's heap orders)\n");

    let sweep_levels = 20;
    let orders_per_level = 3;

    // No prefetch
    let mut tracker_none = LatencyTracker::new(NUM_SAMPLES);
    for _ in 0..NUM_SAMPLES {
        let (mut levels, mut order_index) = build_sparse_book(seed, sweep_levels, orders_per_level);
        tracker_none.record(|| {
            let target_qty = (sweep_levels * orders_per_level * 100) as u64;
            let _fills = execute_no_prefetch(&mut levels, target_qty, &mut order_index);
            std::hint::black_box(&_fills);
        });
    }

    // Prefetch next level's heap data
    let mut tracker_pf = LatencyTracker::new(NUM_SAMPLES);
    for _ in 0..NUM_SAMPLES {
        let (mut levels, mut order_index) = build_sparse_book(seed, sweep_levels, orders_per_level);
        tracker_pf.record(|| {
            let target_qty = (sweep_levels * orders_per_level * 100) as u64;
            let _fills = execute_with_prefetch(&mut levels, target_qty, &mut order_index);
            std::hint::black_box(&_fills);
        });
    }

    let p_none = tracker_none.precentiles().unwrap();
    let p_pf = tracker_pf.precentiles().unwrap();

    println!(
        "{:<25} | {:>14} | {:>8}",
        "Variant", "p50", "vs None"
    );
    println!("{:-<55}", "");
    println!(
        "{:<25} | {:>8} cy {:>3.0}ns | {:>6}",
        "No prefetch", p_none.p50, cycles_to_ns(p_none.p50, cpu_ghz), "—"
    );
    println!(
        "{:<25} | {:>8} cy {:>3.0}ns | {:>5.2}x",
        "Prefetch heap ahead", p_pf.p50, cycles_to_ns(p_pf.p50, cpu_ghz),
        p_none.p50 as f64 / p_pf.p50.max(1) as f64,
    );
    println!();
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
