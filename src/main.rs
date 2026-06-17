use std::process::Command;
use std::time::Instant;

const BENCHMARKS: &[(&str, &str)] = &[
    // Core latency comparison — most important for thesis
    ("bench_latency",        "all 4 implementations × 3 operations, full percentile table"),
    // Memory layout experiments
    ("bench_alignment",      "default vs packed vs cache-line aligned Order structs"),
    ("bench_hugepages",      "4KB vs 2MB pages — TLB pressure on 7800X3D V-Cache"),
    ("bench_prefetch",       "software prefetch effectiveness on V-Cache"),
    ("bench_market_order",   "drain() vs remove() — 1.6x speedup on deep sweeps"),
    // Synthetic distribution workloads
    ("scenario_uniform",     "uniform random price distribution"),
    ("scenario_clustered",   "90% of orders within ±10 ticks of mid"),
    ("scenario_zipfian",     "Zipfian (power-law) price distribution"),
    ("scenario_bursty",      "alternating burst/quiet traffic cycles"),
    // Realistic HFT workloads
    ("scenario_high_cancel", "10:1 cancel ratio — HFT market maker pattern"),
    ("scenario_sweep",       "market order depth sweeps (5/20/50 levels)"),
    ("scenario_buildup",     "cold-start book filling from empty"),
    ("scenario_steady_state","60% add / 30% cancel / 10% market — typical trading day"),
];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "--list" || a == "-l") {
        print_list();
        return;
    }

    // Determine which benchmarks to run
    let to_run: Vec<(&str, &str)> = if args.is_empty() {
        BENCHMARKS.to_vec()
    } else {
        let mut selected = Vec::new();
        for arg in &args {
            match BENCHMARKS.iter().find(|(name, _)| name == arg) {
                Some(&entry) => selected.push(entry),
                None => {
                    eprintln!("Unknown benchmark: '{}'. Run with --list to see options.", arg);
                    std::process::exit(1);
                }
            }
        }
        selected
    };

    println!("=== Orderbook Benchmark Runner ===\n");
    println!("Building examples...");
    build_examples();

    let total = to_run.len();
    let wall_start = Instant::now();
    let mut passed = 0;
    let mut failed: Vec<&str> = Vec::new();

    for (i, (name, desc)) in to_run.iter().enumerate() {
        println!("\n[{}/{}] {} — {}", i + 1, total, name, desc);
        println!("{}", "─".repeat(72));

        let t = Instant::now();
        let ok = run_example(name);
        let elapsed = t.elapsed().as_secs_f64();

        let time_str = if elapsed < 1.0 {
            format!("{:.0}ms", elapsed * 1000.0)
        } else {
            format!("{:.1}s", elapsed)
        };

        if ok {
            passed += 1;
            println!("\n  completed in {}", time_str);
        } else {
            failed.push(name);
            eprintln!("  FAILED after {}", time_str);
        }
    }

    let total_elapsed = wall_start.elapsed().as_secs_f64();
    let total_str = if total_elapsed < 1.0 {
        format!("{:.0}ms", total_elapsed * 1000.0)
    } else {
        format!("{:.1}s", total_elapsed)
    };

    println!("\n{}", "═".repeat(72));
    println!("Results: {}/{} benchmarks completed in {}", passed, total, total_str);

    if !failed.is_empty() {
        eprintln!("Failed: {}", failed.join(", "));
    }

    // List any CSV files that were written
    if let Ok(entries) = std::fs::read_dir("results") {
        let mut csvs: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map_or(false, |x| x == "csv"))
            .map(|e| e.path().display().to_string())
            .collect();
        csvs.sort();
        if !csvs.is_empty() {
            println!("\nCSV output:");
            for csv in &csvs {
                println!("  {}", csv);
            }
        }
    }

    if !failed.is_empty() {
        std::process::exit(1);
    }
}

fn build_examples() {
    let status = Command::new("cargo")
        .args(["build", "--examples", "--release", "--quiet"])
        .status()
        .expect("cargo not found");
    if !status.success() {
        eprintln!("Build failed — fix compile errors before running benchmarks.");
        std::process::exit(1);
    }
    println!("Build OK\n");
}

fn run_example(name: &str) -> bool {
    let binary = format!("target/release/examples/{}", name);
    Command::new(&binary)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn print_list() {
    println!("Available benchmarks:\n");
    for (name, desc) in BENCHMARKS {
        println!("  {:<24} {}", name, desc);
    }
    println!("\nUsage:");
    println!("  cargo run --release              # run all");
    println!("  cargo run --release -- <name>... # run specific benchmarks");
    println!("  cargo run --release -- --list    # show this list");
}
