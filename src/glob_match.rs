/// Shared glob pattern matching using an iterative algorithm.
/// Replaces the three duplicate recursive implementations.

/// Match a value against a glob pattern supporting `*`, `?`, and `[...]`.
pub fn glob_match(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if pattern == value {
        return true;
    }
    let p: Vec<char> = pattern.chars().collect();
    let v: Vec<char> = value.chars().collect();
    glob_match_iter(&p, &v)
}

/// Parse a `[...]` character class starting at `pattern[pi]` (which must be `[`).
/// Returns `Some((negated, ranges))` where each range is `(start, end)` inclusive,
/// and advances `*pi` past the closing `]`.
/// Returns `None` if there is no closing `]` (treat `[` as literal).
fn match_char_class(pattern: &[char], pi: &mut usize) -> Option<(bool, Vec<(char, char)>)> {
    let start = *pi;
    // Must start with '['
    if pattern[start] != '[' {
        return None;
    }
    let mut i = start + 1;
    if i >= pattern.len() {
        return None;
    }

    // Check for negation
    let negated = pattern[i] == '!' || pattern[i] == '^';
    if negated {
        i += 1;
    }

    // If ']' appears right after '[' or '[!' / '[^', treat it as a literal char in the class
    let mut ranges: Vec<(char, char)> = Vec::new();
    let mut first = true;
    while i < pattern.len() {
        let c = pattern[i];
        if c == ']' && !first {
            // Found closing bracket
            *pi = i + 1; // advance past ']'
            return Some((negated, ranges));
        }
        first = false;
        // Check for range: a-z
        if i + 2 < pattern.len() && pattern[i + 1] == '-' && pattern[i + 2] != ']' {
            let range_start = c;
            let range_end = pattern[i + 2];
            ranges.push((range_start, range_end));
            i += 3;
        } else {
            ranges.push((c, c));
            i += 1;
        }
    }

    // No closing ']' found -- treat '[' as literal
    None
}

/// Check if a character matches a parsed character class.
fn char_in_class(ch: char, negated: bool, ranges: &[(char, char)]) -> bool {
    let mut found = false;
    for &(lo, hi) in ranges {
        let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
        if ch >= lo && ch <= hi {
            found = true;
            break;
        }
    }
    if negated { !found } else { found }
}

/// Iterative two-pointer glob matching (O(n*m) worst case, no stack overflow).
/// Supports `*`, `?`, and `[...]` character classes.
fn glob_match_iter(pattern: &[char], value: &[char]) -> bool {
    let mut pi = 0;
    let mut vi = 0;
    let mut star_pi: Option<usize> = None;
    let mut star_vi = 0;

    while vi < value.len() {
        if pi < pattern.len() && pattern[pi] == '[' {
            let mut tmp_pi = pi;
            if let Some((negated, ranges)) = match_char_class(pattern, &mut tmp_pi) {
                if char_in_class(value[vi], negated, &ranges) {
                    pi = tmp_pi;
                    vi += 1;
                    continue;
                }
                // Didn't match the class -- try star backtrack
                if let Some(sp) = star_pi {
                    pi = sp + 1;
                    star_vi += 1;
                    vi = star_vi;
                    continue;
                }
                return false;
            }
            // No closing ']' -- treat '[' as literal
            if pattern[pi] == value[vi] {
                pi += 1;
                vi += 1;
            } else if let Some(sp) = star_pi {
                pi = sp + 1;
                star_vi += 1;
                vi = star_vi;
            } else {
                return false;
            }
        } else if pi < pattern.len() && (pattern[pi] == '?' || pattern[pi] == value[vi]) {
            pi += 1;
            vi += 1;
        } else if pi < pattern.len() && pattern[pi] == '*' {
            star_pi = Some(pi);
            star_vi = vi;
            pi += 1;
        } else if let Some(sp) = star_pi {
            pi = sp + 1;
            star_vi += 1;
            vi = star_vi;
        } else {
            return false;
        }
    }

    // Consume trailing stars
    while pi < pattern.len() && pattern[pi] == '*' {
        pi += 1;
    }

    pi == pattern.len()
}

