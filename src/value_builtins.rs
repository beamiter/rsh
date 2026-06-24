/// Value-aware (in-process) builtins.
///
/// Each fn takes `PipelineData` input and returns `PipelineData`, threading
/// typed `Value`s through the pipeline without forking.

use crate::environment::ShellState;
use crate::pipeline_data::PipelineData;
use crate::value::{render_table, ClosureData, Value};
use indexmap::IndexMap;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Arc;

pub type ValueBuiltin = fn(PipelineData, &[String], &mut ShellState) -> Result<PipelineData, i32>;

pub static VALUE_BUILTINS: Lazy<HashMap<&'static str, ValueBuiltin>> = Lazy::new(|| {
    let mut m: HashMap<&'static str, ValueBuiltin> = HashMap::new();
    m.insert("from-json", vb_from_json);
    m.insert("to-json", vb_to_json);
    m.insert("to-table", vb_to_table);
    m.insert("where", vb_where);
    m.insert("sort-by", vb_sort_by);
    m.insert("select", vb_select);
    m.insert("group-by", vb_group_by);
    m.insert("unique", vb_unique);
    m.insert("count", vb_count);
    m.insert("math", vb_math);
    m.insert("from-csv", vb_from_csv);
    m.insert("length", vb_length);
    m.insert("first", vb_first);
    m.insert("last", vb_last);
    m.insert("reverse", vb_reverse);
    m.insert("each", vb_each);
    m.insert("from-yaml", vb_from_yaml);
    m.insert("to-yaml", vb_to_yaml);
    m.insert("from-toml", vb_from_toml);
    m.insert("to-toml", vb_to_toml);
    m.insert("from-xml", vb_from_xml);
    m.insert("to-xml", vb_to_xml);
    m.insert("ls", vb_ls);
    m.insert("ps", vb_ps);
    m.insert("open", vb_open);
    m.insert("save", vb_save);
    m.insert("lines", vb_lines);
    m.insert("split", vb_split);
    m.insert("parse", vb_parse);
    m.insert("str", vb_str);
    m.insert("get", vb_get);
    m.insert("update", vb_update);
    m.insert("insert", vb_insert);
    m.insert("reject", vb_reject);
    m.insert("wrap", vb_wrap);
    m.insert("flatten", vb_flatten);
    m.insert("into", vb_into);
    m.insert("range", vb_range);
    m
});

/// Builtins that are *always* treated as value-aware (override bash/external).
/// `ls`/`ps` are intentionally excluded so a bare `ls` still runs the system
/// command — they're only value-aware inside multi-command pipelines (see
/// `is_value_aware_in_pipeline`).
pub fn is_value_aware(name: &str) -> bool {
    VALUE_BUILTINS.contains_key(name) && !is_context_only(name)
}

/// Used by the pipeline pre-flight check: ls/ps participate as value-aware
/// only when there's at least one other command in the pipeline (e.g.
/// `ls | where ...`). For a bare `ls`, dispatch falls through to external.
pub fn is_value_aware_in_pipeline(name: &str) -> bool {
    VALUE_BUILTINS.contains_key(name)
}

fn is_context_only(name: &str) -> bool {
    matches!(name, "ls" | "ps")
}

// ---------------------------------------------------------------------------
// Individual builtins
// ---------------------------------------------------------------------------

fn vb_from_json(input: PipelineData, _args: &[String], _state: &mut ShellState) -> Result<PipelineData, i32> {
    let vs = input.into_values()?;
    Ok(PipelineData::Values(vs))
}

fn vb_to_json(input: PipelineData, _args: &[String], _state: &mut ShellState) -> Result<PipelineData, i32> {
    let vs = input.into_values()?;
    let bytes = PipelineData::Values(vs).into_bytes();
    Ok(PipelineData::Bytes(bytes))
}

fn vb_to_table(input: PipelineData, _args: &[String], _state: &mut ShellState) -> Result<PipelineData, i32> {
    let vs = input.into_values()?;
    Ok(PipelineData::Bytes(render_table(&vs).into_bytes()))
}

fn vb_where(input: PipelineData, args: &[String], state: &mut ShellState) -> Result<PipelineData, i32> {
    // Closure form: `where {|r| ...}` or `where $f` where f is a Value::Closure.
    if let Some(closure) = lookup_closure(args.first(), state) {
        let vs = input.into_values()?;
        let mut out = Vec::with_capacity(vs.len());
        for v in vs {
            let result = crate::executor::apply_closure(&closure, std::slice::from_ref(&v), state)?;
            if is_truthy(&result) {
                out.push(v);
            }
        }
        return Ok(PipelineData::Values(out));
    }
    if args.len() < 3 {
        eprintln!("Usage: where <field> <op> <value> | where {{|r| ...}}");
        return Err(1);
    }
    let field = &args[0];
    let op = &args[1];
    let rhs = &args[2];
    let vs = input.into_values()?;
    let out: Vec<Value> = vs
        .into_iter()
        .filter(|v| v.get(field).map(|fv| compare(fv, op, rhs)).unwrap_or(false))
        .collect();
    Ok(PipelineData::Values(out))
}

fn vb_each(input: PipelineData, args: &[String], state: &mut ShellState) -> Result<PipelineData, i32> {
    let closure = match lookup_closure(args.first(), state) {
        Some(c) => c,
        None => {
            eprintln!("Usage: each {{|x| ...}}");
            return Err(1);
        }
    };
    let vs = input.into_values()?;
    let mut out = Vec::with_capacity(vs.len());
    for v in vs {
        let r = crate::executor::apply_closure(&closure, std::slice::from_ref(&v), state)?;
        out.push(r);
    }
    Ok(PipelineData::Values(out))
}

fn vb_sort_by(input: PipelineData, args: &[String], _state: &mut ShellState) -> Result<PipelineData, i32> {
    if args.is_empty() {
        eprintln!("Usage: sort-by <field> [-r]");
        return Err(1);
    }
    let field = &args[0];
    let reverse = args.get(1).map(|s| s == "-r").unwrap_or(false);
    let mut vs = input.into_values()?;
    vs.sort_by(|a, b| {
        let va = a.get(field);
        let vb = b.get(field);
        let cmp = compare_opt(va, vb);
        if reverse { cmp.reverse() } else { cmp }
    });
    Ok(PipelineData::Values(vs))
}

