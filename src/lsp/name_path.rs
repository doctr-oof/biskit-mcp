#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamePathPattern {
    segments: Vec<String>,
    absolute: bool,
    substring_matching: bool,
}

impl NamePathPattern {
    /// Accepts any of the three separators, so `PlayerService.addScore`,
    /// `PlayerService/addScore`, and `PlayerService:addScore` all address the same symbol.
    pub fn parse(pattern: &str, substring_matching: bool) -> Self {
        let trimmed = pattern.trim();
        let absolute = trimmed.starts_with('/') || trimmed.starts_with('.');
        let segments = trimmed
            .split(['/', '.', ':'])
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

    pub fn segments(&self) -> &[String] {
        &self.segments
    }

    pub fn is_absolute(&self) -> bool {
        self.absolute
    }

    /// The literal text a file must contain for any symbol in it to satisfy this pattern.
    ///
    /// A symbol cannot be defined in a file whose bytes never spell its leaf name, so a candidate
    /// file that lacks this string can be skipped without asking the language server about it.
    /// Substring queries hold to the same rule: the queried substring is still spelled literally
    /// inside the definition it is meant to find.
    pub fn literal_filter(&self) -> Option<&str> {
        let leaf = strip_overload_suffix(self.segments.last()?);
        (!leaf.is_empty()).then_some(leaf)
    }

    /// `name_path` is the `/`-joined ancestor chain of a symbol, outermost first.
    ///
    /// Compared right to left so the chain never has to be split into owned segments: this runs
    /// once per symbol node of every scanned file, which is the busiest loop in the crate.
    pub fn matches(&self, name_path: &str) -> bool {
        if self.segments.is_empty() || name_path.is_empty() {
            return false;
        }

        let last = self.segments.len() - 1;
        let mut candidate = name_path.rsplit('/');

        for (index, expected) in self.segments.iter().enumerate().rev() {
            let Some(raw) = candidate.next() else {
                return false;
            };
            if !segment_matches(expected, raw, index == last && self.substring_matching) {
                return false;
            }
        }

        // An absolute pattern has to have consumed the whole chain, leaving no outer owner.
        !(self.absolute && candidate.next().is_some())
    }
}

fn segment_matches(expected: &str, raw: &str, substring: bool) -> bool {
    // An indexed query segment names one specific duplicate, so it is compared against the stored
    // name with its suffix intact.
    if has_overload_suffix(expected) {
        return raw == expected;
    }
    let actual = strip_overload_suffix(raw);
    if substring {
        actual.contains(expected)
    } else {
        actual == expected
    }
}

/// Sibling symbols that share a name are disambiguated with a `[n]` suffix; matching ignores it.
pub fn strip_overload_suffix(name: &str) -> &str {
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

/// True when `name` carries the `[n]` disambiguation suffix.
fn has_overload_suffix(name: &str) -> bool {
    strip_overload_suffix(name).len() != name.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(parts: &[&str]) -> String {
        parts.join("/")
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

    #[test]
    fn colon_separator_is_interchangeable_with_the_others() {
        let target = chain(&["PlayerUtils", "GetPlayerMaid"]);
        assert!(NamePathPattern::parse("PlayerUtils:GetPlayerMaid", false).matches(&target));
        assert!(NamePathPattern::parse("PlayerUtils/GetPlayerMaid", false).matches(&target));
        assert!(NamePathPattern::parse("PlayerUtils.GetPlayerMaid", false).matches(&target));
    }

    #[test]
    fn method_leaf_matches_without_naming_its_owner() {
        let target = chain(&["PlayerUtils", "GetPlayerMaid"]);
        assert!(NamePathPattern::parse("GetPlayerMaid", false).matches(&target));
        assert!(!NamePathPattern::parse("PlayerUtils", false).matches(&target));
        assert!(!NamePathPattern::parse("/GetPlayerMaid", false).matches(&target));
    }

    #[test]
    fn indexed_query_segment_selects_one_duplicate() {
        let first = NamePathPattern::parse("UserInfo[0]", false);
        assert!(first.matches(&chain(&["UserInfo[0]"])));
        assert!(!first.matches(&chain(&["UserInfo[1]"])));
        assert!(!first.matches(&chain(&["UserInfo"])));

        let nested = NamePathPattern::parse("PlayerUtils:Init[1]", false);
        assert!(nested.matches(&chain(&["PlayerUtils", "Init[1]"])));
        assert!(!nested.matches(&chain(&["PlayerUtils", "Init[0]"])));
    }

    #[test]
    fn bare_query_still_matches_every_duplicate() {
        let pattern = NamePathPattern::parse("UserInfo", false);
        assert!(pattern.matches(&chain(&["UserInfo[0]"])));
        assert!(pattern.matches(&chain(&["UserInfo[1]"])));
    }

    #[test]
    fn an_empty_chain_matches_nothing() {
        assert!(!NamePathPattern::parse("update", false).matches(""));
        assert!(!NamePathPattern::parse("", false).matches("update"));
    }

    #[test]
    fn a_longer_pattern_than_chain_does_not_match() {
        let pattern = NamePathPattern::parse("Module/PlayerService/update", false);
        assert!(!pattern.matches(&chain(&["PlayerService", "update"])));
        assert!(pattern.matches(&chain(&["Module", "PlayerService", "update"])));
        assert!(pattern.matches(&chain(&["Outer", "Module", "PlayerService", "update"])));
    }

    #[test]
    fn literal_filter_names_the_leaf_without_its_suffix() {
        assert_eq!(
            NamePathPattern::parse("PlayerUtils:GetPlayerMaid", false).literal_filter(),
            Some("GetPlayerMaid")
        );
        assert_eq!(
            NamePathPattern::parse("UserInfo[1]", false).literal_filter(),
            Some("UserInfo")
        );
        assert_eq!(
            NamePathPattern::parse("Player", true).literal_filter(),
            Some("Player")
        );
        assert_eq!(NamePathPattern::parse("///", false).literal_filter(), None);
    }
}
