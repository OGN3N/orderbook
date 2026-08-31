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
use orderbook::analysis::{CsvExporter, ResultRow};
use orderbook::perf::latency::{LatencyTracker, Percentiles};
use orderbook::perf::{get_tsc_frequency, tsc_ticks_to_ns};
use rand::SeedableRng;
use rand::prelude::*;
use rand::rngs::StdRng;

const NUM_ORDERS: usize = 10_000;
const NUM_ORDERS_RANDOM: usize = 500_000; // spills out of L2 (~1MB), into V-Cache
const DEFAULT_NUM_SAMPLES: usize = 1_000;
const SAMPLE_OVERRIDE_ENV: &str = "ORDERBOOK_ALIGNMENT_SAMPLES";
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
//    Rust may reorder fields in repr(Rust); the observed total is 24 bytes.
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
//    MORE orders fit in the same byte range (about 41% more than default).
//    But fields may be UNALIGNED and cannot be accessed through ordinary
//    aligned Rust references.
//    Packed quantities are therefore read with ptr::read_unaligned.
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
// FALSE SHARING (NOT MEASURED BY THIS SINGLE-THREADED EXPERIMENT):
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
    side: u8,      // 1 byte
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
    id: u64,    // 8 bytes
    side: u8,   // 1 byte
    price: u32, // 4 bytes
    quantity: u32, // 4 bytes
                // 44 bytes padding added by align(64)
}
// Total: 64 bytes