// ============================================================
// Extended glob (extglob) pattern matching
// Supports: !(pat), ?(pat), *(pat), +(pat), @(pat)
// ============================================================

/// Check if a pattern contains extglob syntax
pub fn contains_extglob(pattern: &str) -> bool {
    let mut escaped = false;
    let mut i = 0;
    let chars: Vec<char> = pattern.chars().collect();

    while i < chars.len() {
        if escaped {
            escaped = false;
            i += 1;
            continue;
        }

        if chars[i] == '\\' {
            escaped = true;
            i += 1;
            continue;
        }

        // Check for extglob patterns: !(, ?(, *(, +(, @(
        if i + 1 < chars.len() && chars[i + 1] == '(' {
            match chars[i] {
                '!' | '?' | '*' | '+' | '@' => return true,
                _ => {}
            }
        }

        i += 1;
    }

    false
}

/// Match a value against an extended glob pattern
/// Supports !(pat), ?(pat), *(pat), +(pat), @(pat)
pub fn extglob_match(pattern: &str, value: &str) -> bool {
    // If pattern contains no extglob syntax, fall back to regular glob
    if !contains_extglob(pattern) {
        return glob_match(pattern, value);
    }

    match_extglob_recursive(pattern, value, 0, 0)
}

/// Recursive helper for extglob matching
fn match_extglob_recursive(pattern: &str, value: &str, pi: usize, vi: usize) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let v: Vec<char> = value.chars().collect();

    let mut pi = pi;
    let mut vi = vi;

    while pi < p.len() {
        // Check for extglob pattern at current position
        if pi + 1 < p.len() && p[pi + 1] == '(' {
            match p[pi] {
                '!' => {
                    // !(pat): match anything NOT matching pat
                    if let Some((patterns, end)) = parse_extglob_group(&p, pi) {
                        pi = end;
                        // Try to match with any of the negated patterns
                        let mut matched_any = false;
                        for subpat in &patterns {
                            if match_extglob_pattern(subpat, &v, vi) {
                                matched_any = true;
                                break;
                            }
                        }

                        // If we matched one of the patterns, we're at a dead end
                        if matched_any {
                            return false;
                        }

                        // Otherwise, consume one character and continue
                        if vi < v.len() {
                            vi += 1;
                        } else {
                            return false;
                        }
                    } else {
                        // Invalid extglob, treat as literal
                        if vi < v.len() && v[vi] == p[pi] {
                            pi += 1;
                            vi += 1;
                        } else {
                            return false;
                        }
                    }
                }
                '?' => {
                    // ?(pat): match 0 or 1 occurrence of pat
                    if let Some((patterns, end)) = parse_extglob_group(&p, pi) {
                        pi = end;
                        for subpat in &patterns {
                            if match_extglob_pattern(subpat, &v, vi) {
                                // Pattern matched at current position
                                return match_extglob_recursive(pattern, value, pi, vi + subpat.len());
                            }
                        }
                        // No match, continue without consuming (0 occurrences)
                    } else {
                        // Invalid extglob, treat as literal
                        if vi < v.len() && v[vi] == p[pi] {
                            pi += 1;
                            vi += 1;
                        } else {
                            return false;
                        }
                    }
                }
                '*' => {
                    // *(pat): match 0 or more occurrences of pat
                    if let Some((patterns, end)) = parse_extglob_group(&p, pi) {
                        pi = end;
                        // Try matching 0, 1, 2, ... occurrences
                        let mut current_vi = vi;
                        loop {
                            // Try continuing from current position
                            if match_extglob_recursive(pattern, value, pi, current_vi) {
                                return true;
                            }
                            // Try matching one more occurrence
                            let mut found = false;
                            for subpat in &patterns {
                                if match_extglob_pattern(subpat, &v, current_vi) {
                                    current_vi += subpat.len();
                                    found = true;
                                    break;
                                }
                            }
                            if !found {
                                break;
                            }
                        }
                        return false;
                    } else {
                        // Invalid extglob, treat as literal
                        if vi < v.len() && v[vi] == p[pi] {
                            pi += 1;
                            vi += 1;
                        } else {
                            return false;
                        }
                    }
                }
                '+' => {
                    // +(pat): match 1 or more occurrences of pat
                    if let Some((patterns, end)) = parse_extglob_group(&p, pi) {
                        pi = end;
                        let mut current_vi = vi;
                        let mut matched_at_least_once = false;

                        // Try matching 1, 2, 3, ... occurrences
                        loop {
                            let mut found = false;
                            for subpat in &patterns {
                                if match_extglob_pattern(subpat, &v, current_vi) {
                                    current_vi += subpat.len();
                                    matched_at_least_once = true;
                                    found = true;
                                    break;
                                }
                            }
                            if !found {
                                break;
                            }
                        }

                        if !matched_at_least_once {
                            return false;
                        }

                        // Continue with remaining pattern
                        if match_extglob_recursive(pattern, value, pi, current_vi) {
                            return true;
                        }
                        return false;
                    } else {
                        // Invalid extglob, treat as literal
                        if vi < v.len() && v[vi] == p[pi] {
                            pi += 1;
                            vi += 1;
                        } else {
                            return false;
                        }
                    }
                }
                '@' => {
                    // @(pat): match exactly one of the patterns
                    if let Some((patterns, end)) = parse_extglob_group(&p, pi) {
                        pi = end;
                        for subpat in &patterns {
                            if match_extglob_pattern(subpat, &v, vi) {
                                return match_extglob_recursive(pattern, value, pi, vi + subpat.len());
                            }
                        }
                        return false;
                    } else {
                        // Invalid extglob, treat as literal
                        if vi < v.len() && v[vi] == p[pi] {
                            pi += 1;
                            vi += 1;
                        } else {
                            return false;
                        }
                    }
                }
                _ => {
                    if vi < v.len() && v[vi] == p[pi] {
                        pi += 1;
                        vi += 1;
                    } else {
                        return false;
                    }
                }
            }
        } else if p[pi] == '*' && pi + 1 >= p.len() {
            // Trailing star matches everything
            return true;
        } else if p[pi] == '*' {
            // Regular star: match any sequence
            if vi >= v.len() {
                return match_extglob_recursive(pattern, value, pi + 1, vi);
            }
            // Try matching zero characters
            if match_extglob_recursive(pattern, value, pi + 1, vi) {
                return true;
            }
            // Try matching one or more characters
            return match_extglob_recursive(pattern, value, pi, vi + 1);
        } else if p[pi] == '?' || p[pi] == v.get(vi).copied().unwrap_or('\0') {
            // Single char match
            if vi >= v.len() {
                return false;
            }
            pi += 1;
            vi += 1;
        } else {
            // Mismatch
            return false;
        }
    }

    vi == v.len()
}

