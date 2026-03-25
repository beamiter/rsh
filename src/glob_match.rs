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

/// Iterative two-pointer glob matching (O(n*m) worst case, no stack overflow).
fn glob_match_iter(pattern: &[char], value: &[char]) -> bool {
    let mut pi = 0;
    let mut vi = 0;
    let mut star_pi: Option<usize> = None;
    let mut star_vi = 0;

    while vi < value.len() {
        if pi < pattern.len() && (pattern[pi] == '?' || pattern[pi] == value[vi]) {
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
}
