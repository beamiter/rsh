/// Pipeline data carrier — bytes vs typed values.
///
/// Phase 5a keeps it simple: 3 variants, eager. Streaming variants land later.

use crate::value::Value;
use std::io::{self, Write};

#[derive(Debug)]
pub enum PipelineData {
    Empty,
    Bytes(Vec<u8>),
    Values(Vec<Value>),
}

impl PipelineData {
    /// Coerce to `Vec<Value>`. Bytes are parsed as JSON (array or NDJSON).
    /// Returns Err with an exit code on parse failure.
    pub fn into_values(self) -> Result<Vec<Value>, i32> {
        match self {
            PipelineData::Empty => Ok(Vec::new()),
            PipelineData::Values(v) => Ok(v),
            PipelineData::Bytes(b) => {
                let s = String::from_utf8_lossy(&b);
                let s = s.trim();
                if s.is_empty() {
                    return Ok(Vec::new());
                }
                if let Ok(serde_json::Value::Array(arr)) = serde_json::from_str(s) {
                    return Ok(arr.into_iter().map(Value::from_json).collect());
                }
                // Try a single JSON value
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(s) {
                    return Ok(vec![Value::from_json(v)]);
                }
                // NDJSON
                let mut out = Vec::new();
                for line in s.lines() {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<serde_json::Value>(line) {
                        Ok(v) => out.push(Value::from_json(v)),
                        Err(_) => {
                            eprintln!("rsh: cannot parse pipeline input as JSON");
                            return Err(1);
                        }
                    }
                }
                Ok(out)
            }
        }
    }

    /// Serialize to bytes for the fork/pipe boundary.
    /// - `Values` → pretty JSON array
    /// - `Bytes` → passthrough
    /// - `Empty` → empty
    pub fn into_bytes(self) -> Vec<u8> {
        match self {
            PipelineData::Empty => Vec::new(),
            PipelineData::Bytes(b) => b,
            PipelineData::Values(vs) => {
                let arr = serde_json::Value::Array(vs.iter().map(|v| v.to_json()).collect());
                let mut out = serde_json::to_vec_pretty(&arr).unwrap_or_default();
                out.push(b'\n');
                out
            }
        }
    }

    /// Print to stdout. Values use `Display` (table / record / scalar).
    pub fn write_to_stdout(self) -> io::Result<()> {
        match self {
            PipelineData::Empty => Ok(()),
            PipelineData::Bytes(b) => {
                let stdout = io::stdout();
                let mut h = stdout.lock();
                h.write_all(&b)
            }
            PipelineData::Values(vs) => {
                if vs.is_empty() {
                    return Ok(());
                }
                let stdout = io::stdout();
                let mut h = stdout.lock();
                if vs.len() == 1 {
                    write!(h, "{}", vs[0])
                } else if vs.iter().all(|v| v.is_record()) {
                    write!(h, "{}", crate::value::render_table(&vs))
                } else {
                    for v in &vs {
                        writeln!(h, "{}", v.to_display_string())?;
                    }
                    Ok(())
                }
            }
        }
    }
}