/// Parse an extglob group like !(a|b|c)
/// Returns the list of patterns and the position after the closing paren
fn parse_extglob_group(pattern: &[char], start: usize) -> Option<(Vec<String>, usize)> {
    if start + 1 >= pattern.len() || pattern[start + 1] != '(' {
        return None;
    }

    let mut i = start + 2;
    let mut depth = 1;
    let mut current = String::new();
    let mut patterns = Vec::new();

    while i < pattern.len() && depth > 0 {
        match pattern[i] {
            '(' => {
                depth += 1;
                current.push('(');
            }
            ')' => {
                depth -= 1;
                if depth == 0 {
                    if !current.is_empty() {
                        patterns.push(current.trim().to_string());
                    }
                    return Some((patterns, i + 1));
                } else {
                    current.push(')');
                }
            }
            '|' if depth == 1 => {
                if !current.is_empty() {
                    patterns.push(current.trim().to_string());
                    current = String::new();
                }
            }
            c => current.push(c),
        }
        i += 1;
    }

    None // Unclosed paren
}

/// Check if a simple pattern matches at a position in the value
fn match_extglob_pattern(pattern: &str, value: &[char], start: usize) -> bool {
    let pat_chars: Vec<char> = pattern.chars().collect();
    let mut pi = 0;
    let mut vi = start;

    while pi < pat_chars.len() && vi < value.len() {
        match pat_chars[pi] {
            '*' => {
                // Star in subpattern
                if pi + 1 >= pat_chars.len() {
                    return true; // Star at end matches rest
                }
                // Try matching rest
                loop {
                    if match_extglob_pattern_iter(&pat_chars, pi + 1, value, vi) {
                        return true;
                    }
                    if vi >= value.len() {
                        break;
                    }
                    vi += 1;
                }
                return false;
            }
            '?' => {
                // Question mark matches any single char
                pi += 1;
                vi += 1;
            }
            c => {
                if c == value[vi] {
                    pi += 1;
                    vi += 1;
                } else {
                    return false;
                }
            }
        }
    }

    pi == pat_chars.len() && vi == value.len()
}