fn main() {
    println!("=== Phase 5.1: Alignment and Padding ===\n");

    let tsc_ghz = get_tsc_frequency();
    println!("TSC frequency (calibrated): {:.3} GHz", tsc_ghz);
    let num_samples = sample_count();
    println!("Samples per layout and operation: {}", num_samples);

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
    println!(
        "  align:     {} bytes",
        std::mem::align_of::<OrderDefault>()
    );
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
    println!(
        "  size:      {} bytes",
        std::mem::size_of::<OrderAligned64>()
    );
    println!(
        "  align:     {} bytes",
        std::mem::align_of::<OrderAligned64>()
    );
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
    println!("\nTheoretical straddles with a cache-line-aligned Vec base:");
    println!(
        "Cache line straddles (Default): {}/{} orders ({:.1}%)",
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
    println!("(Compares repeated linear traversal across record layouts)\n");

    let default_seq = bench_sequential_default(seed, num_samples);
    let packed_seq = bench_sequential_packed(seed, num_samples);
    let aligned_seq = bench_sequential_aligned(seed, num_samples);
    print_bench_comparison(
        "Sequential",
        &default_seq,
        &packed_seq,
        &aligned_seq,
        tsc_ghz,
    );

    println!(
        "\n--- Random Access: {} random reads per sample ({} orders, spills L2) ---",
        RANDOM_BATCH, NUM_ORDERS_RANDOM
    );
    println!(
        "(Time is per batch of {} indexed quantity reads)\n",
        RANDOM_BATCH
    );

    let default_rnd = bench_random_access_default(seed, num_samples);
    let packed_rnd = bench_random_access_packed(seed, num_samples);
    let aligned_rnd = bench_random_access_aligned(seed, num_samples);
    print_bench_comparison(
        "Random Access",
        &default_rnd,
        &packed_rnd,
        &aligned_rnd,
        tsc_ghz,
    );

    println!(
        "\n--- Insert: build Vec of {} orders from scratch (with reallocation) ---",
        NUM_ORDERS
    );
    println!(
        "(Time includes allocation, copying, reallocation, and destruction for {} orders)\n",
        NUM_ORDERS
    );

    let default_ins = bench_insert_default(seed, num_samples);
    let packed_ins = bench_insert_packed(seed, num_samples);
    let aligned_ins = bench_insert_aligned(seed, num_samples);
    print_bench_comparison("Insert", &default_ins, &packed_ins, &aligned_ins, tsc_ghz);

    println!("\n--- Summary ---");
    print_summary(
        &default_seq,
        &packed_seq,
        &aligned_seq,
        &default_rnd,
        &packed_rnd,
        &aligned_rnd,
        &default_ins,
        &packed_ins,
        &aligned_ins,
        tsc_ghz,
    );

    export_csv(
        tsc_ghz,
        &default_seq,
        &packed_seq,
        &aligned_seq,
        &default_rnd,
        &packed_rnd,
        &aligned_rnd,
        &default_ins,
        &packed_ins,
        &aligned_ins,
    );
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

type BenchResult = Percentiles;

// ============================================================================
// Sequential Scan Benchmarks
// Iterate over all orders in a Vec, summing quantities.
// This is the most cache-friendly access pattern.
// Compares the effect of record density during linear traversal.
// ============================================================================

fn bench_sequential_default(seed: u64, num_samples: usize) -> BenchResult {
    let mut rng = StdRng::seed_from_u64(seed);
    let orders: Vec<OrderDefault> = (0..NUM_ORDERS)
        .map(|i| OrderDefault {
            id: i as u64,
            side: if rng.random_bool(0.5) { 0 } else { 1 },
            price: rng.random_range(1..10000),
            quantity: 100,
        })
        .collect();

    let mut tracker = LatencyTracker::new(num_samples);
    for _ in 0..num_samples {
        tracker.record(|| {
            let mut sum = 0u64;
            for order in &orders {
                sum += order.quantity as u64;
            }
            std::hint::black_box(sum);
        });
    }

    tracker.precentiles().expect("No samples")
}

fn bench_sequential_packed(seed: u64, num_samples: usize) -> BenchResult {
    let mut rng = StdRng::seed_from_u64(seed);
    let orders: Vec<OrderPacked> = (0..NUM_ORDERS)
        .map(|i| OrderPacked {
            id: i as u64,
            side: if rng.random_bool(0.5) { 0 } else { 1 },
            price: rng.random_range(1..10000),
            quantity: 100,
        })
        .collect();

    let mut tracker = LatencyTracker::new(num_samples);
    for _ in 0..num_samples {
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

    tracker.precentiles().expect("No samples")
}

fn bench_sequential_aligned(seed: u64, num_samples: usize) -> BenchResult {
    let mut rng = StdRng::seed_from_u64(seed);
    let orders: Vec<OrderAligned64> = (0..NUM_ORDERS)
        .map(|i| OrderAligned64 {
            id: i as u64,
            side: if rng.random_bool(0.5) { 0 } else { 1 },
            price: rng.random_range(1..10000),
            quantity: 100,
        })
        .collect();

    let mut tracker = LatencyTracker::new(num_samples);
    for _ in 0..num_samples {
        tracker.record(|| {
            let mut sum = 0u64;
            for order in &orders {
                sum += order.quantity as u64;
            }
            std::hint::black_box(sum);
        });
    }

    tracker.precentiles().expect("No samples")
}

// ============================================================================
// Random Access Benchmarks
// Access orders at random indices to reduce predictable spatial traversal.
// ============================================================================

fn bench_random_access_default(seed: u64, num_samples: usize) -> BenchResult {
    let mut rng = StdRng::seed_from_u64(seed);
    let orders: Vec<OrderDefault> = (0..NUM_ORDERS_RANDOM)
        .map(|i| OrderDefault {
            id: i as u64,
            side: if rng.random_bool(0.5) { 0 } else { 1 },
            price: rng.random_range(1..10000),
            quantity: 100,
        })
        .collect();

    // Batches of RANDOM_BATCH accesses amortise the timestamp overhead.
    let indices: Vec<usize> = (0..num_samples * RANDOM_BATCH)
        .map(|_| rng.random_range(0..NUM_ORDERS_RANDOM))
        .collect();

    let mut tracker = LatencyTracker::new(num_samples);
    for chunk in indices.chunks(RANDOM_BATCH) {
        tracker.record(|| {
            for &idx in chunk {
                let order = &orders[idx];
                std::hint::black_box(order.quantity);
            }
        });
    }

    tracker.precentiles().expect("No samples")
}

fn bench_random_access_packed(seed: u64, num_samples: usize) -> BenchResult {
    let mut rng = StdRng::seed_from_u64(seed);
    let orders: Vec<OrderPacked> = (0..NUM_ORDERS_RANDOM)
        .map(|i| OrderPacked {
            id: i as u64,
            side: if rng.random_bool(0.5) { 0 } else { 1 },
            price: rng.random_range(1..10000),
            quantity: 100,
        })
        .collect();

    let indices: Vec<usize> = (0..num_samples * RANDOM_BATCH)
        .map(|_| rng.random_range(0..NUM_ORDERS_RANDOM))
        .collect();

    let mut tracker = LatencyTracker::new(num_samples);
    for chunk in indices.chunks(RANDOM_BATCH) {
        tracker.record(|| {
            for &idx in chunk {
                let order = &orders[idx];
                let qty = unsafe { std::ptr::read_unaligned(std::ptr::addr_of!(order.quantity)) };
                std::hint::black_box(qty);
            }
        });
    }

    tracker.precentiles().expect("No samples")
}

fn bench_random_access_aligned(seed: u64, num_samples: usize) -> BenchResult {
    let mut rng = StdRng::seed_from_u64(seed);
    let orders: Vec<OrderAligned64> = (0..NUM_ORDERS_RANDOM)
        .map(|i| OrderAligned64 {
            id: i as u64,
            side: if rng.random_bool(0.5) { 0 } else { 1 },
            price: rng.random_range(1..10000),
            quantity: 100,
        })
        .collect();

    let indices: Vec<usize> = (0..num_samples * RANDOM_BATCH)
        .map(|_| rng.random_range(0..NUM_ORDERS_RANDOM))
        .collect();

    let mut tracker = LatencyTracker::new(num_samples);
    for chunk in indices.chunks(RANDOM_BATCH) {
        tracker.record(|| {
            for &idx in chunk {
                let order = &orders[idx];
                std::hint::black_box(order.quantity);
            }
        });
    }

    tracker.precentiles().expect("No samples")
}

// ============================================================================
// Insert Benchmarks
// Push orders into a Vec one at a time.
// Tests write patterns and allocation behavior per layout.
// ============================================================================

// Insert benchmarks: measure time to fill a fresh Vec (no pre-allocated capacity).
// Each sample rebuilds from scratch so reallocation cost is included.
// We batch NUM_ORDERS pushes per sample — the unit is "insert NUM_ORDERS orders".

fn bench_insert_default(seed: u64, num_samples: usize) -> BenchResult {
    let mut rng = StdRng::seed_from_u64(seed);
    let prepared: Vec<OrderDefault> = (0..NUM_ORDERS)
        .map(|i| OrderDefault {
            id: i as u64,
            side: if rng.random_bool(0.5) { 0 } else { 1 },
            price: rng.random_range(1..10000),
            quantity: 100,
        })
        .collect();

    let mut tracker = LatencyTracker::new(num_samples);
    for _ in 0..num_samples {
        let orders_ref = &prepared;
        tracker.record(|| {
            let mut v: Vec<OrderDefault> = Vec::new();
            for &o in orders_ref {
                v.push(o);
            }
            std::hint::black_box(v);
        });
    }

    tracker.precentiles().expect("No samples")
}

fn bench_insert_packed(seed: u64, num_samples: usize) -> BenchResult {
    let mut rng = StdRng::seed_from_u64(seed);
    let prepared: Vec<OrderPacked> = (0..NUM_ORDERS)
        .map(|i| OrderPacked {
            id: i as u64,
            side: if rng.random_bool(0.5) { 0 } else { 1 },
            price: rng.random_range(1..10000),
            quantity: 100,
        })
        .collect();

    let mut tracker = LatencyTracker::new(num_samples);
    for _ in 0..num_samples {
        let orders_ref = &prepared;
        tracker.record(|| {
            let mut v: Vec<OrderPacked> = Vec::new();
            for &o in orders_ref {
                v.push(o);
            }
            std::hint::black_box(v);
        });
    }

    tracker.precentiles().expect("No samples")
}

fn bench_insert_aligned(seed: u64, num_samples: usize) -> BenchResult {
    let mut rng = StdRng::seed_from_u64(seed);
    let prepared: Vec<OrderAligned64> = (0..NUM_ORDERS)
        .map(|i| OrderAligned64 {
            id: i as u64,
            side: if rng.random_bool(0.5) { 0 } else { 1 },
            price: rng.random_range(1..10000),
            quantity: 100,
        })
        .collect();

    let mut tracker = LatencyTracker::new(num_samples);
    for _ in 0..num_samples {
        let orders_ref = &prepared;
        tracker.record(|| {
            let mut v: Vec<OrderAligned64> = Vec::new();
            for &o in orders_ref {
                v.push(o);
            }
            std::hint::black_box(v);
        });
    }

    tracker.precentiles().expect("No samples")
}

// ============================================================================
// Output
// ============================================================================

fn print_bench_comparison(
    label: &str,
    default: &BenchResult,
    packed: &BenchResult,
    aligned: &BenchResult,
    tsc_ghz: f64,
) {
    println!(
        "{:<15} | {:>14} | {:>14} | {:>14}",
        label, "Default (24B)", "Packed (17B)", "Aligned (64B)"
    );
    println!("{:-<65}", "");
    println!(
        "{:<15} | {:>8} TSC {:>4.0}ns | {:>8} TSC {:>4.0}ns | {:>8} TSC {:>4.0}ns",
        "p50",
        default.p50,
        tsc_ticks_to_ns(default.p50, tsc_ghz),
        packed.p50,
        tsc_ticks_to_ns(packed.p50, tsc_ghz),
        aligned.p50,
        tsc_ticks_to_ns(aligned.p50, tsc_ghz),
    );
    println!(
        "{:<15} | {:>8} TSC {:>4.0}ns | {:>8} TSC {:>4.0}ns | {:>8} TSC {:>4.0}ns",
        "p99",
        default.p99,
        tsc_ticks_to_ns(default.p99, tsc_ghz),
        packed.p99,
        tsc_ticks_to_ns(packed.p99, tsc_ghz),
        aligned.p99,
        tsc_ticks_to_ns(aligned.p99, tsc_ghz),
    );
    println!(
        "{:<15} | {:>8} TSC {:>4.0}ns | {:>8} TSC {:>4.0}ns | {:>8} TSC {:>4.0}ns",
        "Max",
        default.max,
        tsc_ticks_to_ns(default.max, tsc_ghz),
        packed.max,
        tsc_ticks_to_ns(packed.max, tsc_ghz),
        aligned.max,
        tsc_ticks_to_ns(aligned.max, tsc_ghz),
    );
}

fn print_summary(
    def_seq: &BenchResult,
    pack_seq: &BenchResult,
    align_seq: &BenchResult,
    def_rnd: &BenchResult,
    pack_rnd: &BenchResult,
    align_rnd: &BenchResult,
    def_ins: &BenchResult,
    pack_ins: &BenchResult,
    align_ins: &BenchResult,
    _tsc_ghz: f64,
) {
    println!("\np50 comparison (TSC ticks):");
    println!(
        "{:<15} | {:>14} | {:>14} | {:>14}",
        "Operation", "Default (24B)", "Packed (17B)", "Aligned (64B)"
    );
    println!("{:-<65}", "");
    println!(
        "{:<15} | {:>11} TSC | {:>11} TSC | {:>11} TSC",
        "Sequential", def_seq.p50, pack_seq.p50, align_seq.p50
    );
    println!(
        "{:<15} | {:>11} TSC | {:>11} TSC | {:>11} TSC",
        "Random Access", def_rnd.p50, pack_rnd.p50, align_rnd.p50
    );
    println!(
        "{:<15} | {:>11} TSC | {:>11} TSC | {:>11} TSC",
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
        std::mem::size_of::<OrderPacked>() as f64 / std::mem::size_of::<OrderDefault>() as f64
            * 100.0
    );
    println!(
        "  Aligned:  {:>8} bytes ({:.1} KB) — {:.0}% of Default",
        NUM_ORDERS * std::mem::size_of::<OrderAligned64>(),
        (NUM_ORDERS * std::mem::size_of::<OrderAligned64>()) as f64 / 1024.0,
        std::mem::size_of::<OrderAligned64>() as f64 / std::mem::size_of::<OrderDefault>() as f64
            * 100.0
    );

    println!("\nTradeoffs:");
    println!("  Packed:  -29% memory and requires unaligned field reads");
    println!("  Aligned: +167% memory and prevents adjacent records sharing a cache line");
    println!("  Default: balanced — natural alignment with moderate padding");
}

#[allow(clippy::too_many_arguments)]
fn export_csv(
    tsc_ghz: f64,
    def_seq: &BenchResult,
    pack_seq: &BenchResult,
    align_seq: &BenchResult,
    def_rnd: &BenchResult,
    pack_rnd: &BenchResult,
    align_rnd: &BenchResult,
    def_ins: &BenchResult,
    pack_ins: &BenchResult,
    align_ins: &BenchResult,
) {
    let mut csv = CsvExporter::create("bench_alignment").expect("failed to create alignment CSV");
    let layouts = [
        ("default_24b", [def_seq, def_rnd, def_ins]),
        ("packed_17b", [pack_seq, pack_rnd, pack_ins]),
        ("aligned_64b", [align_seq, align_rnd, align_ins]),
    ];
    let operations = [
        "sequential_scan_10000",
        "random_read_batch_64",
        "insert_batch_10000",
    ];

    for (implementation, results) in layouts {
        for (operation, percentiles) in operations.iter().copied().zip(results) {
            csv.append(&ResultRow {
                scenario: "bench_alignment",
                implementation,
                operation,
                tsc_ghz,
                percentiles,
            })
            .expect("failed to append alignment CSV row");
        }
    }
    csv.flush().expect("failed to flush alignment CSV");
}
