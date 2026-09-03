use crate::methodology::{latency::Percentiles, tsc_ticks_to_ns};
use std::fs::{self, File};
use std::io::{BufWriter, Write};

/// One row in the results CSV: a single (scenario, implementation, operation) measurement.
pub struct ResultRow<'a> {
    pub scenario: &'a str,
    pub implementation: &'a str,
    pub operation: &'a str,
    pub tsc_ghz: f64,
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
            "scenario,implementation,operation,tsc_ghz,\
             min_tsc,p50_tsc,p95_tsc,p99_tsc,p999_tsc,p9999_tsc,max_tsc,mean_tsc,\
             min_ns,p50_ns,p95_ns,p99_ns,p999_ns,p9999_ns,max_ns,mean_ns"
        )?;
        println!("Results → {}", path);
        Ok(Self { writer })
    }

    pub fn append(&mut self, row: &ResultRow) -> std::io::Result<()> {
        let p = row.percentiles;
        let g = row.tsc_ghz;
        writeln!(
            self.writer,
            "{},{},{},{:.3},{},{},{},{},{},{},{},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1}",
            row.scenario,
            row.implementation,
            row.operation,
            g,
            p.min,
            p.p50,
            p.p95,
            p.p99,
            p.p999,
            p.p9999,
            p.max,
            p.mean,
            tsc_ticks_to_ns(p.min, g),
            tsc_ticks_to_ns(p.p50, g),
            tsc_ticks_to_ns(p.p95, g),
            tsc_ticks_to_ns(p.p99, g),
            tsc_ticks_to_ns(p.p999, g),
            tsc_ticks_to_ns(p.p9999, g),
            tsc_ticks_to_ns(p.max, g),
            p.mean / g,
        )
    }

    /// Flush all buffered rows so write errors are reported before returning.
    pub fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }
}
