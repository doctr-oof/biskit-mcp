mod build;
mod chain;
mod graph;
mod resolve;
mod scan;
mod string_require;

use anyhow::Result;

pub use build::{GraphStamp, build_or_reuse};
pub use graph::{
    Edge, GraphAnswer, GraphRequest, Module, ReachedModule, RequireGraph, UnresolvedEntry,
    UnresolvedRequire,
};

/// Which way an edge is followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Dependencies,
    Dependents,
}

impl Direction {
    pub fn parse(value: Option<&str>) -> Result<Vec<Self>> {
        match value.unwrap_or("both").trim() {
            "both" => Ok(vec![Self::Dependencies, Self::Dependents]),
            "dependencies" => Ok(vec![Self::Dependencies]),
            "dependents" => Ok(vec![Self::Dependents]),
            other => Err(crate::errors::hinted(
                format!(
                    "direction must be \"dependencies\", \"dependents\", or \"both\", got {other:?}"
                ),
                "omit direction to get both",
            )),
        }
    }
}

/// Blanks every comment, keeping every byte offset and every line break where it was.
pub fn blank_comments(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out = bytes.to_vec();
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'-' if bytes.get(index + 1) == Some(&b'-') => {
                let start = index;
                index += 2;
                match long_bracket(bytes, index) {
                    Some(level) => {
                        let close = format!("]{}]", "=".repeat(level));
                        let from = index + level + 2;
                        let end = source[from..]
                            .find(&close)
                            .map(|offset| from + offset + close.len())
                            .unwrap_or(bytes.len());
                        blank(&mut out, start, end);
                        index = end;
                    }
                    None => {
                        let end = source[index..]
                            .find('\n')
                            .map(|offset| index + offset)
                            .unwrap_or(bytes.len());
                        blank(&mut out, start, end);
                        index = end;
                    }
                }
            }
            quote @ (b'"' | b'\'') => {
                index += 1;
                while index < bytes.len() && bytes[index] != quote && bytes[index] != b'\n' {
                    index += if bytes[index] == b'\\' { 2 } else { 1 };
                }
                index += 1;
            }
            b'[' => match long_bracket(bytes, index) {
                Some(level) => {
                    let close = format!("]{}]", "=".repeat(level));
                    let from = index + level + 2;
                    index = source[from..]
                        .find(&close)
                        .map(|offset| from + offset + close.len())
                        .unwrap_or(bytes.len());
                }
                None => index += 1,
            },
            _ => index += 1,
        }
    }

    String::from_utf8(out).unwrap_or_else(|_| source.to_string())
}

fn long_bracket(bytes: &[u8], index: usize) -> Option<usize> {
    if bytes.get(index) != Some(&b'[') {
        return None;
    }
    let mut level = 0;
    while bytes.get(index + 1 + level) == Some(&b'=') {
        level += 1;
    }
    match bytes.get(index + 1 + level) {
        Some(&b'[') => Some(level),
        _ => None,
    }
}

fn blank(out: &mut [u8], from: usize, to: usize) {
    let end = to.min(out.len());
    for byte in &mut out[from..end] {
        if *byte != b'\n' {
            *byte = b' ';
        }
    }
}

#[cfg(test)]
mod tests {
    use super::scan::{find_requires, find_shared_requires};
    use super::*;
    use crate::lines::LineIndex;

    #[test]
    fn a_commented_out_require_is_not_a_dependency() {
        let source = "-- require(script.Parent.Old)\nrequire(script.Parent.New)\n";
        let blanked = blank_comments(source);
        let lines = LineIndex::new(&blanked);
        let found = find_requires(&blanked, &lines);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].expression, "script.Parent.New");
        assert_eq!(found[0].line, 2);
    }

    #[test]
    fn a_long_comment_is_blanked_without_moving_the_lines_after_it() {
        let source = "--[==[\nrequire(script.Hidden)\n]==]\nrequire(script.Real)\n";
        let blanked = blank_comments(source);
        let lines = LineIndex::new(&blanked);
        let found = find_requires(&blanked, &lines);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].line, 4);
    }

    #[test]
    fn a_double_dash_inside_a_string_does_not_start_a_comment() {
        let source = "local separator = \"--\" local m = require(script.Real)\n";
        let blanked = blank_comments(source);
        let lines = LineIndex::new(&blanked);
        assert_eq!(find_requires(&blanked, &lines).len(), 1);
    }

    #[test]
    fn a_commented_out_shared_call_is_not_a_dependency() {
        let source = "-- shared(\"Old\")\nshared(\"New\")\n";
        let blanked = blank_comments(source);
        let lines = LineIndex::new(&blanked);
        let found = find_shared_requires(&blanked, &lines);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].line, 2);
    }
}
