use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;

use super::super::shared_require;
use crate::lines::LineIndex;

pub(super) enum CallKind {
    Require,
    Shared(Option<String>),
}

pub(super) struct RequireCall {
    pub(super) line: u32,
    offset: usize,
    pub(super) expression: String,
    pub(super) kind: CallKind,
}

pub(super) fn find_calls(
    blanked: &str,
    lines: &LineIndex<'_>,
    include_shared: bool,
) -> Vec<RequireCall> {
    let mut found = find_requires(blanked, lines);
    if include_shared {
        found.extend(find_shared_requires(blanked, lines));
        found.sort_by_key(|call| call.offset);
    }
    found
}

pub(super) fn find_requires(blanked: &str, lines: &LineIndex<'_>) -> Vec<RequireCall> {
    let bytes = blanked.as_bytes();
    let mut found = Vec::new();
    let finder = memchr::memmem::Finder::new(b"require");

    let mut from = 0;
    while let Some(offset) = finder.find(&bytes[from..]) {
        let start = from + offset;
        from = start + 7;

        let before_ok = start == 0 || !is_word_byte(bytes[start - 1]);
        if !before_ok {
            continue;
        }

        let mut cursor = start + 7;
        while cursor < bytes.len() && (bytes[cursor] == b' ' || bytes[cursor] == b'\t') {
            cursor += 1;
        }
        if cursor >= bytes.len() {
            continue;
        }

        let expression = match bytes[cursor] {
            b'(' => match balanced(bytes, cursor) {
                Some((inner, end)) => {
                    from = end;
                    blanked[inner].trim().to_string()
                }
                None => continue,
            },
            b'"' | b'\'' => {
                let rest = &blanked[cursor..];
                let quote = bytes[cursor] as char;
                match rest[1..].find(quote) {
                    Some(end) => rest[..end + 2].to_string(),
                    None => continue,
                }
            }
            _ => continue,
        };

        if expression.is_empty() {
            continue;
        }
        found.push(RequireCall {
            line: lines.line_of(start) as u32 + 1,
            offset: start,
            expression,
            kind: CallKind::Require,
        });
    }
    found
}

pub(super) fn find_shared_requires(blanked: &str, lines: &LineIndex<'_>) -> Vec<RequireCall> {
    let bytes = blanked.as_bytes();
    let name = shared_require::GLOBAL_NAME;
    let mut found = Vec::new();
    let finder = memchr::memmem::Finder::new(name.as_bytes());

    let mut from = 0;
    while let Some(offset) = finder.find(&bytes[from..]) {
        let start = from + offset;
        from = start + name.len();

        if start > 0 {
            let before = bytes[start - 1];
            if is_word_byte(before) || before == b'.' || before == b':' {
                continue;
            }
        }

        let mut cursor = start + name.len();
        if cursor < bytes.len() && is_word_byte(bytes[cursor]) {
            continue;
        }
        while cursor < bytes.len() && (bytes[cursor] == b' ' || bytes[cursor] == b'\t') {
            cursor += 1;
        }
        if cursor >= bytes.len() {
            continue;
        }

        let expression = match bytes[cursor] {
            b'(' => match balanced(bytes, cursor) {
                Some((inner, end)) => {
                    from = end;
                    blanked[inner].trim().to_string()
                }
                None => continue,
            },
            b'"' | b'\'' => {
                let rest = &blanked[cursor..];
                let quote = bytes[cursor] as char;
                match rest[1..].find(quote) {
                    Some(end) => rest[..end + 2].to_string(),
                    None => continue,
                }
            }
            _ => continue,
        };

        if expression.is_empty() {
            continue;
        }
        let module = string_literal(&expression);
        found.push(RequireCall {
            line: lines.line_of(start) as u32 + 1,
            offset: start,
            expression,
            kind: CallKind::Shared(module),
        });
    }
    found
}

fn string_literal(expression: &str) -> Option<String> {
    let chars: Vec<char> = expression.chars().collect();
    let quote = *chars.first()?;
    if quote != '"' && quote != '\'' {
        return None;
    }

    let mut value = String::new();
    let mut index = 1;
    while index < chars.len() {
        match chars[index] {
            character if character == quote => {
                return chars[index + 1..]
                    .iter()
                    .all(|rest| rest.is_whitespace())
                    .then_some(value);
            }
            '\\' => {
                let escaped = *chars.get(index + 1)?;
                value.push(match escaped {
                    'n' => '\n',
                    't' => '\t',
                    'r' => '\r',
                    other => other,
                });
                index += 2;
            }
            character => {
                value.push(character);
                index += 1;
            }
        }
    }
    None
}