fn vb_select(input: PipelineData, args: &[String], _state: &mut ShellState) -> Result<PipelineData, i32> {
    if args.is_empty() {
        eprintln!("Usage: select <field1> [field2] ...");
        return Err(1);
    }
    let vs = input.into_values()?;
    let out: Vec<Value> = vs
        .into_iter()
        .map(|v| match v {
            Value::Record(map) => {
                let mut new_map = IndexMap::new();
                for f in args {
                    if let Some(val) = map.get(f) {
                        new_map.insert(f.clone(), val.clone());
                    }
                }
                Value::Record(new_map)
            }
            other => other,
        })
        .collect();
    Ok(PipelineData::Values(out))
}

fn vb_group_by(input: PipelineData, args: &[String], _state: &mut ShellState) -> Result<PipelineData, i32> {
    if args.is_empty() {
        eprintln!("Usage: group-by <field>");
        return Err(1);
    }
    let field = &args[0];
    let vs = input.into_values()?;
    let mut groups: IndexMap<String, Vec<Value>> = IndexMap::new();
    for v in vs {
        let key = v.get(field).map(|fv| fv.to_display_string()).unwrap_or_else(|| "null".to_string());
        groups.entry(key).or_default().push(v);
    }
    let mut rec = IndexMap::new();
    for (k, items) in groups {
        rec.insert(k, Value::List(items));
    }
    Ok(PipelineData::Values(vec![Value::Record(rec)]))
}

fn vb_unique(input: PipelineData, args: &[String], _state: &mut ShellState) -> Result<PipelineData, i32> {
    let field = args.first().map(|s| s.as_str());
    let vs = input.into_values()?;
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for v in vs {
        let key = match field {
            Some(f) => v.get(f).map(|fv| fv.to_display_string()).unwrap_or_default(),
            None => v.to_display_string(),
        };
        if seen.insert(key) {
            out.push(v);
        }
    }
    Ok(PipelineData::Values(out))
}

fn vb_count(input: PipelineData, _args: &[String], _state: &mut ShellState) -> Result<PipelineData, i32> {
    let vs = input.into_values()?;
    Ok(PipelineData::Values(vec![Value::Int(vs.len() as i64)]))
}

fn vb_math(input: PipelineData, args: &[String], _state: &mut ShellState) -> Result<PipelineData, i32> {
    if args.is_empty() {
        eprintln!("Usage: math <sum|avg|min|max|stddev> [field]");
        return Err(1);
    }
    let op = &args[0];
    let field = args.get(1);
    let vs = input.into_values()?;
    // Two modes: `math sum field` (extract field from records) OR
    // `math sum` (pipeline is a list of numbers).
    let nums: Vec<f64> = if let Some(f) = field {
        vs.iter().filter_map(|v| v.get(f).and_then(|fv| fv.as_f64())).collect()
    } else {
        vs.iter().filter_map(|v| v.as_f64()).collect()
    };
    if nums.is_empty() {
        eprintln!("math: no numeric values");
        return Err(1);
    }
    let r = match op.as_str() {
        "sum" => nums.iter().sum::<f64>(),
        "avg" | "mean" => nums.iter().sum::<f64>() / nums.len() as f64,
        "min" => nums.iter().copied().fold(f64::INFINITY, f64::min),
        "max" => nums.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        "stddev" | "std" => {
            let mean = nums.iter().sum::<f64>() / nums.len() as f64;
            let var = nums.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / nums.len() as f64;
            var.sqrt()
        }
        _ => {
            eprintln!("math: unknown op '{}'", op);
            return Err(1);
        }
    };
    let out = if r.fract() == 0.0 && r.abs() < 1e16 {
        Value::Int(r as i64)
    } else {
        Value::Float(r)
    };
    Ok(PipelineData::Values(vec![out]))
}

fn vb_from_csv(input: PipelineData, _args: &[String], _state: &mut ShellState) -> Result<PipelineData, i32> {
    let bytes = match input {
        PipelineData::Empty => Vec::new(),
        PipelineData::Bytes(b) => b,
        PipelineData::Values(_) => {
            eprintln!("from-csv: expected text input");
            return Err(1);
        }
    };
    let s = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = s.lines().collect();
    if lines.is_empty() {
        return Ok(PipelineData::Values(Vec::new()));
    }
    let headers = parse_csv_line(lines[0]);
    let mut out = Vec::new();
    for line in &lines[1..] {
        if line.trim().is_empty() {
            continue;
        }
        let cells = parse_csv_line(line);
        let mut rec = IndexMap::new();
        for (i, h) in headers.iter().enumerate() {
            let cell = cells.get(i).cloned().unwrap_or_default();
            // Try numeric coercion
            let val = if let Ok(i) = cell.parse::<i64>() {
                Value::Int(i)
            } else if let Ok(f) = cell.parse::<f64>() {
                Value::Float(f)
            } else {
                Value::String(cell)
            };
            rec.insert(h.clone(), val);
        }
        out.push(Value::Record(rec));
    }
    Ok(PipelineData::Values(out))
}

fn vb_length(input: PipelineData, _args: &[String], _state: &mut ShellState) -> Result<PipelineData, i32> {
    let vs = input.into_values()?;
    Ok(PipelineData::Values(vec![Value::Int(vs.len() as i64)]))
}

fn vb_first(input: PipelineData, args: &[String], _state: &mut ShellState) -> Result<PipelineData, i32> {
    let n: usize = args.first().and_then(|s| s.parse().ok()).unwrap_or(1);
    let mut vs = input.into_values()?;
    vs.truncate(n);
    Ok(PipelineData::Values(vs))
}

fn vb_last(input: PipelineData, args: &[String], _state: &mut ShellState) -> Result<PipelineData, i32> {
    let n: usize = args.first().and_then(|s| s.parse().ok()).unwrap_or(1);
    let vs = input.into_values()?;
    let start = vs.len().saturating_sub(n);
    Ok(PipelineData::Values(vs[start..].to_vec()))
}

fn vb_reverse(input: PipelineData, _args: &[String], _state: &mut ShellState) -> Result<PipelineData, i32> {
    let mut vs = input.into_values()?;
    vs.reverse();
    Ok(PipelineData::Values(vs))
}

// ---------------------------------------------------------------------------
// Phase 5d — format converters
// ---------------------------------------------------------------------------

