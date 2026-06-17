/// Phase 5.1: Alignment and Padding
///
/// Compares three Order struct memory layouts:
/// - Default: Rust's natural alignment (8-byte aligned, 24 bytes)
/// - Packed: No padding (#[repr(packed)], 17 bytes)
/// - CacheLine: 64-byte aligned (1 order per cache line)
///
/// Tests how alignment affects:
/// - Sequential iteration (sum quantities)
/// - Random access (lookup by index)
/// - Insert performance (push to Vec)
///
/// Run with: cargo run --release --example bench_alignment
use orderbook::perf::latency::LatencyTracker;
use orderbook::perf::{cycles_to_ns, get_cpu_frequency};
use rand::prelude::*;
use rand::rngs::StdRng;
use rand::SeedableRng;

const NUM_ORDERS: usize = 10_000;
const NUM_ORDERS_RANDOM: usize = 500_000; // spills out of L2 (~1MB), into V-Cache
const NUM_SAMPLES: usize = 1_000;
const RANDOM_BATCH: usize = 64; // accesses per sample — amortises RDTSC overhead

// ============================================================================
// Phase 5.1: Alignment and Padding
// ============================================================================
//
// BACKGROUND:
// Modern CPUs access memory in cache lines (64 bytes on x86). How data is
// aligned within cache lines dramatically affects performance:
//
// 1. NATURAL ALIGNMENT (Default):
//    Rust aligns structs to their largest field. Our Order has u64 (8 bytes),
//    so it aligns to 8-byte boundaries. Size: 24 bytes.
//    Layout: [id:8][side:1][pad:3][price:4][qty:4] = 24 bytes
//
//    Cache line packing: 64 / 24 = 2.6 orders per line
//    Some orders STRADDLE two cache lines (split access).
//    Example: Orders at byte offsets 0, 24, 48 — order at 48 spans lines
//    (bytes 48-63 in line 0, bytes 64-71 in line 1).
//
// 2. PACKED (no padding):
//    #[repr(packed)] removes all padding. Size: 17 bytes.
//    Layout: [id:8][side:1][price:4][qty:4] = 17 bytes
//
//    Cache line packing: 64 / 17 = 3.7 orders per line
//    MORE orders fit in cache (42% more than default).
//    But fields may be UNALIGNED: reading a u64 that doesn't start on an
//    8-byte boundary causes an unaligned access penalty.
//    On x86: works but slower. On ARM: may fault.
//
// 3. CACHE-LINE ALIGNED (64 bytes):
//    #[repr(C, align(64))] pads each order to exactly 64 bytes.
//    Layout: [id:8][side:1][pad:3][price:4][qty:4][pad:44] = 64 bytes
//
//    Exactly 1 order per cache line. No straddling, no false sharing.
//    But uses 2.7x more memory than default.
//    Good for: multi-threaded access (no false sharing)
//    Bad for: sequential scan (fewer orders in cache)
//
// FALSE SHARING:
// When two threads modify data on the SAME cache line, the line "bounces"
// between cores (MESI protocol). Cache-line alignment eliminates this
// by ensuring each order lives on its own line.
//
// WHAT WE MEASURE:
// - Sequential scan: iterate all orders, sum quantities
// - Random access: look up orders at random indices
// - Insert: push orders into a Vec
// ============================================================================

// --- Order struct variants ---

/// Default Rust alignment: 8-byte aligned, 24 bytes total
/// Fields reordered by compiler for optimal packing
#[derive(Clone, Copy)]
struct OrderDefault {
    id: u64,       // 8 bytes
    side: u8,      // 1 byte + 3 bytes padding
    price: u32,    // 4 bytes
    quantity: u32, // 4 bytes
}
// Total: 24 bytes (with padding)

/// Packed: no padding, 17 bytes total
/// Fields are laid out exactly as declared
#[repr(packed)]
#[derive(Clone, Copy)]
struct OrderPacked {
    id: u64,       // 8 bytes
    side: u8,      // 1 byte (no padding!)
    price: u32,    // 4 bytes (may be unaligned!)
    quantity: u32, // 4 bytes
}
// Total: 17 bytes

