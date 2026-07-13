/// Data processing and transformation commands
/// Provides CSV, JSON, filtering, aggregation, and transformation utilities
use crate::environment::ShellState;
use std::collections::HashMap;
use std::io::IsTerminal;

/// filter - Keep only lines matching a pattern
pub fn builtin_filter(args: &[String]) -> i32 {
    if args.is_empty() {
        eprintln!("Usage: filter <pattern>");
        return 1;
    }
    if std::io::stdin().is_terminal() {
        eprintln!("filter: requires piped input, e.g.: cat file | filter <pattern>");
        return 1;
    }

    let pattern = &args[0];
    use std::io::BufRead;

    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        if let Ok(line) = line {
            if line.contains(pattern) {
                println!("{}", line);
            }
        }
    }
    0
}

/// map - Transform each line using a pattern
pub fn builtin_map(args: &[String]) -> i32 {
    if args.is_empty() {
        eprintln!("Usage: map <transform_pattern>");
        return 1;
    }
    if std::io::stdin().is_terminal() {
        eprintln!("map: requires piped input, e.g.: cat file | map <pattern>");
        return 1;
    }

    let _pattern = &args[0];
    use std::io::BufRead;

    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        if let Ok(line) = line {
            // Simple substitution: replace {x} with field x
            let mut result = line.clone();
            for (i, field) in line.split_whitespace().enumerate() {
                let placeholder = format!("{{{}}}", i);
                result = result.replace(&placeholder, field);
            }
            println!("{}", result);
        }
    }
    0
}

/// group-by - Group lines by a field
pub fn builtin_group_by(args: &[String], _state: &mut ShellState) -> i32 {
    if args.is_empty() {
        eprintln!("Usage: group-by <field_index>");
        return 1;
    }
    if std::io::stdin().is_terminal() {
        eprintln!("group-by: requires piped input, e.g.: cat file | group-by <field>");
        return 1;
    }

    let field_idx: usize = match args[0].parse() {
        Ok(n) => n,
        Err(_) => {
            eprintln!("Invalid field index");
            return 1;
        }
    };

    use std::io::BufRead;
    let mut groups: HashMap<String, Vec<String>> = HashMap::new();

    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        if let Ok(line) = line {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let key = if field_idx < fields.len() {
                fields[field_idx].to_string()
            } else {
                "unknown".to_string()
            };

            groups.entry(key).or_insert_with(Vec::new).push(line);
        }
    }

    for (key, lines) in groups {
        println!("Group: {}", key);
        for line in lines {
            println!("  {}", line);
        }
    }

    0
}

/// select - Select specific fields from each line
pub fn builtin_select(args: &[String]) -> i32 {
    if args.is_empty() {
        eprintln!("Usage: select <field1> [field2] ...");
        return 1;
    }
    if std::io::stdin().is_terminal() {
        eprintln!("select: requires piped input, e.g.: cat file | select <fields>");
        return 1;
    }

    let indices: Vec<usize> = args.iter().filter_map(|s| s.parse().ok()).collect();

    if indices.is_empty() {
        eprintln!("No valid field indices provided");
        return 1;
    }

    use std::io::BufRead;

    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        if let Ok(line) = line {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let selected: Vec<&str> = indices
                .iter()
                .filter_map(|&i| fields.get(i).copied())
                .collect();

            println!("{}", selected.join(" "));
        }
    }

    0
}

/// uniq - Remove duplicate consecutive lines
pub fn builtin_uniq(args: &[String]) -> i32 {
    if std::io::stdin().is_terminal() {
        eprintln!("uniq: requires piped input, e.g.: cat file | uniq");
        return 1;
    }
    let count = args.iter().any(|a| a == "-c");

    use std::io::BufRead;

    let stdin = std::io::stdin();
    let mut prev = String::new();
    let mut count_val = 0;

    for line in stdin.lock().lines() {
        if let Ok(line) = line {
            if line == prev {
                count_val += 1;
            } else {
                if !prev.is_empty() {
                    if count {
                        println!("{} {}", count_val, prev);
                    } else {
                        println!("{}", prev);
                    }
                }
                prev = line;
                count_val = 1;
            }
        }
    }

    if !prev.is_empty() {
        if count {
            println!("{} {}", count_val, prev);
        } else {
            println!("{}", prev);
        }
    }

    0
}

/// shuffle - Randomly shuffle lines
pub fn builtin_shuffle(_args: &[String]) -> i32 {
    if std::io::stdin().is_terminal() {
        eprintln!("shuffle: requires piped input, e.g.: cat file | shuffle");
        return 1;
    }
    use std::io::BufRead;

    let stdin = std::io::stdin();
    let mut lines: Vec<String> = stdin.lock().lines().filter_map(Result::ok).collect();

    // Simple shuffle using XORShift
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1);

    let mut rng = seed;
    for i in (1..lines.len()).rev() {
        rng = rng.wrapping_mul(1103515245).wrapping_add(12345);
        let j = (rng as usize) % (i + 1);
        lines.swap(i, j);
    }

    for line in lines {
        println!("{}", line);
    }

    0
}

/// dedupe - Remove all duplicates (unordered)
pub fn builtin_dedupe(_args: &[String]) -> i32 {
    if std::io::stdin().is_terminal() {
        eprintln!("dedupe: requires piped input, e.g.: cat file | dedupe");
        return 1;
    }
    use std::collections::HashSet;
    use std::io::BufRead;

    let stdin = std::io::stdin();
    let mut seen = HashSet::new();

    for line in stdin.lock().lines() {
        if let Ok(line) = line {
            if seen.insert(line.clone()) {
                println!("{}", line);
            }
        }
    }

    0
}