fn vb_from_yaml(input: PipelineData, _args: &[String], _state: &mut ShellState) -> Result<PipelineData, i32> {
    let bytes = match input {
        PipelineData::Empty => Vec::new(),
        PipelineData::Bytes(b) => b,
        PipelineData::Values(_) => {
            eprintln!("from-yaml: expected text input");
            return Err(1);
        }
    };
    // serde_yaml deserializes into serde_json::Value first (lossy-typed but JSON-compatible).
    let yval: serde_yaml::Value = serde_yaml::from_slice(&bytes).map_err(|e| {
        eprintln!("from-yaml: {}", e);
        1
    })?;
    let jval = yaml_to_json(yval);
    let v = Value::from_json(jval);
    let out = match v {
        Value::List(items) => items,
        other => vec![other],
    };
    Ok(PipelineData::Values(out))
}

fn vb_to_yaml(input: PipelineData, _args: &[String], _state: &mut ShellState) -> Result<PipelineData, i32> {
    let vs = input.into_values()?;
    let jval = if vs.len() == 1 {
        vs.into_iter().next().unwrap().to_json()
    } else {
        Value::List(vs).to_json()
    };
    let s = serde_yaml::to_string(&jval).map_err(|e| {
        eprintln!("to-yaml: {}", e);
        1
    })?;
    Ok(PipelineData::Bytes(s.into_bytes()))
}

fn vb_from_toml(input: PipelineData, _args: &[String], _state: &mut ShellState) -> Result<PipelineData, i32> {
    let bytes = match input {
        PipelineData::Empty => Vec::new(),
        PipelineData::Bytes(b) => b,
        PipelineData::Values(_) => {
            eprintln!("from-toml: expected text input");
            return Err(1);
        }
    };
    let s = String::from_utf8_lossy(&bytes);
    let tval: toml::Value = toml::from_str(&s).map_err(|e| {
        eprintln!("from-toml: {}", e);
        1
    })?;
    let v = Value::from_json(toml_to_json(tval));
    Ok(PipelineData::Values(vec![v]))
}

fn vb_to_toml(input: PipelineData, _args: &[String], _state: &mut ShellState) -> Result<PipelineData, i32> {
    let vs = input.into_values()?;
    if vs.len() != 1 || !vs[0].is_record() {
        eprintln!("to-toml: expects a single record");
        return Err(1);
    }
    let jval = vs.into_iter().next().unwrap().to_json();
    let s = toml::to_string(&jval).map_err(|e| {
        eprintln!("to-toml: {}", e);
        1
    })?;
    Ok(PipelineData::Bytes(s.into_bytes()))
}

fn vb_from_xml(input: PipelineData, _args: &[String], _state: &mut ShellState) -> Result<PipelineData, i32> {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;

    let bytes = match input {
        PipelineData::Empty => Vec::new(),
        PipelineData::Bytes(b) => b,
        PipelineData::Values(_) => {
            eprintln!("from-xml: expected text input");
            return Err(1);
        }
    };
    let mut reader = Reader::from_reader(bytes.as_slice());
    reader.config_mut().trim_text(true);
    // Build a nested record: each element is {tag, attrs, content, children}.
    // Stack-based: top of stack is the open element we're populating.
    struct Open {
        tag: String,
        attrs: IndexMap<String, Value>,
        children: Vec<Value>,
        text: String,
    }
    let mut stack: Vec<Open> = Vec::new();
    let mut top_level: Vec<Value> = Vec::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(ev @ Event::Start(_)) | Ok(ev @ Event::Empty(_)) => {
                let is_empty = matches!(ev, Event::Empty(_));
                let e = match &ev {
                    Event::Start(e) | Event::Empty(e) => e,
                    _ => unreachable!(),
                };
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                let mut attrs = IndexMap::new();
                for a in e.attributes().flatten() {
                    let k = String::from_utf8_lossy(a.key.as_ref()).to_string();
                    let v = String::from_utf8_lossy(&a.value).to_string();
                    attrs.insert(k, Value::String(v));
                }
                let opened = Open { tag, attrs, children: Vec::new(), text: String::new() };
                stack.push(opened);
                if is_empty {
                    // Synthesize an End event by popping immediately.
                    if let Some(o) = stack.pop() {
                        let mut rec = IndexMap::new();
                        rec.insert("tag".to_string(), Value::String(o.tag));
                        if !o.attrs.is_empty() {
                            rec.insert("attrs".to_string(), Value::Record(o.attrs));
                        }
                        let v = Value::Record(rec);
                        match stack.last_mut() {
                            Some(parent) => parent.children.push(v),
                            None => top_level.push(v),
                        }
                    }
                }
            }
            Ok(Event::Text(t)) => {
                let s = String::from_utf8_lossy(&t).to_string();
                if let Some(o) = stack.last_mut() {
                    o.text.push_str(&s);
                }
            }
            Ok(Event::End(_)) => {
                if let Some(o) = stack.pop() {
                    let mut rec = IndexMap::new();
                    rec.insert("tag".to_string(), Value::String(o.tag));
                    if !o.attrs.is_empty() {
                        rec.insert("attrs".to_string(), Value::Record(o.attrs));
                    }
                    let txt = o.text.trim().to_string();
                    if !txt.is_empty() {
                        rec.insert("text".to_string(), Value::String(txt));
                    }
                    if !o.children.is_empty() {
                        rec.insert("children".to_string(), Value::List(o.children));
                    }
                    let v = Value::Record(rec);
                    match stack.last_mut() {
                        Some(parent) => parent.children.push(v),
                        None => top_level.push(v),
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                eprintln!("from-xml: {}", e);
                return Err(1);
            }
            _ => {}
        }
        buf.clear();
    }
    Ok(PipelineData::Values(top_level))
}

fn vb_to_xml(input: PipelineData, _args: &[String], _state: &mut ShellState) -> Result<PipelineData, i32> {
    let vs = input.into_values()?;
    let mut out = String::new();
    for v in &vs {
        render_xml_node(v, &mut out);
    }
    Ok(PipelineData::Bytes(out.into_bytes()))
}