/// Cache-line aligned: 64 bytes, one order per cache line
/// Eliminates false sharing and split-line access
#[repr(C, align(64))]
#[derive(Clone, Copy)]
struct OrderAligned64 {
    id: u64,       // 8 bytes
    side: u8,      // 1 byte
    price: u32,    // 4 bytes
    quantity: u32, // 4 bytes
    // 44 bytes padding added by align(64)
}
// Total: 64 bytes

fn main() {
    println!("=== Phase 5.1: Alignment and Padding ===\n");

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

    // Print layout analysis
    println!("\n--- Memory Layout Analysis ---\n");

    println!("OrderDefault:");
    println!("  size:      {} bytes", std::mem::size_of::<OrderDefault>());
    println!("  align:     {} bytes", std::mem::align_of::<OrderDefault>());
    println!(
        "  per cache line: {:.1}",
        64.0 / std::mem::size_of::<OrderDefault>() as f64
    );
    println!(
        "  {} orders = {} bytes ({:.1} KB)",
        NUM_ORDERS,
        NUM_ORDERS * std::mem::size_of::<OrderDefault>(),
        (NUM_ORDERS * std::mem::size_of::<OrderDefault>()) as f64 / 1024.0
    );

    println!("\nOrderPacked:");
    println!("  size:      {} bytes", std::mem::size_of::<OrderPacked>());
    println!("  align:     {} bytes", std::mem::align_of::<OrderPacked>());
    println!(
        "  per cache line: {:.1}",
        64.0 / std::mem::size_of::<OrderPacked>() as f64
    );
    println!(
        "  {} orders = {} bytes ({:.1} KB)",
        NUM_ORDERS,
        NUM_ORDERS * std::mem::size_of::<OrderPacked>(),
        (NUM_ORDERS * std::mem::size_of::<OrderPacked>()) as f64 / 1024.0
    );

    println!("\nOrderAligned64:");
    println!("  size:      {} bytes", std::mem::size_of::<OrderAligned64>());
    println!("  align:     {} bytes", std::mem::align_of::<OrderAligned64>());
    println!(
        "  per cache line: {:.1}",
        64.0 / std::mem::size_of::<OrderAligned64>() as f64
    );
    println!(
        "  {} orders = {} bytes ({:.1} KB)",
        NUM_ORDERS,
        NUM_ORDERS * std::mem::size_of::<OrderAligned64>(),
        (NUM_ORDERS * std::mem::size_of::<OrderAligned64>()) as f64 / 1024.0
    );

    // Straddle analysis for Default
    let straddle_count = count_straddles(
        std::mem::size_of::<OrderDefault>(),
        std::mem::align_of::<OrderDefault>(),
        NUM_ORDERS,
    );
    println!(
        "\nCache line straddles (Default): {}/{} orders ({:.1}%)",
        straddle_count,
        NUM_ORDERS,
        straddle_count as f64 / NUM_ORDERS as f64 * 100.0
    );
    let straddle_count_packed = count_straddles(
        std::mem::size_of::<OrderPacked>(),
        std::mem::align_of::<OrderPacked>(),
        NUM_ORDERS,
    );
    println!(
        "Cache line straddles (Packed):  {}/{} orders ({:.1}%)",
        straddle_count_packed,
        NUM_ORDERS,
        straddle_count_packed as f64 / NUM_ORDERS as f64 * 100.0
    );
    println!(
        "Cache line straddles (Aligned): 0/{} orders (0.0%)",
        NUM_ORDERS
    );

    let seed: u64 = 42;

    // Run benchmarks
    println!("\n--- Sequential Scan: sum all quantities ---");
    println!("(Tests cache line utilization during linear traversal)\n");

    let default_seq = bench_sequential_default(seed);
    let packed_seq = bench_sequential_packed(seed);
    let aligned_seq = bench_sequential_aligned(seed);
    print_bench_comparison("Sequential", &default_seq, &packed_seq, &aligned_seq, cpu_ghz);

    println!("\n--- Random Access: {} random reads per sample ({} orders, spills L2) ---", RANDOM_BATCH, NUM_ORDERS_RANDOM);
    println!("(Tests cache miss behavior per layout — time is per batch of {} accesses)\n", RANDOM_BATCH);

    let default_rnd = bench_random_access_default(seed);
    let packed_rnd = bench_random_access_packed(seed);
    let aligned_rnd = bench_random_access_aligned(seed);
    print_bench_comparison("Random Access", &default_rnd, &packed_rnd, &aligned_rnd, cpu_ghz);

    println!("\n--- Insert: build Vec of {} orders from scratch (with reallocation) ---", NUM_ORDERS);
    println!("(Tests allocation cost per layout — time is per full insert of {} orders)\n", NUM_ORDERS);

    let default_ins = bench_insert_default(seed);
    let packed_ins = bench_insert_packed(seed);
    let aligned_ins = bench_insert_aligned(seed);
    print_bench_comparison("Insert", &default_ins, &packed_ins, &aligned_ins, cpu_ghz);

    println!("\n--- Summary ---");
    print_summary(
        &default_seq, &packed_seq, &aligned_seq,
        &default_rnd, &packed_rnd, &aligned_rnd,
        &default_ins, &packed_ins, &aligned_ins,
        cpu_ghz,
    );
}

