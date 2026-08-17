#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamePathPattern {
    segments: Vec<String>,
    absolute: bool,
    substring_matching: bool,
}

impl NamePathPattern {
    /// Accepts either separator, so `PlayerService.addScore` and `PlayerService/addScore`
    /// both address the same symbol.
    pub fn parse(pattern: &str, substring_matching: bool) -> Self {
        let trimmed = pattern.trim();
        let absolute = trimmed.starts_with('/') || trimmed.starts_with('.');
        let segments = trimmed
            .split(['/', '.'])
            .filter(|segment| !segment.is_empty())
            .map(str::to_string)
            .collect();
        Self {
            segments,
            absolute,
            substring_matching,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// `candidate` is the full ancestor chain of a symbol, outermost first.
    pub fn matches(&self, candidate: &[String]) -> bool {
        if self.segments.is_empty() || candidate.is_empty() {
            return false;
        }
        if self.segments.len() > candidate.len() {
            return false;
        }

        let offset = candidate.len() - self.segments.len();
        if self.absolute && offset != 0 {
            return false;
        }

        let tail = &candidate[offset..];
        let last = self.segments.len() - 1;
        self.segments.iter().enumerate().all(|(index, expected)| {
            let actual = strip_overload_suffix(&tail[index]);
            if index == last && self.substring_matching {
                actual.contains(expected.as_str())
            } else {
                actual == expected
            }
        })
    }
}

/// Sibling symbols that share a name are disambiguated with a `[n]` suffix; matching ignores it.
fn strip_overload_suffix(name: &str) -> &str {
    let Some(open) = name.rfind('[') else {
        return name;
    };
    if !name.ends_with(']') {
        return name;
    }
    if name[open + 1..name.len() - 1]
        .chars()
        .all(|c| c.is_ascii_digit())
    {
        &name[..open]
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| part.to_string()).collect()
    }

    #[test]
    fn relative_pattern_matches_at_any_depth() {
        let pattern = NamePathPattern::parse("update", false);
        assert!(pattern.matches(&chain(&["PlayerService", "update"])));
        assert!(pattern.matches(&chain(&["update"])));
    }

    #[test]
    fn absolute_pattern_anchors_to_top_level() {
        let pattern = NamePathPattern::parse("/PlayerService", false);
        assert!(pattern.matches(&chain(&["PlayerService"])));
        assert!(!pattern.matches(&chain(&["Outer", "PlayerService"])));
    }

    #[test]
    fn multi_segment_requires_contiguous_suffix() {
        let pattern = NamePathPattern::parse("PlayerService/update", false);
        assert!(pattern.matches(&chain(&["Module", "PlayerService", "update"])));
        assert!(!pattern.matches(&chain(&["PlayerService", "Inner", "update"])));
    }

    #[test]
    fn substring_matching_applies_to_last_segment_only() {
        let pattern = NamePathPattern::parse("PlayerService/upd", true);
        assert!(pattern.matches(&chain(&["PlayerService", "update"])));

        let strict = NamePathPattern::parse("PlayerServ/update", true);
        assert!(!strict.matches(&chain(&["PlayerService", "update"])));
    }

    #[test]
    fn dot_and_slash_separators_are_interchangeable() {
        let target = chain(&["PlayerService", "addScore"]);
        assert!(NamePathPattern::parse("PlayerService.addScore", false).matches(&target));
        assert!(NamePathPattern::parse("PlayerService/addScore", false).matches(&target));
        assert!(NamePathPattern::parse(".PlayerService.addScore", false).matches(&target));
        assert!(!NamePathPattern::parse(".addScore", false).matches(&target));
    }

    #[test]
    fn overload_suffix_is_ignored() {
        let pattern = NamePathPattern::parse("update", false);
        assert!(pattern.matches(&chain(&["update[1]"])));
        assert!(!pattern.matches(&chain(&["update[abc]"])));
    }
}