fn render_xml_node(v: &Value, out: &mut String) {
    let rec = match v {
        Value::Record(r) => r,
        other => {
            out.push_str(&xml_escape(&other.to_display_string()));
            return;
        }
    };
    let tag = rec.get("tag").map(|t| t.to_display_string()).unwrap_or_else(|| "item".to_string());
    out.push('<');
    out.push_str(&tag);
    if let Some(Value::Record(attrs)) = rec.get("attrs") {
        for (k, v) in attrs {
            out.push(' ');
            out.push_str(k);
            out.push('=');
            out.push('"');
            out.push_str(&xml_escape(&v.to_display_string()));
            out.push('"');
        }
    }
    let text = rec.get("text").map(|t| t.to_display_string());
    let children = match rec.get("children") {
        Some(Value::List(items)) => Some(items),
        _ => None,
    };
    if text.is_none() && children.is_none() {
        out.push_str("/>");
        return;
    }
    out.push('>');
    if let Some(t) = text {
        out.push_str(&xml_escape(&t));
    }
    if let Some(items) = children {
        for c in items {
            render_xml_node(c, out);
        }
    }
    out.push_str("</");
    out.push_str(&tag);
    out.push('>');
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn yaml_to_json(y: serde_yaml::Value) -> serde_json::Value {
    use serde_yaml::Value as Y;
    match y {
        Y::Null => serde_json::Value::Null,
        Y::Bool(b) => serde_json::Value::Bool(b),
        Y::Number(n) => {
            if let Some(i) = n.as_i64() {
                serde_json::Value::Number(i.into())
            } else if let Some(f) = n.as_f64() {
                serde_json::Number::from_f64(f).map(serde_json::Value::Number).unwrap_or(serde_json::Value::Null)
            } else {
                serde_json::Value::Null
            }
        }
        Y::String(s) => serde_json::Value::String(s),
        Y::Sequence(seq) => serde_json::Value::Array(seq.into_iter().map(yaml_to_json).collect()),
        Y::Mapping(map) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in map {
                let key = match k {
                    Y::String(s) => s,
                    other => serde_yaml::to_string(&other).unwrap_or_default().trim().to_string(),
                };
                obj.insert(key, yaml_to_json(v));
            }
            serde_json::Value::Object(obj)
        }
        Y::Tagged(t) => yaml_to_json(t.value),
    }
}

fn toml_to_json(t: toml::Value) -> serde_json::Value {
    use toml::Value as T;
    match t {
        T::String(s) => serde_json::Value::String(s),
        T::Integer(i) => serde_json::Value::Number(i.into()),
        T::Float(f) => serde_json::Number::from_f64(f).map(serde_json::Value::Number).unwrap_or(serde_json::Value::Null),
        T::Boolean(b) => serde_json::Value::Bool(b),
        T::Datetime(d) => serde_json::Value::String(d.to_string()),
        T::Array(arr) => serde_json::Value::Array(arr.into_iter().map(toml_to_json).collect()),
        T::Table(tbl) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in tbl {
                obj.insert(k, toml_to_json(v));
            }
            serde_json::Value::Object(obj)
        }
    }
}

// ---------------------------------------------------------------------------
// Phase 5d — structured `ls` and `ps`
// ---------------------------------------------------------------------------

fn vb_ls(_input: PipelineData, args: &[String], _state: &mut ShellState) -> Result<PipelineData, i32> {
    use std::fs;
    let path = args.first().map(|s| s.as_str()).unwrap_or(".");
    let cwd = std::path::PathBuf::from(path);
    let entries = match fs::read_dir(&cwd) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("ls: {}: {}", path, e);
            return Err(1);
        }
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let md = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let kind = if md.is_dir() { "dir" } else if md.is_symlink() { "link" } else { "file" };
        let size = md.len() as i64;
        let modified = md.modified().ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let mut rec = IndexMap::new();
        rec.insert("name".to_string(), Value::String(name));
        rec.insert("type".to_string(), Value::String(kind.to_string()));
        rec.insert("size".to_string(), Value::Int(size));
        rec.insert("modified".to_string(), Value::Int(modified));
        out.push(Value::Record(rec));
    }
    out.sort_by(|a, b| {
        a.get("name").map(|v| v.to_display_string())
            .cmp(&b.get("name").map(|v| v.to_display_string()))
    });
    Ok(PipelineData::Values(out))
}

fn vb_ps(_input: PipelineData, _args: &[String], _state: &mut ShellState) -> Result<PipelineData, i32> {
    use std::fs;
    let mut out = Vec::new();
    let entries = match fs::read_dir("/proc") {
        Ok(e) => e,
        Err(e) => {
            eprintln!("ps: /proc: {}", e);
            return Err(1);
        }
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let pid_str = name.to_string_lossy();
        let pid: i64 = match pid_str.parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        let stat_path = entry.path().join("stat");
        let stat = match fs::read_to_string(&stat_path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        // /proc/<pid>/stat: pid (comm) state ppid ...
        // comm can contain spaces & parens; find last ')' to split safely.
        let close = match stat.rfind(')') {
            Some(i) => i,
            None => continue,
        };
        let comm = &stat[stat.find('(').map(|i| i + 1).unwrap_or(0)..close];
        let rest: Vec<&str> = stat[close + 2..].split_whitespace().collect();
        if rest.len() < 2 {
            continue;
        }
        let state_ch = rest[0].to_string();
        let ppid: i64 = rest[1].parse().unwrap_or(0);
        let cmdline = fs::read_to_string(entry.path().join("cmdline"))
            .ok()
            .map(|s| s.replace('\0', " ").trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| comm.to_string());
        let mut rec = IndexMap::new();
        rec.insert("pid".to_string(), Value::Int(pid));
        rec.insert("ppid".to_string(), Value::Int(ppid));
        rec.insert("state".to_string(), Value::String(state_ch));
        rec.insert("name".to_string(), Value::String(comm.to_string()));
        rec.insert("cmd".to_string(), Value::String(cmdline));
        out.push(Value::Record(rec));
    }
    out.sort_by_key(|v| v.get("pid").and_then(|x| x.as_f64()).map(|f| f as i64).unwrap_or(0));
    Ok(PipelineData::Values(out))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve a closure reference: either the inline-expansion sentinel
/// `\x01rsh-closure:<idx>\x02` or a let-bound `Value::Closure`.
fn lookup_closure(arg: Option<&String>, state: &ShellState) -> Option<Arc<ClosureData>> {
    let a = arg?;
    if let Some(rest) = a.strip_prefix('\u{01}').and_then(|s| s.strip_suffix('\u{02}')) {
        if let Some(idx) = rest.strip_prefix("rsh-closure:").and_then(|s| s.parse::<usize>().ok()) {
            return state.inline_closures.get(idx).cloned();
        }
    }
    if let Some(Value::Closure(c)) = state.let_vars.get(a) {
        return Some(c.clone());
    }
    None
}

fn is_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Int(i) => *i != 0,
        Value::Float(f) => *f != 0.0,
        Value::String(s) => !s.is_empty(),
        Value::List(items) => !items.is_empty(),
        Value::Record(map) => !map.is_empty(),
        Value::Binary(b) => !b.is_empty(),
        Value::Closure(_) => true,
    }
}