/// Count how many orders straddle a cache line boundary
fn count_straddles(struct_size: usize, _struct_align: usize, count: usize) -> usize {
    let mut straddles = 0;
    for i in 0..count {
        // Simulate contiguous allocation (Vec)
        // Starting address aligned to struct_align
        let offset = i * struct_size;
        let start_line = offset / 64;
        let end_line = (offset + struct_size - 1) / 64;
        if start_line != end_line {
            straddles += 1;
        }
    }
    straddles
}

struct BenchResult {
    p50: u64,
    p99: u64,
    max: u64,
}

// ============================================================================
// Sequential Scan Benchmarks
// Iterate over all orders in a Vec, summing quantities.
// This is the most cache-friendly access pattern.
// Packed should win: more orders per cache line = fewer cache misses.
// ============================================================================

fn bench_sequential_default(seed: u64) -> BenchResult {
    let mut rng = StdRng::seed_from_u64(seed);
    let orders: Vec<OrderDefault> = (0..NUM_ORDERS)
        .map(|i| OrderDefault {
            id: i as u64,
            side: if rng.random_bool(0.5) { 0 } else { 1 },
            price: rng.random_range(1..10000),
            quantity: 100,
        })
        .collect();

    let mut tracker = LatencyTracker::new(NUM_SAMPLES);
    for _ in 0..NUM_SAMPLES {
        tracker.record(|| {
            let mut sum = 0u64;
            for order in &orders {
                sum += order.quantity as u64;
            }
            std::hint::black_box(sum);
        });
    }

    let p = tracker.precentiles().expect("No samples");
    BenchResult { p50: p.p50, p99: p.p99, max: p.max }
}

fn bench_sequential_packed(seed: u64) -> BenchResult {
    let mut rng = StdRng::seed_from_u64(seed);
    let orders: Vec<OrderPacked> = (0..NUM_ORDERS)
        .map(|i| OrderPacked {
            id: i as u64,
            side: if rng.random_bool(0.5) { 0 } else { 1 },
            price: rng.random_range(1..10000),
            quantity: 100,
        })
        .collect();

    let mut tracker = LatencyTracker::new(NUM_SAMPLES);
    for _ in 0..NUM_SAMPLES {
        tracker.record(|| {
            let mut sum = 0u64;
            for order in &orders {
                // Packed fields: addr_of! avoids creating a misaligned reference
                let qty = unsafe { std::ptr::read_unaligned(std::ptr::addr_of!(order.quantity)) };
                sum += qty as u64;
            }
            std::hint::black_box(sum);
        });
    }

    let p = tracker.precentiles().expect("No samples");
    BenchResult { p50: p.p50, p99: p.p99, max: p.max }
}

