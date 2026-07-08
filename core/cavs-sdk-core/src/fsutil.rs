//! Filesystem helpers shared by the SDK operations: the deterministic
//! sorted walk and the `.cavsignore` rules. Semantics are identical to
//! `cavs-cli` (`ignore.rs` / `pack_dir.rs`): `*`/`?` stay within one path
//! segment, `**` crosses segments, a trailing `/` matches a directory and
//! everything under it, patterns without `/` match basenames at any depth,
//! and there is no negation.

use crate::error::Result;
use std::path::{Path, PathBuf};

pub const IGNORE_FILE: &str = ".cavsignore";

/// Every path under `root`, sorted, symlinks not followed.
pub fn walk_sorted(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut children: Vec<_> = std::fs::read_dir(&dir)?
            .collect::<std::io::Result<Vec<_>>>()?
            .into_iter()
            .map(|e| e.path())
            .collect();
        children.sort();
        for child in children {
            let meta = std::fs::symlink_metadata(&child)?;
            out.push(child.strip_prefix(root).unwrap().to_path_buf());
            if meta.is_dir() && !meta.file_type().is_symlink() {
                stack.push(child);
            }
        }
    }
    out.sort();
    Ok(out)
}

#[derive(Debug, Default, Clone)]
pub struct IgnoreRules {
    patterns: Vec<String>,
}

impl IgnoreRules {
    /// Caller patterns + the root's `.cavsignore` when present.
    pub fn load(root: &Path, extra_patterns: &[String]) -> std::io::Result<Self> {
        let mut patterns: Vec<String> = extra_patterns.to_vec();
        let file = root.join(IGNORE_FILE);
        if file.is_file() {
            for line in std::fs::read_to_string(&file)?.lines() {
                let line = line.trim();
                if !line.is_empty() && !line.starts_with('#') {
                    patterns.push(line.to_string());
                }
            }
        }
        Ok(IgnoreRules { patterns })
    }

    pub fn matches(&self, rel: &str, is_dir: bool) -> bool {
        if rel == IGNORE_FILE {
            return true;
        }
        self.patterns
            .iter()
            .any(|p| pattern_matches(p, rel, is_dir))
    }
}

fn pattern_matches(pattern: &str, rel: &str, is_dir: bool) -> bool {
    let (pattern, dir_only) = match pattern.strip_suffix('/') {
        Some(p) => (p, true),
        None => (pattern, false),
    };
    if pattern.is_empty() {
        return false;
    }
    let candidates: Vec<&str> = if pattern.contains('/') {
        vec![rel]
    } else {
        rel.split('/').collect()
    };
    if dir_only {
        if pattern.contains('/') {
            return (is_dir && glob_match(pattern, rel)) || prefix_dir_match(pattern, rel);
        }
        let segments: Vec<&str> = rel.split('/').collect();
        for (i, seg) in segments.iter().enumerate() {
            let last = i == segments.len() - 1;
            if glob_match(pattern, seg) && (!last || is_dir) {
                return true;
            }
        }
        return false;
    }
    if pattern.contains('/') {
        return glob_match(pattern, rel);
    }
    candidates.iter().any(|seg| glob_match(pattern, seg))
}

fn prefix_dir_match(pattern: &str, rel: &str) -> bool {
    rel.char_indices()
        .filter(|&(_, c)| c == '/')
        .any(|(i, _)| glob_match(pattern, &rel[..i]))
}

pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    glob_rec(&p, &t)
}

fn glob_rec(p: &[char], t: &[char]) -> bool {
    if p.is_empty() {
        return t.is_empty();
    }
    match p[0] {
        '*' if p.len() > 1 && p[1] == '*' => {
            let rest = if p.len() > 2 && p[2] == '/' {
                &p[3..]
            } else {
                &p[2..]
            };
            (0..=t.len()).any(|i| glob_rec(rest, &t[i..]))
        }
        '*' => (0..=t.len())
            .take_while(|&i| i == 0 || t[i - 1] != '/')
            .any(|i| glob_rec(&p[1..], &t[i..])),
        '?' => !t.is_empty() && t[0] != '/' && glob_rec(&p[1..], &t[1..]),
        c => !t.is_empty() && t[0] == c && glob_rec(&p[1..], &t[1..]),
    }
}

#[cfg(unix)]
pub fn is_executable(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
pub fn is_executable(_meta: &std::fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(patterns: &[&str]) -> IgnoreRules {
        IgnoreRules {
            patterns: patterns.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn basename_rule_matches_any_depth() {
        let r = rules(&["*.log"]);
        assert!(r.matches("a/b/x.log", false));
        assert!(!r.matches("a/b/x.txt", false));
    }

    #[test]
    fn anchored_rule_matches_from_root() {
        let r = rules(&["build/*.o"]);
        assert!(r.matches("build/x.o", false));
        assert!(!r.matches("src/build/x.o", false));
    }

    #[test]
    fn dir_rule_covers_children() {
        let r = rules(&["logs/"]);
        assert!(r.matches("logs", true));
        assert!(r.matches("logs/today/x.txt", false));
        assert!(!r.matches("logs", false));
    }

    #[test]
    fn ignore_file_always_excluded() {
        let r = rules(&[]);
        assert!(r.matches(IGNORE_FILE, false));
    }
}