fn compare(v: &Value, op: &str, rhs: &str) -> bool {
    let lhs_str = v.to_display_string();
    let lhs_num = v.as_f64();
    let rhs_num = rhs.parse::<f64>().ok();
    match op {
        "==" | "=" => lhs_str == rhs,
        "!=" => lhs_str != rhs,
        ">" | "-gt" => match (lhs_num, rhs_num) {
            (Some(l), Some(r)) => l > r,
            _ => lhs_str.as_str() > rhs,
        },
        ">=" | "-ge" => match (lhs_num, rhs_num) {
            (Some(l), Some(r)) => l >= r,
            _ => lhs_str.as_str() >= rhs,
        },
        "<" | "-lt" => match (lhs_num, rhs_num) {
            (Some(l), Some(r)) => l < r,
            _ => lhs_str.as_str() < rhs,
        },
        "<=" | "-le" => match (lhs_num, rhs_num) {
            (Some(l), Some(r)) => l <= r,
            _ => lhs_str.as_str() <= rhs,
        },
        "=~" => lhs_str.contains(rhs),
        _ => false,
    }
}

fn compare_opt(a: Option<&Value>, b: Option<&Value>) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(va), Some(vb)) => {
            if let (Some(na), Some(nb)) = (va.as_f64(), vb.as_f64()) {
                return na.partial_cmp(&nb).unwrap_or(Ordering::Equal);
            }
            va.to_display_string().cmp(&vb.to_display_string())
        }
    }
}

fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    cur.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                cur.push(c);
            }
        } else {
            match c {
                '"' => in_quotes = true,
                ',' => {
                    fields.push(cur.trim().to_string());
                    cur = String::new();
                }
                _ => cur.push(c),
            }
        }
    }
    fields.push(cur.trim().to_string());
    fields
}

// ---------------------------------------------------------------------------
// Phase 6a — `open` / `save` (file I/O bridges to converters)
// ---------------------------------------------------------------------------

fn detect_format(path: &str) -> &'static str {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".json") { "json" }
    else if lower.ends_with(".yaml") || lower.ends_with(".yml") { "yaml" }
    else if lower.ends_with(".toml") { "toml" }
    else if lower.ends_with(".xml") { "xml" }
    else if lower.ends_with(".csv") { "csv" }
    else if lower.ends_with(".txt") || lower.ends_with(".md") || lower.ends_with(".log") { "text" }
    else { "raw" }
}

fn vb_open(_input: PipelineData, args: &[String], state: &mut ShellState) -> Result<PipelineData, i32> {
    let path = match args.first() {
        Some(p) => p,
        None => {
            eprintln!("open: missing path");
            return Err(1);
        }
    };
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("open: {}: {}", path, e);
            return Err(1);
        }
    };
    let fmt = detect_format(path);
    let input = PipelineData::Bytes(bytes);
    match fmt {
        "json" => vb_from_json(input, &[], state),
        "yaml" => vb_from_yaml(input, &[], state),
        "toml" => vb_from_toml(input, &[], state),
        "xml"  => vb_from_xml(input, &[], state),
        "csv"  => vb_from_csv(input, &[], state),
        // Plain text: surface as a single String value so it can be piped to
        // `lines` / `parse` / `str ...` in Phase 6b.
        "text" => {
            let s = match input {
                PipelineData::Bytes(b) => String::from_utf8_lossy(&b).into_owned(),
                _ => String::new(),
            };
            Ok(PipelineData::Values(vec![Value::String(s)]))
        }
        // Unknown extension: passthrough bytes — external commands keep working.
        _ => Ok(input),
    }
}

fn vb_save(input: PipelineData, args: &[String], state: &mut ShellState) -> Result<PipelineData, i32> {
    let mut append = false;
    let mut path: Option<&str> = None;
    for a in args {
        if a == "--append" || a == "-a" { append = true; }
        else if path.is_none() { path = Some(a.as_str()); }
    }
    let path = match path {
        Some(p) => p,
        None => {
            eprintln!("save: missing path");
            return Err(1);
        }
    };
    let fmt = detect_format(path);
    // Serialize input into bytes via the matching converter, then write.
    let bytes_data = match fmt {
        "json" => vb_to_json(input, &[], state)?,
        "yaml" => vb_to_yaml(input, &[], state)?,
        "toml" => vb_to_toml(input, &[], state)?,
        "xml"  => vb_to_xml(input, &[], state)?,
        // text/raw/csv-fallback: write Bytes as-is; for Values, render Display.
        _ => match input {
            PipelineData::Bytes(b) => PipelineData::Bytes(b),
            PipelineData::Empty => PipelineData::Bytes(Vec::new()),
            PipelineData::Values(vs) => {
                let mut s = String::new();
                for (i, v) in vs.iter().enumerate() {
                    if i > 0 { s.push('\n'); }
                    s.push_str(&v.to_display_string());
                }
                PipelineData::Bytes(s.into_bytes())
            }
        },
    };
    let bytes = match bytes_data {
        PipelineData::Bytes(b) => b,
        _ => Vec::new(),
    };
    use std::fs::OpenOptions;
    use std::io::Write as IoWrite;
    let mut opts = OpenOptions::new();
    opts.write(true).create(true);
    if append { opts.append(true); } else { opts.truncate(true); }
    match opts.open(path) {
        Ok(mut f) => {
            if let Err(e) = f.write_all(&bytes) {
                eprintln!("save: {}: {}", path, e);
                return Err(1);
            }
        }
        Err(e) => {
            eprintln!("save: {}: {}", path, e);
            return Err(1);
        }
    }
    Ok(PipelineData::Empty)
}