fn bench_sequential_aligned(seed: u64) -> BenchResult {
    let mut rng = StdRng::seed_from_u64(seed);
    let orders: Vec<OrderAligned64> = (0..NUM_ORDERS)
        .map(|i| OrderAligned64 {
            id: i as u64,
            side: if rng.random_bool(0.5) { 0 } else { 1 },
            price: rng.random_range(1..10000),
            quantity: 100,
        })
        .collect();

    let mut tracker = LatencyTracker::new(NUM_SAMPLES);
    for _ in 0..NUM_SAMPLES {
        tracker.record(|| {
            let mut sum = 0u64;
            for order in &orders {
                sum += order.quantity as u64;
            }
            std::hint::black_box(sum);
        });
    }

    let p = tracker.precentiles().expect("No samples");
    BenchResult { p50: p.p50, p99: p.p99, max: p.max }
}

// ============================================================================
// Random Access Benchmarks
// Access orders at random indices. This defeats cache prefetching.
// Aligned should show more consistent latency (no straddles).
// ============================================================================

fn bench_random_access_default(seed: u64) -> BenchResult {
    let mut rng = StdRng::seed_from_u64(seed);
    let orders: Vec<OrderDefault> = (0..NUM_ORDERS_RANDOM)
        .map(|i| OrderDefault {
            id: i as u64,
            side: if rng.random_bool(0.5) { 0 } else { 1 },
            price: rng.random_range(1..10000),
            quantity: 100,
        })
        .collect();

    // NUM_SAMPLES batches of RANDOM_BATCH accesses each — amortises RDTSC overhead
    let indices: Vec<usize> = (0..NUM_SAMPLES * RANDOM_BATCH)
        .map(|_| rng.random_range(0..NUM_ORDERS_RANDOM))
        .collect();

    let mut tracker = LatencyTracker::new(NUM_SAMPLES);
    for chunk in indices.chunks(RANDOM_BATCH) {
        tracker.record(|| {
            for &idx in chunk {
                let order = &orders[idx];
                std::hint::black_box(order.quantity);
            }
        });
    }

    let p = tracker.precentiles().expect("No samples");
    BenchResult { p50: p.p50, p99: p.p99, max: p.max }
}

fn bench_random_access_packed(seed: u64) -> BenchResult {
    let mut rng = StdRng::seed_from_u64(seed);
    let orders: Vec<OrderPacked> = (0..NUM_ORDERS_RANDOM)
        .map(|i| OrderPacked {
            id: i as u64,
            side: if rng.random_bool(0.5) { 0 } else { 1 },
            price: rng.random_range(1..10000),
            quantity: 100,
        })
        .collect();

    let indices: Vec<usize> = (0..NUM_SAMPLES * RANDOM_BATCH)
        .map(|_| rng.random_range(0..NUM_ORDERS_RANDOM))
        .collect();

    let mut tracker = LatencyTracker::new(NUM_SAMPLES);
    for chunk in indices.chunks(RANDOM_BATCH) {
        tracker.record(|| {
            for &idx in chunk {
                let order = &orders[idx];
                let qty = unsafe { std::ptr::read_unaligned(std::ptr::addr_of!(order.quantity)) };
                std::hint::black_box(qty);
            }
        });
    }

    let p = tracker.precentiles().expect("No samples");
    BenchResult { p50: p.p50, p99: p.p99, max: p.max }
}

fn bench_random_access_aligned(seed: u64) -> BenchResult {
    let mut rng = StdRng::seed_from_u64(seed);
    let orders: Vec<OrderAligned64> = (0..NUM_ORDERS_RANDOM)
        .map(|i| OrderAligned64 {
            id: i as u64,
            side: if rng.random_bool(0.5) { 0 } else { 1 },
            price: rng.random_range(1..10000),
            quantity: 100,
        })
        .collect();

    let indices: Vec<usize> = (0..NUM_SAMPLES * RANDOM_BATCH)
        .map(|_| rng.random_range(0..NUM_ORDERS_RANDOM))
        .collect();

    let mut tracker = LatencyTracker::new(NUM_SAMPLES);
    for chunk in indices.chunks(RANDOM_BATCH) {
        tracker.record(|| {
            for &idx in chunk {
                let order = &orders[idx];
                std::hint::black_box(order.quantity);
            }
        });
    }

    let p = tracker.precentiles().expect("No samples");
    BenchResult { p50: p.p50, p99: p.p99, max: p.max }
}

