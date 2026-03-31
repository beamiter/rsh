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
}