// ---------------------------------------------------------------------------
// Phase 6b — text → structured bridges
// ---------------------------------------------------------------------------

fn input_as_string(input: PipelineData) -> Result<String, i32> {
    match input {
        PipelineData::Empty => Ok(String::new()),
        PipelineData::Bytes(b) => Ok(String::from_utf8_lossy(&b).into_owned()),
        PipelineData::Values(vs) => {
            if vs.len() == 1 {
                if let Value::String(s) = &vs[0] { return Ok(s.clone()); }
            }
            let mut s = String::new();
            for (i, v) in vs.iter().enumerate() {
                if i > 0 { s.push('\n'); }
                s.push_str(&v.to_display_string());
            }
            Ok(s)
        }
    }
}

fn vb_lines(input: PipelineData, _args: &[String], _state: &mut ShellState) -> Result<PipelineData, i32> {
    let s = input_as_string(input)?;
    // Drop trailing empty line so `echo "a\nb\n"` yields ["a","b"], not ["a","b",""].
    let mut out: Vec<Value> = s.split('\n').map(|l| Value::String(l.to_string())).collect();
    if let Some(Value::String(last)) = out.last() {
        if last.is_empty() {
            out.pop();
        }
    }
    Ok(PipelineData::Values(out))
}

fn vb_split(input: PipelineData, args: &[String], _state: &mut ShellState) -> Result<PipelineData, i32> {
    let sub = match args.first().map(|s| s.as_str()) {
        Some("row") => "row",
        Some("column") => "column",
        _ => {
            eprintln!("split: expected 'row' or 'column' subcommand");
            return Err(1);
        }
    };
    let rest = &args[1..];
    let sep = match rest.first() {
        Some(s) => s.clone(),
        None => {
            eprintln!("split {}: missing separator", sub);
            return Err(1);
        }
    };
    if sub == "row" {
        // String/Bytes → split by sep into List[String]; List[String] → per-element split (flatten).
        match input {
            PipelineData::Empty => Ok(PipelineData::Values(Vec::new())),
            PipelineData::Bytes(b) => {
                let s = String::from_utf8_lossy(&b);
                Ok(PipelineData::Values(s.split(sep.as_str()).map(|x| Value::String(x.to_string())).collect()))
            }
            PipelineData::Values(vs) => {
                let mut out = Vec::new();
                for v in vs {
                    let s = v.to_display_string();
                    for piece in s.split(sep.as_str()) {
                        out.push(Value::String(piece.to_string()));
                    }
                }
                Ok(PipelineData::Values(out))
            }
        }
    } else {
        // split column SEP NAME...
        let names: Vec<String> = rest[1..].to_vec();
        let mut process = |line: &str| -> Value {
            let parts: Vec<&str> = line.split(sep.as_str()).collect();
            let mut rec = IndexMap::new();
            for (i, p) in parts.iter().enumerate() {
                let key = names.get(i).cloned().unwrap_or_else(|| format!("column{}", i + 1));
                rec.insert(key, Value::String(p.to_string()));
            }
            Value::Record(rec)
        };
        match input {
            PipelineData::Empty => Ok(PipelineData::Values(Vec::new())),
            PipelineData::Bytes(b) => {
                let s = String::from_utf8_lossy(&b).into_owned();
                Ok(PipelineData::Values(vec![process(&s)]))
            }
            PipelineData::Values(vs) => {
                let out: Vec<Value> = vs.iter().map(|v| process(&v.to_display_string())).collect();
                Ok(PipelineData::Values(out))
            }
        }
    }
}

fn vb_parse(input: PipelineData, args: &[String], _state: &mut ShellState) -> Result<PipelineData, i32> {
    let pat = match args.first() {
        Some(p) => p,
        None => {
            eprintln!("parse: missing template");
            return Err(1);
        }
    };
    // Translate "{name1} text {name2}" into a regex with named captures.
    let (rx, names) = match template_to_regex(pat) {
        Ok(p) => p,
        Err(e) => { eprintln!("parse: bad template: {}", e); return Err(1); }
    };
    let lines: Vec<String> = match input {
        PipelineData::Empty => Vec::new(),
        PipelineData::Bytes(b) => String::from_utf8_lossy(&b).lines().map(|s| s.to_string()).collect(),
        PipelineData::Values(vs) => vs.iter().map(|v| v.to_display_string()).collect(),
    };
    let mut out = Vec::new();
    for line in &lines {
        if let Some(caps) = rx.captures(line) {
            let mut rec = IndexMap::new();
            for name in &names {
                let val = caps.name(name).map(|m| m.as_str().to_string()).unwrap_or_default();
                rec.insert(name.clone(), Value::String(val));
            }
            out.push(Value::Record(rec));
        }
    }
    Ok(PipelineData::Values(out))
}

fn template_to_regex(template: &str) -> Result<(regex::Regex, Vec<String>), String> {
    let mut rx = String::from("^");
    let mut names = Vec::new();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            let mut name = String::new();
            while let Some(&nc) = chars.peek() {
                chars.next();
                if nc == '}' { break; }
                name.push(nc);
            }
            if name.is_empty() {
                return Err("empty {} capture".into());
            }
            rx.push_str(&format!("(?P<{}>.*?)", name));
            names.push(name);
        } else {
            // Escape regex metachars
            if "\\.+*?^$()[]|".contains(c) {
                rx.push('\\');
            }
            rx.push(c);
        }
    }
    rx.push('$');
    regex::Regex::new(&rx).map(|r| (r, names)).map_err(|e| e.to_string())
}