// ============================================================================
// Insert Benchmarks
// Push orders into a Vec one at a time.
// Tests write patterns and allocation behavior per layout.
// ============================================================================

// Insert benchmarks: measure time to fill a fresh Vec (no pre-allocated capacity).
// Each sample rebuilds from scratch so reallocation cost is included.
// We batch NUM_ORDERS pushes per sample — the unit is "insert NUM_ORDERS orders".

fn bench_insert_default(seed: u64) -> BenchResult {
    let mut rng = StdRng::seed_from_u64(seed);
    let prepared: Vec<OrderDefault> = (0..NUM_ORDERS)
        .map(|i| OrderDefault {
            id: i as u64,
            side: if rng.random_bool(0.5) { 0 } else { 1 },
            price: rng.random_range(1..10000),
            quantity: 100,
        })
        .collect();

    let mut tracker = LatencyTracker::new(NUM_SAMPLES);
    for _ in 0..NUM_SAMPLES {
        let orders_ref = &prepared;
        tracker.record(|| {
            let mut v: Vec<OrderDefault> = Vec::new();
            for &o in orders_ref {
                v.push(o);
            }
            std::hint::black_box(v);
        });
    }

    let p = tracker.precentiles().expect("No samples");
    BenchResult { p50: p.p50, p99: p.p99, max: p.max }
}

fn bench_insert_packed(seed: u64) -> BenchResult {
    let mut rng = StdRng::seed_from_u64(seed);
    let prepared: Vec<OrderPacked> = (0..NUM_ORDERS)
        .map(|i| OrderPacked {
            id: i as u64,
            side: if rng.random_bool(0.5) { 0 } else { 1 },
            price: rng.random_range(1..10000),
            quantity: 100,
        })
        .collect();

    let mut tracker = LatencyTracker::new(NUM_SAMPLES);
    for _ in 0..NUM_SAMPLES {
        let orders_ref = &prepared;
        tracker.record(|| {
            let mut v: Vec<OrderPacked> = Vec::new();
            for &o in orders_ref {
                v.push(o);
            }
            std::hint::black_box(v);
        });
    }

    let p = tracker.precentiles().expect("No samples");
    BenchResult { p50: p.p50, p99: p.p99, max: p.max }
}

fn bench_insert_aligned(seed: u64) -> BenchResult {
    let mut rng = StdRng::seed_from_u64(seed);
    let prepared: Vec<OrderAligned64> = (0..NUM_ORDERS)
        .map(|i| OrderAligned64 {
            id: i as u64,
            side: if rng.random_bool(0.5) { 0 } else { 1 },
            price: rng.random_range(1..10000),
            quantity: 100,
        })
        .collect();

    let mut tracker = LatencyTracker::new(NUM_SAMPLES);
    for _ in 0..NUM_SAMPLES {
        let orders_ref = &prepared;
        tracker.record(|| {
            let mut v: Vec<OrderAligned64> = Vec::new();
            for &o in orders_ref {
                v.push(o);
            }
            std::hint::black_box(v);
        });
    }

    let p = tracker.precentiles().expect("No samples");
    BenchResult { p50: p.p50, p99: p.p99, max: p.max }
}

// ============================================================================
// Output
// ============================================================================