/// Helper for matching pattern from pi with value from vi
fn match_extglob_pattern_iter(pattern: &[char], pi: usize, value: &[char], vi: usize) -> bool {
    let mut pi = pi;
    let mut vi = vi;

    while pi < pattern.len() && vi < value.len() {
        match pattern[pi] {
            '*' => {
                if pi + 1 >= pattern.len() {
                    return true;
                }
                loop {
                    if match_extglob_pattern_iter(pattern, pi + 1, value, vi) {
                        return true;
                    }
                    if vi >= value.len() {
                        break;
                    }
                    vi += 1;
                }
                return false;
            }
            '?' => {
                pi += 1;
                vi += 1;
            }
            c => {
                if c == value[vi] {
                    pi += 1;
                    vi += 1;
                } else {
                    return false;
                }
            }
        }
    }

    pi == pattern.len() && vi == value.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact() {
        assert!(glob_match("hello", "hello"));
        assert!(!glob_match("hello", "world"));
    }

    #[test]
    fn test_star() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("he*", "hello"));
        assert!(glob_match("*lo", "hello"));
        assert!(glob_match("h*l*o", "hello"));
        assert!(!glob_match("h*x", "hello"));
    }

    #[test]
    fn test_question() {
        assert!(glob_match("h?llo", "hello"));
        assert!(!glob_match("h?llo", "hllo"));
    }

    #[test]
    fn test_empty() {
        assert!(glob_match("", ""));
        assert!(glob_match("*", ""));
        assert!(!glob_match("?", ""));
    }

    #[test]
    fn test_many_stars_no_exponential() {
        // This would blow up with naive recursion
        let pat = "a*a*a*a*a*a*a*a*b";
        let val = "aaaaaaaaaaaaaaaa";
        assert!(!glob_match(pat, val));
    }

    // Character class tests
    #[test]
    fn test_char_class_basic() {
        assert!(glob_match("[abc]", "a"));
        assert!(glob_match("[abc]", "b"));
        assert!(glob_match("[abc]", "c"));
        assert!(!glob_match("[abc]", "d"));
        assert!(!glob_match("[abc]", ""));
    }

    #[test]
    fn test_char_class_range() {
        assert!(glob_match("[a-z]", "a"));
        assert!(glob_match("[a-z]", "m"));
        assert!(glob_match("[a-z]", "z"));
        assert!(!glob_match("[a-z]", "A"));
        assert!(!glob_match("[a-z]", "0"));
    }

    #[test]
    fn test_char_class_negation() {
        assert!(!glob_match("[!abc]", "a"));
        assert!(glob_match("[!abc]", "d"));
        assert!(!glob_match("[^abc]", "b"));
        assert!(glob_match("[^abc]", "x"));
    }

    #[test]
    fn test_char_class_negated_range() {
        assert!(!glob_match("[!a-z]", "m"));
        assert!(glob_match("[!a-z]", "M"));
        assert!(glob_match("[!a-z]", "5"));
    }

    #[test]
    fn test_char_class_with_wildcards() {
        assert!(glob_match("*.[ch]", "foo.c"));
        assert!(glob_match("*.[ch]", "bar.h"));
        assert!(!glob_match("*.[ch]", "baz.o"));
        assert!(glob_match("[abc]*", "alpha"));
        assert!(!glob_match("[abc]*", "delta"));
        assert!(glob_match("?[0-9]?", "a5b"));
        assert!(!glob_match("?[0-9]?", "abc"));
    }

    #[test]
    fn test_char_class_unclosed_bracket() {
        // Unclosed '[' should be treated as a literal
        assert!(glob_match("[", "["));
        assert!(!glob_match("[", "a"));
        assert!(glob_match("[abc", "[abc"));
    }

    #[test]
    fn test_char_class_bracket_as_member() {
        // ']' right after '[' is treated as literal member
        assert!(glob_match("[]abc]", "]"));
        assert!(glob_match("[]abc]", "a"));
    }

    #[test]
    fn test_char_class_multiple_ranges() {
        assert!(glob_match("[a-zA-Z0-9]", "g"));
        assert!(glob_match("[a-zA-Z0-9]", "G"));
        assert!(glob_match("[a-zA-Z0-9]", "5"));
        assert!(!glob_match("[a-zA-Z0-9]", "!"));
    }

    #[test]
    fn test_char_class_in_complex_pattern() {
        assert!(glob_match("file[0-9].txt", "file3.txt"));
        assert!(!glob_match("file[0-9].txt", "fileA.txt"));
        assert!(glob_match("*[!.]*", "hello"));
        assert!(glob_match("[a-z][a-z][a-z]", "abc"));
        assert!(!glob_match("[a-z][a-z][a-z]", "ab1"));
    }

    #[test]
    fn test_extglob_basic() {
        // Test contains_extglob detection
        assert!(contains_extglob("!(pattern)"));
        assert!(contains_extglob("?(pattern)"));
        assert!(contains_extglob("*(pattern)"));
        assert!(contains_extglob("+(pattern)"));
        assert!(contains_extglob("@(pattern)"));
        assert!(!contains_extglob("*pattern"));
        assert!(!contains_extglob("?pattern"));
        assert!(!contains_extglob("[pattern]"));
    }

    #[test]
    fn test_extglob_negation() {
        // !(pat): match anything NOT matching pat
        assert!(extglob_match("!(test)", "hello"));
        assert!(extglob_match("!(test)", "foo"));
        assert!(!extglob_match("!(test)", "test"));
        assert!(extglob_match("!(*.txt)", "file.rs"));
        assert!(!extglob_match("!(*.txt)", "file.txt"));
    }

    #[test]
    fn test_extglob_optional() {
        // ?(pat): match 0 or 1 occurrence
        assert!(extglob_match("?(test)", ""));
        assert!(extglob_match("?(test)", "test"));
        assert!(!extglob_match("?(test)", "testtest"));
    }

    #[test]
    fn test_extglob_zero_or_more() {
        // *(pat): match 0 or more occurrences
        assert!(extglob_match("*(test)", ""));
        assert!(extglob_match("*(test)", "test"));
        assert!(extglob_match("*(test)", "testtest"));
        assert!(extglob_match("*(test)", "testtesttest"));
    }

    #[test]
    fn test_extglob_one_or_more() {
        // +(pat): match 1 or more occurrences
        assert!(!extglob_match("+(test)", ""));
        assert!(extglob_match("+(test)", "test"));
        assert!(extglob_match("+(test)", "testtest"));
        assert!(extglob_match("+(test)", "testtesttest"));
    }

    #[test]
    fn test_extglob_exactly_one() {
        // @(pat): match exactly one pattern
        assert!(!extglob_match("@(foo|bar)", "foobar"));
        assert!(extglob_match("@(foo|bar)", "foo"));
        assert!(extglob_match("@(foo|bar)", "bar"));
        assert!(!extglob_match("@(foo|bar)", "baz"));
    }
}