fn vb_str(input: PipelineData, args: &[String], _state: &mut ShellState) -> Result<PipelineData, i32> {
    let sub = match args.first().map(|s| s.as_str()) {
        Some(s) => s,
        None => { eprintln!("str: missing subcommand"); return Err(1); }
    };
    let rest = &args[1..];
    let map_str = |s: &str| -> Result<String, i32> {
        Ok(match sub {
            "trim" => s.trim().to_string(),
            "upcase" | "upper" => s.to_uppercase(),
            "downcase" | "lower" => s.to_lowercase(),
            "length" => return Ok(s.chars().count().to_string()),
            "contains" => {
                let needle = rest.first().map(|x| x.as_str()).unwrap_or("");
                return Ok(s.contains(needle).to_string());
            }
            "replace" => {
                if rest.len() < 2 { eprintln!("str replace: need <from> <to>"); return Err(1); }
                s.replace(&rest[0], &rest[1])
            }
            "split" => {
                let sep = rest.first().map(|x| x.as_str()).unwrap_or(" ");
                return Ok(format!("__rsh_split__{}", s.split(sep).collect::<Vec<_>>().join("\x1f")));
            }
            _ => { eprintln!("str: unknown subcommand '{}'", sub); return Err(1); }
        })
    };
    // Apply per-Value. For Bytes, treat as single string.
    let convert = |v: Value| -> Result<Value, i32> {
        match v {
            Value::String(s) => {
                let r = map_str(&s)?;
                if let Some(rest_s) = r.strip_prefix("__rsh_split__") {
                    Ok(Value::List(rest_s.split('\x1f').map(|p| Value::String(p.to_string())).collect()))
                } else if sub == "length" {
                    Ok(Value::Int(r.parse().unwrap_or(0)))
                } else if sub == "contains" {
                    Ok(Value::Bool(r == "true"))
                } else {
                    Ok(Value::String(r))
                }
            }
            other => {
                let s = other.to_display_string();
                let r = map_str(&s)?;
                if let Some(rest_s) = r.strip_prefix("__rsh_split__") {
                    Ok(Value::List(rest_s.split('\x1f').map(|p| Value::String(p.to_string())).collect()))
                } else if sub == "length" {
                    Ok(Value::Int(r.parse().unwrap_or(0)))
                } else if sub == "contains" {
                    Ok(Value::Bool(r == "true"))
                } else {
                    Ok(Value::String(r))
                }
            }
        }
    };
    match input {
        PipelineData::Empty => Ok(PipelineData::Empty),
        PipelineData::Bytes(b) => {
            let v = Value::String(String::from_utf8_lossy(&b).into_owned());
            Ok(PipelineData::Values(vec![convert(v)?]))
        }
        PipelineData::Values(vs) => {
            let mut out = Vec::with_capacity(vs.len());
            for v in vs { out.push(convert(v)?); }
            Ok(PipelineData::Values(out))
        }
    }
}

// ---------------------------------------------------------------------------
// Phase 6c — table / record operators (`get`, `update`, `insert`, `reject`,
// `wrap`, `flatten`)
// ---------------------------------------------------------------------------

fn parse_cell_path(s: &str) -> Vec<crate::parser::ast::PathSeg> {
    use crate::parser::ast::PathSeg;
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_bracket = false;
    let flush = |cur: &mut String, out: &mut Vec<PathSeg>| {
        if cur.is_empty() { return; }
        // Bare integer segment → Index (nushell-style `0.a.b`).
        if let Ok(n) = cur.parse::<i64>() {
            out.push(PathSeg::Index(n));
        } else {
            out.push(PathSeg::Field(std::mem::take(cur)));
        }
        cur.clear();
    };
    for c in s.chars() {
        match c {
            '.' if !in_bracket => flush(&mut cur, &mut out),
            '[' => { flush(&mut cur, &mut out); in_bracket = true; }
            ']' => {
                if in_bracket {
                    if let Ok(n) = cur.parse::<i64>() { out.push(PathSeg::Index(n)); }
                    cur.clear();
                    in_bracket = false;
                }
            }
            _ => cur.push(c),
        }
    }
    flush(&mut cur, &mut out);
    out
}

fn resolve_cell_path<'a>(v: &'a Value, path: &[crate::parser::ast::PathSeg]) -> Option<Value> {
    use crate::parser::ast::PathSeg;
    let mut cur = v.clone();
    for seg in path {
        cur = match (seg, &cur) {
            (PathSeg::Field(k), Value::Record(r)) => r.get(k).cloned()?,
            (PathSeg::Index(i), Value::List(items)) => {
                let len = items.len() as i64;
                let idx = if *i < 0 { len + *i } else { *i };
                if idx < 0 || idx as usize >= items.len() { return None; }
                items[idx as usize].clone()
            }
            _ => return None,
        };
    }
    Some(cur)
}

fn vb_get(input: PipelineData, args: &[String], _state: &mut ShellState) -> Result<PipelineData, i32> {
    use crate::parser::ast::PathSeg;
    let path_str = match args.first() {
        Some(p) => p,
        None => { eprintln!("get: missing cell-path"); return Err(1); }
    };
    let path = parse_cell_path(path_str);
    let vs = input.into_values()?;
    // Path that begins with an Index treats the pipeline as a List(vs); other
    // paths drill into a single Value or project a column from each row.
    let starts_with_index = matches!(path.first(), Some(PathSeg::Index(_)));
    if starts_with_index {
        let v = Value::List(vs);
        match resolve_cell_path(&v, &path) {
            Some(r) => Ok(PipelineData::Values(vec![r])),
            None => { eprintln!("get: path '{}' not found", path_str); Err(1) }
        }
    } else if vs.len() == 1 {
        match resolve_cell_path(&vs[0], &path) {
            Some(v) => Ok(PipelineData::Values(vec![v])),
            None => { eprintln!("get: path '{}' not found", path_str); Err(1) }
        }
    } else {
        let mut out = Vec::with_capacity(vs.len());
        for v in vs {
            match resolve_cell_path(&v, &path) {
                Some(x) => out.push(x),
                None => out.push(Value::Null),
            }
        }
        Ok(PipelineData::Values(out))
    }
}

fn vb_update(input: PipelineData, args: &[String], state: &mut ShellState) -> Result<PipelineData, i32> {
    if args.len() < 2 {
        eprintln!("update: usage `update <col> <value-or-closure>`");
        return Err(1);
    }
    let col = &args[0];
    let new_arg = &args[1];
    let closure = lookup_closure(Some(new_arg), state);
    let vs = input.into_values()?;
    let mut out = Vec::with_capacity(vs.len());
    for v in vs {
        let updated = if let Value::Record(mut r) = v {
            let new_val = if let Some(ref c) = closure {
                let row = Value::Record(r.clone());
                crate::executor::apply_closure(c, std::slice::from_ref(&row), state)?
            } else {
                parse_literal_value(new_arg)
            };
            r.insert(col.clone(), new_val);
            Value::Record(r)
        } else {
            v
        };
        out.push(updated);
    }
    Ok(PipelineData::Values(out))
}

