/// Structured data pipeline: JSON-based data processing builtins.

use serde_json::Value;
use std::io::{self, BufRead, Write};

/// Read JSON from stdin (array or newline-delimited objects).
pub fn read_json_stdin() -> Vec<Value> {
    let stdin = io::stdin();
    let mut input = String::new();
    for line in stdin.lock().lines() {
        match line {
            Ok(l) => {
                input.push_str(&l);
                input.push('\n');
            }
            Err(_) => break,
        }
    }
    let input = input.trim();
    if input.is_empty() { return Vec::new(); }

    // Try parsing as JSON array first
    if let Ok(Value::Array(arr)) = serde_json::from_str(input) {
        return arr;
    }

    // Try newline-delimited JSON
    let mut records = Vec::new();
    for line in input.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }
        if let Ok(val) = serde_json::from_str(line) {
            records.push(val);
        }
    }
    records
}

/// Filter records by field comparison.
pub fn filter_where(records: &[Value], field: &str, op: &str, value: &str) -> Vec<Value> {
    records.iter().filter(|record| {
        let field_val = record.get(field);
        match field_val {
            Some(v) => compare_value(v, op, value),
            None => false,
        }
    }).cloned().collect()
}

fn compare_value(v: &Value, op: &str, rhs: &str) -> bool {
    let lhs_str = match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        _ => v.to_string(),
    };

    // Try numeric comparison
    let lhs_num = lhs_str.parse::<f64>().ok();
    let rhs_num = rhs.parse::<f64>().ok();

    match op {
        "==" | "=" => lhs_str == rhs,
        "!=" => lhs_str != rhs,
        ">" | "-gt" => {
            if let (Some(l), Some(r)) = (lhs_num, rhs_num) { l > r }
            else { lhs_str.as_str() > rhs }
        }
        ">=" | "-ge" => {
            if let (Some(l), Some(r)) = (lhs_num, rhs_num) { l >= r }
            else { lhs_str.as_str() >= rhs }
        }
        "<" | "-lt" => {
            if let (Some(l), Some(r)) = (lhs_num, rhs_num) { l < r }
            else { lhs_str.as_str() < rhs }
        }
        "<=" | "-le" => {
            if let (Some(l), Some(r)) = (lhs_num, rhs_num) { l <= r }
            else { lhs_str.as_str() <= rhs }
        }
        "=~" => lhs_str.contains(rhs),
        _ => false,
    }
}

/// Sort records by a field.
pub fn sort_by(records: &mut [Value], field: &str, reverse: bool) {
    records.sort_by(|a, b| {
        let va = a.get(field);
        let vb = b.get(field);
        let cmp = compare_json_values(va, vb);
        if reverse { cmp.reverse() } else { cmp }
    });
}

fn compare_json_values(a: Option<&Value>, b: Option<&Value>) -> std::cmp::Ordering {
    match (a, b) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
        (Some(va), Some(vb)) => {
            // Try numeric comparison
            if let (Some(na), Some(nb)) = (va.as_f64(), vb.as_f64()) {
                return na.partial_cmp(&nb).unwrap_or(std::cmp::Ordering::Equal);
            }
            let sa = value_to_string(va);
            let sb = value_to_string(vb);
            sa.cmp(&sb)
        }
    }
}

fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => String::new(),
        _ => v.to_string(),
    }
}

/// Project only selected fields.
pub fn select_fields(records: &[Value], fields: &[&str]) -> Vec<Value> {
    records.iter().map(|record| {
        if let Value::Object(map) = record {
            let mut new_map = serde_json::Map::new();
            for &field in fields {
                if let Some(v) = map.get(field) {
                    new_map.insert(field.to_string(), v.clone());
                }
            }
            Value::Object(new_map)
        } else {
            record.clone()
        }
    }).collect()
}

/// Pretty-print records as an aligned table.
pub fn to_table(records: &[Value]) -> String {
    if records.is_empty() { return String::new(); }

    // Collect all field names (preserving order from first record)
    let mut columns: Vec<String> = Vec::new();
    for record in records {
        if let Value::Object(map) = record {
            for key in map.keys() {
                if !columns.contains(key) {
                    columns.push(key.clone());
                }
            }
        }
    }
    if columns.is_empty() { return String::new(); }

    // Build rows
    let mut rows: Vec<Vec<String>> = Vec::new();
    rows.push(columns.clone()); // header
    for record in records {
        let row: Vec<String> = columns.iter().map(|col| {
            record.get(col).map(|v| value_to_string(v)).unwrap_or_default()
        }).collect();
        rows.push(row);
    }

    // Compute column widths
    let mut widths: Vec<usize> = columns.iter().map(|c| c.len()).collect();
    for row in &rows[1..] {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(cell.len());
            }
        }
    }

    // Render
    let mut output = String::new();
    for (ri, row) in rows.iter().enumerate() {
        for (i, cell) in row.iter().enumerate() {
            if i > 0 { output.push_str("  "); }
            let w = widths.get(i).copied().unwrap_or(0);
            output.push_str(&format!("{:<width$}", cell, width = w));
        }
        output.push('\n');
        // Separator after header
        if ri == 0 {
            for (i, w) in widths.iter().enumerate() {
                if i > 0 { output.push_str("  "); }
                output.push_str(&"-".repeat(*w));
            }
            output.push('\n');
        }
    }
    output
}

/// Write JSON array to stdout.
pub fn write_json_stdout(records: &[Value]) {
    let out = serde_json::to_string_pretty(records).unwrap_or_default();
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    handle.write_all(out.as_bytes()).ok();
    handle.write_all(b"\n").ok();
}
