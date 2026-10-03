/// Matches a pack path against a glob: `*` and `?` within a segment, `**` across segments.
pub fn matches(pattern: &str, path: &str) -> bool {
    let pattern: Vec<&str> = pattern.split('/').collect();
    let path: Vec<&str> = path.split('/').collect();
    segments(&pattern, &path)
}

pub fn any(patterns: &[String], path: &str) -> bool {
    patterns.iter().any(|p| matches(p, path))
}

fn segments(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|skip| segments(rest, &path[skip..])),
        Some((first, rest)) => match path.split_first() {
            Some((segment, others)) => {
                segment_matches(first.as_bytes(), segment.as_bytes()) && segments(rest, others)
            }
            None => false,
        },
    }
}

fn segment_matches(pattern: &[u8], name: &[u8]) -> bool {
    match (pattern.split_first(), name.split_first()) {
        (None, None) => true,
        (Some((b'*', rest)), _) => {
            segment_matches(rest, name)
                || name
                    .split_first()
                    .is_some_and(|(_, tail)| segment_matches(pattern, tail))
        }
        (Some((b'?', rest)), Some((_, tail))) => segment_matches(rest, tail),
        (Some((a, rest)), Some((b, tail))) if a == b => segment_matches(rest, tail),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::matches;

    #[test]
    fn globs_follow_segment_rules() {
        assert!(matches("options.txt", "options.txt"));
        assert!(!matches("options.txt", "config/options.txt"));
        assert!(matches("config/xaero/**", "config/xaero/minimap/a.txt"));
        assert!(matches("config/xaero/**", "config/xaero"));
        assert!(!matches("config/xaero/**", "config/xaerominimap.txt"));
        assert!(matches("**/*.bak", "a.bak"));
        assert!(matches("**/*.bak", "config/deep/a.bak"));
        assert!(!matches("*.bak", "config/a.bak"));
        assert!(matches("config/*-client.toml", "config/create-client.toml"));
        assert!(!matches(
            "config/*-client.toml",
            "config/sub/create-client.toml"
        ));
        assert!(matches("kubejs/?.js", "kubejs/a.js"));
        assert!(!matches("kubejs/?.js", "kubejs/ab.js"));
    }
}