fn parse_literal_value(s: &str) -> Value {
    if let Ok(n) = s.parse::<i64>() { return Value::Int(n); }
    if let Ok(f) = s.parse::<f64>() { return Value::Float(f); }
    match s {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        "null" => Value::Null,
        _ => Value::String(s.to_string()),
    }
}

fn vb_insert(input: PipelineData, args: &[String], state: &mut ShellState) -> Result<PipelineData, i32> {
    // Same as update but errors if column already exists. Closure form supported.
    if args.len() < 2 {
        eprintln!("insert: usage `insert <col> <value-or-closure>`");
        return Err(1);
    }
    let col = &args[0];
    let new_arg = &args[1];
    let closure = lookup_closure(Some(new_arg), state);
    let vs = input.into_values()?;
    let mut out = Vec::with_capacity(vs.len());
    for v in vs {
        let inserted = if let Value::Record(mut r) = v {
            if r.contains_key(col) {
                eprintln!("insert: column '{}' already exists", col);
                return Err(1);
            }
            let new_val = if let Some(ref c) = closure {
                let row = Value::Record(r.clone());
                crate::executor::apply_closure(c, std::slice::from_ref(&row), state)?
            } else {
                parse_literal_value(new_arg)
            };
            r.insert(col.clone(), new_val);
            Value::Record(r)
        } else {
            v
        };
        out.push(inserted);
    }
    Ok(PipelineData::Values(out))
}

fn vb_reject(input: PipelineData, args: &[String], _state: &mut ShellState) -> Result<PipelineData, i32> {
    if args.is_empty() {
        eprintln!("reject: missing column names");
        return Err(1);
    }
    let vs = input.into_values()?;
    let mut out = Vec::with_capacity(vs.len());
    for v in vs {
        let pruned = if let Value::Record(mut r) = v {
            for col in args { r.shift_remove(col); }
            Value::Record(r)
        } else { v };
        out.push(pruned);
    }
    Ok(PipelineData::Values(out))
}

fn vb_wrap(input: PipelineData, args: &[String], _state: &mut ShellState) -> Result<PipelineData, i32> {
    let col = match args.first() {
        Some(c) => c.clone(),
        None => { eprintln!("wrap: missing column name"); return Err(1); }
    };
    let vs = input.into_values()?;
    let out: Vec<Value> = vs.into_iter().map(|v| {
        let mut r = IndexMap::new();
        r.insert(col.clone(), v);
        Value::Record(r)
    }).collect();
    Ok(PipelineData::Values(out))
}

fn vb_flatten(input: PipelineData, _args: &[String], _state: &mut ShellState) -> Result<PipelineData, i32> {
    let vs = input.into_values()?;
    let mut out = Vec::new();
    for v in vs {
        match v {
            Value::List(items) => out.extend(items),
            other => out.push(other),
        }
    }
    Ok(PipelineData::Values(out))
}

// ---------------------------------------------------------------------------
// Phase 6d — type conversions and ranges
// ---------------------------------------------------------------------------

fn vb_into(input: PipelineData, args: &[String], _state: &mut ShellState) -> Result<PipelineData, i32> {
    let target = match args.first().map(|s| s.as_str()) {
        Some(t) => t,
        None => { eprintln!("into: missing type (int|float|string|bool)"); return Err(1); }
    };
    let col = args.get(1).map(|s| s.as_str());
    let convert = |v: &Value| -> Value {
        match target {
            "int" => match v {
                Value::Int(n) => Value::Int(*n),
                Value::Float(f) => Value::Int(*f as i64),
                Value::Bool(b) => Value::Int(if *b { 1 } else { 0 }),
                Value::String(s) => s.trim().parse::<i64>().ok()
                    .or_else(|| s.trim().parse::<f64>().ok().map(|f| f as i64))
                    .map(Value::Int).unwrap_or(Value::Null),
                _ => Value::Null,
            },
            "float" => match v {
                Value::Int(n) => Value::Float(*n as f64),
                Value::Float(f) => Value::Float(*f),
                Value::String(s) => s.trim().parse::<f64>().ok().map(Value::Float).unwrap_or(Value::Null),
                _ => Value::Null,
            },
            "string" => Value::String(v.to_display_string()),
            "bool" => match v {
                Value::Bool(b) => Value::Bool(*b),
                Value::Int(n) => Value::Bool(*n != 0),
                Value::Float(f) => Value::Bool(*f != 0.0),
                Value::String(s) => Value::Bool(!s.is_empty() && s != "false" && s != "0"),
                Value::Null => Value::Bool(false),
                _ => Value::Bool(true),
            },
            _ => v.clone(),
        }
    };
    let vs = input.into_values()?;
    let out: Vec<Value> = vs.into_iter().map(|v| {
        match (col, &v) {
            (Some(c), Value::Record(r)) => {
                let mut new_r = r.clone();
                if let Some(cell) = r.get(c) {
                    new_r.insert(c.to_string(), convert(cell));
                }
                Value::Record(new_r)
            }
            _ => convert(&v),
        }
    }).collect();
    Ok(PipelineData::Values(out))
}

fn vb_range(_input: PipelineData, args: &[String], _state: &mut ShellState) -> Result<PipelineData, i32> {
    let spec = match args.first() {
        Some(s) => s,
        None => { eprintln!("range: usage `range N..M` (inclusive) or `range N..<M` (exclusive)"); return Err(1); }
    };
    let (lo, hi, inclusive) = if let Some((a, b)) = spec.split_once("..<") {
        (a.parse::<i64>().ok(), b.parse::<i64>().ok(), false)
    } else if let Some((a, b)) = spec.split_once("..") {
        (a.parse::<i64>().ok(), b.parse::<i64>().ok(), true)
    } else {
        eprintln!("range: bad spec '{}'", spec);
        return Err(1);
    };
    let (lo, hi) = match (lo, hi) {
        (Some(a), Some(b)) => (a, b),
        _ => { eprintln!("range: bounds must be integers"); return Err(1); }
    };
    let end = if inclusive { hi + 1 } else { hi };
    let out: Vec<Value> = (lo..end).map(Value::Int).collect();
    Ok(PipelineData::Values(out))
}
