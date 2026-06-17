use crate::perf::{cycles_to_ns, latency::Percentiles};
use std::fs::{self, File};
use std::io::{BufWriter, Write};

/// One row in the results CSV: a single (scenario, implementation, operation) measurement.
pub struct ResultRow<'a> {
    pub scenario: &'a str,
    pub implementation: &'a str,
    pub operation: &'a str,
    pub cpu_ghz: f64,
    pub percentiles: &'a Percentiles,
}

/// Writes benchmark results to a CSV file.
///
/// Creates `results/<name>.csv` — one row per (scenario, implementation, operation).
/// Column layout is stable so multiple runs can be stacked in a spreadsheet or Python.
pub struct CsvExporter {
    writer: BufWriter<File>,
}

impl CsvExporter {
    pub fn create(name: &str) -> std::io::Result<Self> {
        fs::create_dir_all("results")?;
        let path = format!("results/{}.csv", name);
        let file = File::create(&path)?;
        let mut writer = BufWriter::new(file);
        writeln!(
            writer,
            "scenario,implementation,operation,cpu_ghz,\
             min_cy,p50_cy,p95_cy,p99_cy,p999_cy,p9999_cy,max_cy,mean_cy,\
             min_ns,p50_ns,p95_ns,p99_ns,p999_ns,p9999_ns,max_ns,mean_ns"
        )?;
        println!("Results → {}", path);
        Ok(Self { writer })
    }

    pub fn append(&mut self, row: &ResultRow) -> std::io::Result<()> {
        let p = row.percentiles;
        let g = row.cpu_ghz;
        writeln!(
            self.writer,
            "{},{},{},{:.3},{},{},{},{},{},{},{},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1}",
            row.scenario,
            row.implementation,
            row.operation,
            g,
            p.min, p.p50, p.p95, p.p99, p.p999, p.p9999, p.max, p.mean,
            cycles_to_ns(p.min, g),
            cycles_to_ns(p.p50, g),
            cycles_to_ns(p.p95, g),
            cycles_to_ns(p.p99, g),
            cycles_to_ns(p.p999, g),
            cycles_to_ns(p.p9999, g),
            cycles_to_ns(p.max, g),
            p.mean / g,
        )
    }
}
