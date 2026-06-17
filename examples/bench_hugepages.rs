/// Phase 5.2: Huge Pages
///
/// Compares standard 4KB pages vs 2MB huge pages (via madvise)
/// Measures TLB miss reduction for orderbook-sized arrays
///
/// Run with: cargo run --release --example bench_hugepages
///
/// NOTE: Requires Linux with THP in "madvise" or "always" mode.
/// Check: cat /sys/kernel/mm/transparent_hugepage/enabled
use orderbook::perf::latency::LatencyTracker;
use orderbook::perf::{cycles_to_ns, get_cpu_frequency};
use rand::prelude::*;
use rand::rngs::StdRng;
use rand::SeedableRng;

const NUM_SAMPLES: usize = 1_000;

// ============================================================================
// Phase 5.2: Huge Pages
// ============================================================================
//
// BACKGROUND: Virtual Memory and the TLB
//
// Every memory access goes through virtual → physical address translation:
//   1. CPU looks up the virtual address in the TLB (Translation Lookaside Buffer)
//   2. TLB HIT: physical address returned in ~1 cycle
//   3. TLB MISS: page table walk — 10-100 cycles on x86 (multiple memory reads)
//
// PAGE SIZES:
//   Standard:  4 KB pages → TLB covers ~1500 × 4KB = ~6 MB
//   Huge:      2 MB pages → TLB covers ~1500 × 2MB = ~3 GB
//
// ORDERBOOK MEMORY FOOTPRINT:
//   Fixed-Tick: 10,000 levels × 24B (Vec header) = 240 KB
//     → 60 standard pages (may cause TLB misses on random access)
//     → 1 huge page (all TLB hits)
//
//   With orders: 10,000 orders × 24B = 240 KB additional
//     → Total ~480 KB = 120 standard pages vs 1 huge page
//
// WHAT WE TEST:
//   1. Sequential scan of large array (should benefit less — prefetcher helps)
//   2. Random access of large array (should benefit more — TLB misses dominate)
//   3. Different array sizes to cross page boundaries
//
// HOW WE DO IT:
//   - mmap anonymous memory
//   - madvise(MADV_HUGEPAGE) to request THP backing
//   - Compare access latency with and without the hint
//
// EXPECTED RESULTS:
//   - Random access: huge pages should win (fewer TLB misses)
//   - Sequential access: smaller difference (prefetcher covers TLB misses)
//   - Larger arrays: bigger benefit (more pages to track)
// ============================================================================