fn balanced(bytes: &[u8], open: usize) -> Option<(std::ops::Range<usize>, usize)> {
    let mut depth = 0usize;
    let mut index = open;
    while index < bytes.len() {
        match bytes[index] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some((open + 1..index, index + 1));
                }
            }
            quote @ (b'"' | b'\'') => {
                index += 1;
                while index < bytes.len() && bytes[index] != quote {
                    index += if bytes[index] == b'\\' { 2 } else { 1 };
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn local_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"(?m)^[ \t]*local[ \t]+([A-Za-z_][A-Za-z0-9_]*)[ \t]*=[ \t]*([^\r\n]+)")
            .expect("the local binding pattern is a literal")
    })
}

pub(super) fn local_bindings(blanked: &str) -> HashMap<String, String> {
    let mut bindings = HashMap::new();
    for capture in local_pattern().captures_iter(blanked) {
        bindings.insert(capture[1].to_string(), capture[2].trim().to_string());
    }
    bindings
}

#[cfg(test)]
mod tests {
    use super::super::blank_comments;
    use super::*;

    #[test]
    fn a_require_spanning_lines_is_read_whole() {
        let source = "local m = require(\n\tgame:GetService(\"ReplicatedStorage\").Shared\n)\n";
        let blanked = blank_comments(source);
        let lines = LineIndex::new(&blanked);
        let found = find_requires(&blanked, &lines);

        assert_eq!(found.len(), 1);
        assert!(found[0].expression.contains("GetService"));
        assert_eq!(found[0].line, 1);
    }

    #[test]
    fn a_word_ending_in_require_is_not_a_require() {
        let source = "local prerequire = 1\nlocal m = require(script.Real)\n";
        let blanked = blank_comments(source);
        let lines = LineIndex::new(&blanked);
        assert_eq!(find_requires(&blanked, &lines).len(), 1);
    }

    #[test]
    fn local_bindings_keep_the_last_value_a_name_was_given() {
        let bindings = local_bindings("local a = script.One\nlocal a = script.Two\n");
        assert_eq!(bindings.get("a").map(String::as_str), Some("script.Two"));
    }

    #[test]
    fn multiple_assignment_is_not_read_as_a_binding() {
        let bindings = local_bindings("local a, b = 1, 2\n");
        assert!(bindings.is_empty());
    }

    #[test]
    fn indexing_the_shared_table_is_not_a_require() {
        let source = "shared.someFlag = true\nlocal held = shared.cache\nlocal n = shared[key]\n";
        let blanked = blank_comments(source);
        let lines = LineIndex::new(&blanked);
        assert!(find_shared_requires(&blanked, &lines).is_empty());
    }

    #[test]
    fn a_qualified_call_is_not_the_shared_global() {
        let source = "local a = Framework.shared(\"Config\")\nlocal b = self:shared(\"Config\")\n";
        let blanked = blank_comments(source);
        let lines = LineIndex::new(&blanked);
        assert!(find_shared_requires(&blanked, &lines).is_empty());
    }

    #[test]
    fn calls_are_reported_in_the_order_they_are_written() {
        let source = "require(script.A)\nshared(\"B\")\nrequire(script.C)\n";
        let blanked = blank_comments(source);
        let lines = LineIndex::new(&blanked);
        let lines_found: Vec<u32> = find_calls(&blanked, &lines, true)
            .iter()
            .map(|call| call.line)
            .collect();
        assert_eq!(lines_found, vec![1, 2, 3]);
    }

    #[test]
    fn only_a_lone_string_literal_carries_a_module_name() {
        assert_eq!(string_literal("\"Foo\""), Some("Foo".to_string()));
        assert_eq!(string_literal("'Foo'"), Some("Foo".to_string()));
        assert_eq!(
            string_literal("\"jobs\\\\Foo\""),
            Some("jobs\\Foo".to_string())
        );
        assert_eq!(string_literal("\"Foo\"  "), Some("Foo".to_string()));

        for expression in ["name", "\"a\" .. b", "\"a\", \"b\"", "\"unterminated", ""] {
            assert_eq!(string_literal(expression), None, "{expression} parsed");
        }
    }
}
