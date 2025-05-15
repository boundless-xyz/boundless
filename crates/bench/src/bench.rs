// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::{
    fs::{create_dir_all, File},
    path::Path,
    path::PathBuf,
};

use anyhow::{bail, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
/// Parameters for the benchmark
pub struct Bench {
    /// The cycle count per request
    pub cycle_count_per_request: u64,
    /// The number of requests to send
    pub requests_count: u32,
    /// delay between requests in seconds
    ///
    /// If this is set to 0, the requests will be sent as fast as possible.
    pub delay: u64,
    /// Timeout for each request in seconds
    pub timeout: u32,
    /// The lock timeout for each request in seconds
    pub lock_timeout: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BenchRow {
    pub request_digest: String,
    pub request_id: String,
    pub cycle_count: u64,
    pub bid_start: u64,
    pub expires_at: u64,
    pub fulfilled_at: Option<u64>,
    pub prover: Option<String>,
    pub latency: Option<u64>,
}

impl BenchRow {
    /// Create a new benchmark row
    pub fn new(
        request_digest: String,
        request_id: String,
        cycle_count: u64,
        bid_start: u64,
        expires_at: u64,
    ) -> Self {
        Self {
            request_digest,
            request_id,
            cycle_count,
            bid_start,
            expires_at,
            fulfilled_at: None,
            prover: None,
            latency: None,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct BenchRows(pub Vec<BenchRow>);

impl BenchRows {
    /// Write the rows out as CSV to `path`.
    pub fn write_csv<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let file = File::create(path)?;
        let mut wtr = csv::Writer::from_writer(file);

        for row in &self.0 {
            wtr.serialize(row)?;
        }
        wtr.flush()?;
        Ok(())
    }

    /// Write the rows out as pretty-printed JSON array to `path`.
    pub fn write_json<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let file = File::create(path)?;
        serde_json::to_writer_pretty(file, &self.0)?;
        Ok(())
    }

    /// Write the rows out as JSON or CSV array to `path`.
    pub fn dump(&self, file_path: Option<PathBuf>, json: bool) -> Result<()> {
        let output: PathBuf = if let Some(p) = &file_path {
            p.clone()
        } else {
            let ext = if json { "json" } else { "csv" };
            let file = format!("bench_{}.{}", Utc::now().timestamp(), ext);
            let path = PathBuf::from("out").join(file);
            if let Some(dir) = path.parent() {
                create_dir_all(dir)?;
            }
            path
        };

        let want_json = json
            || output
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.eq_ignore_ascii_case("json"));

        tracing::info!(
            "Writing benchmark {} to {}",
            if want_json { "JSON" } else { "CSV" },
            output.display()
        );

        if want_json {
            self.write_json(&output)?;
        } else {
            self.write_csv(&output)?;
        }
        Ok(())
    }

    /// Read the rows from a CSV file.
    pub fn from_csv<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = File::open(path)?;
        let mut reader = csv::ReaderBuilder::new().has_headers(true).from_reader(file);
        let mut rows = Vec::new();
        for result in reader.deserialize() {
            let row: BenchRow = result?;
            rows.push(row);
        }
        Ok(Self(rows))
    }

    /// Read the rows from a JSON file.
    pub fn from_json<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = File::open(path)?;
        let rows: Vec<BenchRow> = serde_json::from_reader(file)?;
        Ok(Self(rows))
    }

    /// Read the rows from a file, either JSON or CSV.
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let ext = path.as_ref().extension().and_then(|s| s.to_str()).unwrap().to_lowercase();
        match ext.as_str() {
            "json" => Self::from_json(path),
            "csv" => Self::from_csv(path),
            _ => bail!("Unsupported file format"),
        }
    }
}