/// Allocate `size` bytes via mmap, optionally requesting huge page backing
#[cfg(target_os = "linux")]
fn alloc_mmap(size: usize, use_hugepages: bool) -> *mut u8 {
    use std::ptr;

    let addr = unsafe {
        libc::mmap(
            ptr::null_mut(),
            size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };

    if addr == libc::MAP_FAILED {
        panic!("mmap failed");
    }

    if use_hugepages {
        unsafe {
            libc::madvise(addr, size, libc::MADV_HUGEPAGE);
        }
    } else {
        unsafe {
            libc::madvise(addr, size, libc::MADV_NOHUGEPAGE);
        }
    }

    // Touch all pages to fault them in
    let ptr = addr as *mut u8;
    for i in (0..size).step_by(4096) {
        unsafe {
            ptr.add(i).write_volatile(0);
        }
    }

    ptr
}

#[cfg(target_os = "linux")]
fn free_mmap(ptr: *mut u8, size: usize) {
    unsafe {
        libc::munmap(ptr as *mut libc::c_void, size);
    }
}

/// Simulates a price level slot (like Fixed-Tick's Level Vec header)
#[repr(C)]
struct Slot {
    count: u64,   // number of orders (simulates Vec len)
    total_qty: u64, // total quantity at this level
    _pad: [u64; 1], // pad to 24 bytes (matches Vec header size)
}

const SLOT_SIZE: usize = std::mem::size_of::<Slot>();

fn main() {
    println!("=== Phase 5.2: Huge Pages ===\n");

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

    // Check THP status
    #[cfg(target_os = "linux")]
    {
        if let Ok(thp) = std::fs::read_to_string("/sys/kernel/mm/transparent_hugepage/enabled") {
            println!("THP status: {}", thp.trim());
        }
    }

    println!("\nSlot size: {} bytes", SLOT_SIZE);
    println!("Standard page: 4 KB");
    println!("Huge page: 2 MB\n");

    #[cfg(target_os = "linux")]
    run_benchmarks(cpu_ghz);

    #[cfg(not(target_os = "linux"))]
    println!("Huge page benchmarks require Linux. Skipping.");
}

#[cfg(target_os = "linux")]
fn run_benchmarks(cpu_ghz: f64) {
    let seed: u64 = 42;

    // Test different array sizes
    let sizes: Vec<(usize, &str)> = vec![
        (10_000, "10K slots (240 KB)"),    // Fixed-Tick orderbook size
        (100_000, "100K slots (2.4 MB)"),  // Larger than L1+L2 cache
        (1_000_000, "1M slots (24 MB)"),   // Larger than L3 cache
    ];

    println!("--- Sequential Scan ---");
    println!("(Iterate all slots, sum quantities)\n");
    println!(
        "{:<25} | {:>14} | {:>14} | {:>8}",
        "Array Size", "4KB pages", "2MB pages", "Speedup"
    );
    println!("{:-<70}", "");

    for &(num_slots, label) in &sizes {
        let (normal_p50, huge_p50) = bench_sequential(num_slots, seed, cpu_ghz);
        let speedup = normal_p50 as f64 / huge_p50.max(1) as f64;
        println!(
            "{:<25} | {:>8} cy {:>3.0}ns | {:>8} cy {:>3.0}ns | {:>6.2}x",
            label,
            normal_p50, cycles_to_ns(normal_p50, cpu_ghz),
            huge_p50, cycles_to_ns(huge_p50, cpu_ghz),
            speedup,
        );
    }

    println!("\n--- Random Access ---");
    println!("(Read slots at random indices — TLB stress test)\n");
    println!(
        "{:<25} | {:>14} | {:>14} | {:>8}",
        "Array Size", "4KB pages", "2MB pages", "Speedup"
    );
    println!("{:-<70}", "");

    for &(num_slots, label) in &sizes {
        let (normal_p50, huge_p50) = bench_random(num_slots, seed, cpu_ghz);
        let speedup = normal_p50 as f64 / huge_p50.max(1) as f64;
        println!(
            "{:<25} | {:>8} cy {:>3.0}ns | {:>8} cy {:>3.0}ns | {:>6.2}x",
            label,
            normal_p50, cycles_to_ns(normal_p50, cpu_ghz),
            huge_p50, cycles_to_ns(huge_p50, cpu_ghz),
            speedup,
        );
    }

    println!("\n--- Strided Access ---");
    println!("(Access every 170th slot — crosses page boundary each time)\n");
    println!(
        "{:<25} | {:>14} | {:>14} | {:>8}",
        "Array Size", "4KB pages", "2MB pages", "Speedup"
    );
    println!("{:-<70}", "");

    for &(num_slots, label) in &sizes {
        let (normal_p50, huge_p50) = bench_strided(num_slots, seed, cpu_ghz);
        let speedup = normal_p50 as f64 / huge_p50.max(1) as f64;
        println!(
            "{:<25} | {:>8} cy {:>3.0}ns | {:>8} cy {:>3.0}ns | {:>6.2}x",
            label,
            normal_p50, cycles_to_ns(normal_p50, cpu_ghz),
            huge_p50, cycles_to_ns(huge_p50, cpu_ghz),
            speedup,
        );
    }

    // Page analysis
    println!("\n--- Page Count Analysis ---\n");
    for &(num_slots, label) in &sizes {
        let total_bytes = num_slots * SLOT_SIZE;
        let std_pages = (total_bytes + 4095) / 4096;
        let huge_pages = (total_bytes + (2 * 1024 * 1024 - 1)) / (2 * 1024 * 1024);
        println!(
            "{:<25}: {} bytes → {} std pages, {} huge pages",
            label, total_bytes, std_pages, huge_pages
        );
    }

    println!("\nInterpretation:");
    println!("  - Sequential: prefetcher hides TLB misses → small benefit");
    println!("  - Random: every access may TLB miss → biggest benefit");
    println!("  - Strided: crosses pages predictably → moderate benefit");
    println!("  - Larger arrays = more pages = more TLB pressure = bigger benefit");
}

#[cfg(target_os = "linux")]
fn bench_sequential(num_slots: usize, _seed: u64, _cpu_ghz: f64) -> (u64, u64) {
    let size = num_slots * SLOT_SIZE;

    // Normal pages
    let ptr_normal = alloc_mmap(size, false);
    let slots_normal = unsafe { std::slice::from_raw_parts_mut(ptr_normal as *mut Slot, num_slots) };
    // Initialize
    for (i, slot) in slots_normal.iter_mut().enumerate() {
        slot.count = 1;
        slot.total_qty = (i as u64) % 1000;
    }

    let mut tracker_normal = LatencyTracker::new(NUM_SAMPLES);
    for _ in 0..NUM_SAMPLES {
        tracker_normal.record(|| {
            let mut sum = 0u64;
            for slot in slots_normal.iter() {
                sum += slot.total_qty;
            }
            std::hint::black_box(sum);
        });
    }

    let p_normal = tracker_normal.precentiles().unwrap();
    free_mmap(ptr_normal, size);

    // Huge pages
    let ptr_huge = alloc_mmap(size, true);
    let slots_huge = unsafe { std::slice::from_raw_parts_mut(ptr_huge as *mut Slot, num_slots) };
    for (i, slot) in slots_huge.iter_mut().enumerate() {
        slot.count = 1;
        slot.total_qty = (i as u64) % 1000;
    }

    let mut tracker_huge = LatencyTracker::new(NUM_SAMPLES);
    for _ in 0..NUM_SAMPLES {
        tracker_huge.record(|| {
            let mut sum = 0u64;
            for slot in slots_huge.iter() {
                sum += slot.total_qty;
            }
            std::hint::black_box(sum);
        });
    }

    let p_huge = tracker_huge.precentiles().unwrap();
    free_mmap(ptr_huge, size);

    (p_normal.p50, p_huge.p50)
}

#[cfg(target_os = "linux")]
fn bench_random(num_slots: usize, seed: u64, _cpu_ghz: f64) -> (u64, u64) {
    let size = num_slots * SLOT_SIZE;
    let mut rng = StdRng::seed_from_u64(seed);

    // Pre-generate random indices
    let indices: Vec<usize> = (0..num_slots)
        .map(|_| rng.random_range(0..num_slots))
        .collect();

    // Normal pages
    let ptr_normal = alloc_mmap(size, false);
    let slots_normal = unsafe { std::slice::from_raw_parts_mut(ptr_normal as *mut Slot, num_slots) };
    for (i, slot) in slots_normal.iter_mut().enumerate() {
        slot.count = 1;
        slot.total_qty = (i as u64) % 1000;
    }

    let mut tracker_normal = LatencyTracker::new(NUM_SAMPLES);
    for _ in 0..NUM_SAMPLES {
        tracker_normal.record(|| {
            let mut sum = 0u64;
            for &idx in &indices {
                sum += slots_normal[idx].total_qty;
            }
            std::hint::black_box(sum);
        });
    }

    let p_normal = tracker_normal.precentiles().unwrap();
    free_mmap(ptr_normal, size);

    // Huge pages
    let ptr_huge = alloc_mmap(size, true);
    let slots_huge = unsafe { std::slice::from_raw_parts_mut(ptr_huge as *mut Slot, num_slots) };
    for (i, slot) in slots_huge.iter_mut().enumerate() {
        slot.count = 1;
        slot.total_qty = (i as u64) % 1000;
    }

    let mut tracker_huge = LatencyTracker::new(NUM_SAMPLES);
    for _ in 0..NUM_SAMPLES {
        tracker_huge.record(|| {
            let mut sum = 0u64;
            for &idx in &indices {
                sum += slots_huge[idx].total_qty;
            }
            std::hint::black_box(sum);
        });
    }

    let p_huge = tracker_huge.precentiles().unwrap();
    free_mmap(ptr_huge, size);

    (p_normal.p50, p_huge.p50)
}

#[cfg(target_os = "linux")]
fn bench_strided(num_slots: usize, _seed: u64, _cpu_ghz: f64) -> (u64, u64) {
    let size = num_slots * SLOT_SIZE;

    // Stride of 170 slots × 24 bytes = 4080 bytes ≈ 1 page
    // This ensures nearly every access crosses a page boundary
    let stride = 4096 / SLOT_SIZE; // ~170 slots

    // Normal pages
    let ptr_normal = alloc_mmap(size, false);
    let slots_normal = unsafe { std::slice::from_raw_parts_mut(ptr_normal as *mut Slot, num_slots) };
    for (i, slot) in slots_normal.iter_mut().enumerate() {
        slot.count = 1;
        slot.total_qty = (i as u64) % 1000;
    }

    let mut tracker_normal = LatencyTracker::new(NUM_SAMPLES);
    for _ in 0..NUM_SAMPLES {
        tracker_normal.record(|| {
            let mut sum = 0u64;
            let mut i = 0;
            while i < num_slots {
                sum += slots_normal[i].total_qty;
                i += stride;
            }
            std::hint::black_box(sum);
        });
    }

    let p_normal = tracker_normal.precentiles().unwrap();
    free_mmap(ptr_normal, size);

    // Huge pages
    let ptr_huge = alloc_mmap(size, true);
    let slots_huge = unsafe { std::slice::from_raw_parts_mut(ptr_huge as *mut Slot, num_slots) };
    for (i, slot) in slots_huge.iter_mut().enumerate() {
        slot.count = 1;
        slot.total_qty = (i as u64) % 1000;
    }

    let mut tracker_huge = LatencyTracker::new(NUM_SAMPLES);
    for _ in 0..NUM_SAMPLES {
        tracker_huge.record(|| {
            let mut sum = 0u64;
            let mut i = 0;
            while i < num_slots {
                sum += slots_huge[i].total_qty;
                i += stride;
            }
            std::hint::black_box(sum);
        });
    }

    let p_huge = tracker_huge.precentiles().unwrap();
    free_mmap(ptr_huge, size);

    (p_normal.p50, p_huge.p50)
}