fn print_bench_comparison(
    label: &str,
    default: &BenchResult,
    packed: &BenchResult,
    aligned: &BenchResult,
    cpu_ghz: f64,
) {
    println!(
        "{:<15} | {:>14} | {:>14} | {:>14}",
        label, "Default (24B)", "Packed (17B)", "Aligned (64B)"
    );
    println!("{:-<65}", "");
    println!(
        "{:<15} | {:>8} cy {:>4.0}ns | {:>8} cy {:>4.0}ns | {:>8} cy {:>4.0}ns",
        "p50",
        default.p50,
        cycles_to_ns(default.p50, cpu_ghz),
        packed.p50,
        cycles_to_ns(packed.p50, cpu_ghz),
        aligned.p50,
        cycles_to_ns(aligned.p50, cpu_ghz),
    );
    println!(
        "{:<15} | {:>8} cy {:>4.0}ns | {:>8} cy {:>4.0}ns | {:>8} cy {:>4.0}ns",
        "p99",
        default.p99,
        cycles_to_ns(default.p99, cpu_ghz),
        packed.p99,
        cycles_to_ns(packed.p99, cpu_ghz),
        aligned.p99,
        cycles_to_ns(aligned.p99, cpu_ghz),
    );
    println!(
        "{:<15} | {:>8} cy {:>4.0}ns | {:>8} cy {:>4.0}ns | {:>8} cy {:>4.0}ns",
        "Max",
        default.max,
        cycles_to_ns(default.max, cpu_ghz),
        packed.max,
        cycles_to_ns(packed.max, cpu_ghz),
        aligned.max,
        cycles_to_ns(aligned.max, cpu_ghz),
    );
}

fn print_summary(
    def_seq: &BenchResult, pack_seq: &BenchResult, align_seq: &BenchResult,
    def_rnd: &BenchResult, pack_rnd: &BenchResult, align_rnd: &BenchResult,
    def_ins: &BenchResult, pack_ins: &BenchResult, align_ins: &BenchResult,
    cpu_ghz: f64,
) {
    println!("\np50 comparison (cycles):");
    println!(
        "{:<15} | {:>14} | {:>14} | {:>14}",
        "Operation", "Default (24B)", "Packed (17B)", "Aligned (64B)"
    );
    println!("{:-<65}", "");
    println!(
        "{:<15} | {:>12} cy | {:>12} cy | {:>12} cy",
        "Sequential", def_seq.p50, pack_seq.p50, align_seq.p50
    );
    println!(
        "{:<15} | {:>12} cy | {:>12} cy | {:>12} cy",
        "Random Access", def_rnd.p50, pack_rnd.p50, align_rnd.p50
    );
    println!(
        "{:<15} | {:>12} cy | {:>12} cy | {:>12} cy",
        "Insert", def_ins.p50, pack_ins.p50, align_ins.p50
    );

    println!("\nMemory footprint for {} orders:", NUM_ORDERS);
    println!(
        "  Default:  {:>8} bytes ({:.1} KB)",
        NUM_ORDERS * std::mem::size_of::<OrderDefault>(),
        (NUM_ORDERS * std::mem::size_of::<OrderDefault>()) as f64 / 1024.0
    );
    println!(
        "  Packed:   {:>8} bytes ({:.1} KB) — {:.0}% of Default",
        NUM_ORDERS * std::mem::size_of::<OrderPacked>(),
        (NUM_ORDERS * std::mem::size_of::<OrderPacked>()) as f64 / 1024.0,
        std::mem::size_of::<OrderPacked>() as f64 / std::mem::size_of::<OrderDefault>() as f64 * 100.0
    );
    println!(
        "  Aligned:  {:>8} bytes ({:.1} KB) — {:.0}% of Default",
        NUM_ORDERS * std::mem::size_of::<OrderAligned64>(),
        (NUM_ORDERS * std::mem::size_of::<OrderAligned64>()) as f64 / 1024.0,
        std::mem::size_of::<OrderAligned64>() as f64 / std::mem::size_of::<OrderDefault>() as f64 * 100.0
    );

    println!("\nTradeoffs:");
    println!("  Packed:  -29% memory, but unaligned access penalties on field reads");
    println!("  Aligned: +167% memory, but zero cache line straddles, zero false sharing");
    println!("  Default: balanced — natural alignment with moderate padding");
}
