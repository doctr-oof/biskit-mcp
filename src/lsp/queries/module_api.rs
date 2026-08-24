use anyhow::Result;
use serde::Serialize;

use super::SymbolQuery;
use super::hover::{split_hover, strip_unbound_generics};
use crate::lines::LineIndex;
use crate::lsp::protocol::Position;
use crate::lsp::session::ensure_luau_file;
use crate::lsp::symbols::{SymbolNode, is_identifier_byte};

const MAX_TYPE_DECLARATION_LINES: usize = 40;

/// One hover request per export, so this bounds how slow reading a wide module can get.
const MAX_EXPORT_HOVERS: usize = 50;

const NOT_A_MODULE_NOTE: &str = "this file returns nothing, so it is a Script or a LocalScript \
                                 rather than a ModuleScript and has no public surface. Use \
                                 get_symbols_overview to see what it defines.";

const UNLOCATED_RETURN_NOTE: &str = "the sourcemap calls this a ModuleScript, so it returns \
                                     something, but no return statement outside a function body \
                                     could be found in the source. Read the end of the file for \
                                     what it hands back.";

const CONDITIONAL_RETURN_NOTE: &str = "this module returns from more than one branch, so which \
                                       surface a caller gets depends on the condition. The \
                                       branches return:";

const DETAIL_CAPPED_NOTE: &str = "detail was filled for the first exports only: one hover request \
                                  per export is spent resolving a signature, and this module has \
                                  more exports than that ceiling. Use find_symbol with \
                                  include_detail for the ones still missing a signature.";

const UNTRACED_RETURN_NOTE: &str = "the returned value is not a table declared in this file, so \
                                    there is no surface to list.";

const TABLE_LITERAL_NOTE: &str = "this module returns a table written inline in the return \
                                  statement rather than a named one, which the symbol tree does \
                                  not describe. Read the return statement itself, or use \
                                  explain_symbol on it for the type it resolves to.";

const FUNCTION_RETURN_NOTE: &str = "this module returns a function rather than a table, so \
                                    calling it is its whole surface. Use explain_symbol on the \
                                    return statement for the function's signature.";

const EMPTY_TABLE_NOTE: &str = "the returned table has no members the language server can see in \
                                this file. Members assigned through another name, or by a loop, \
                                are invisible here. The table is";

/// One entry of a module's public surface.
#[derive(Debug, Clone, Serialize)]
pub struct ModuleExport {
    pub name: String,
    pub kind: String,
    pub line: u32,
    /// The language server's signature for it, which is the type the caller will be handed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// An `export type` declaration, quoted as written.
#[derive(Debug, Clone, Serialize)]
pub struct ExportedType {
    pub name: String,
    pub line: u32,
    pub declaration: String,
}

/// What a ModuleScript hands back, and nothing else.
#[derive(Debug, Clone, Serialize)]
pub struct ModuleApi {
    pub relative_path: String,
    /// The returned expression as written, so a module that returns something unusual still says what it returns.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub returns: Option<String>,
    /// One of `table`, `table_literal`, `function`, `expression`, `conditional`, `unknown`, or `none`.
    pub return_kind: &'static str,
    pub exports: Vec<ModuleExport>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub types: Vec<ExportedType>,
    #[serde(skip_serializing_if = "crate::serde_skip::is_false")]
    pub truncated: bool,
    /// Why the surface is empty or partial, on the paths where it is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl<'a> SymbolQuery<'a> {
    /// The public surface of a ModuleScript: what its returned value exposes, plus the types it exports, and none of the body either was implemented in.
    ///
    /// `class_name` is what the sourcemap calls the file, which decides whether a missing return means "this is a Script" or "the return could not be located".
    pub async fn module_api(
        &self,
        relative_path: &str,
        class_name: Option<&str>,
        max_exports: usize,
    ) -> Result<ModuleApi> {
        let path = self.project().resolve(relative_path)?;
        ensure_luau_file(&path)?;

        let session = self.handle.session().await?;
        let (symbols, content) = self.handle.document_symbols(&session, &path).await?;
        let relative = self.project().relativize(&path)?;

        let blanked = crate::roblox::requires::blank_comments(&content);
        let types = exported_types(&content, &blanked, max_exports);
        let returns = module_returns(&blanked);

        let Some((line, expression)) = returns.first().cloned() else {
            let is_module = class_name == Some("ModuleScript");
            return Ok(ModuleApi {
                relative_path: relative,
                returns: None,
                return_kind: if is_module { "unknown" } else { "none" },
                exports: Vec::new(),
                types,
                truncated: false,
                note: Some(match is_module {
                    true => UNLOCATED_RETURN_NOTE.to_string(),
                    false => NOT_A_MODULE_NOTE.to_string(),
                }),
            });
        };

        let classified: Vec<(Option<String>, Option<&'static str>)> = returns
            .iter()
            .map(|(_, expression)| returned_name(expression))
            .collect();
        let agreed_name = agreed(classified.iter().map(|(name, _)| name.clone())).flatten();
        let agreed_kind = agreed(classified.iter().map(|(_, kind)| *kind)).flatten();

        let owner = agreed_name
            .as_deref()
            .and_then(|name| top_level_symbol(&symbols, name));

        let mut exports: Vec<(ModuleExport, Position)> = owner
            .map(|node| {
                node.children
                    .iter()
                    .map(|child| {
                        (
                            ModuleExport {
                                name: child.name.clone(),
                                kind: child.kind_label().to_string(),
                                line: child.range.start.line + 1,
                                detail: child.detail.clone(),
                            },
                            child.target_position(&content),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        exports.sort_by_key(|(export, _)| export.line);

        let truncated = exports.len() > max_exports;
        exports.truncate(max_exports);

        let detail_capped = exports.len() > MAX_EXPORT_HOVERS;
        for (export, position) in exports.iter_mut().take(MAX_EXPORT_HOVERS) {
            let Ok(Some(hover)) = session.hover(&path, *position).await else {
                continue;
            };
            let (signature, _) = split_hover(&hover.contents.into_markdown());
            if !signature.is_empty() {
                export.detail = Some(strip_unbound_generics(&signature));
            }
        }

        let conditional = returns.len() > 1;
        let return_kind = match (agreed_kind, owner) {
            (Some(kind), _) => kind,
            (None, Some(node)) if !node.children.is_empty() => "table",
            (None, _) if conditional => "conditional",
            (None, _) => "expression",
        };

        let mut note = match (owner, return_kind) {
            (_, "table_literal") => Some(format!("{TABLE_LITERAL_NOTE} It starts on line {line}.")),
            (_, "function") => Some(FUNCTION_RETURN_NOTE.to_string()),
            (None, _) if conditional => None,
            (None, _) => Some(format!(
                "{UNTRACED_RETURN_NOTE} It returns `{expression}` on line {line}."
            )),
            (Some(node), _) if node.children.is_empty() => {
                Some(format!("{EMPTY_TABLE_NOTE} {}", node.name_path))
            }
            _ => None,
        };

        if conditional {
            let branches: Vec<String> = returns
                .iter()
                .map(|(line, expression)| format!("line {line}: `{expression}`"))
                .collect();
            note = Some(join_notes(
                note,
                format!("{CONDITIONAL_RETURN_NOTE} {}.", branches.join("; ")),
            ));
        }
        if detail_capped {
            note = Some(join_notes(note, DETAIL_CAPPED_NOTE.to_string()));
        }

        Ok(ModuleApi {
            relative_path: relative,
            returns: (return_kind != "table_literal").then_some(expression),
            return_kind,
            exports: exports.into_iter().map(|(export, _)| export).collect(),
            types,
            truncated,
            note,
        })
    }
}

fn join_notes(existing: Option<String>, addition: String) -> String {
    match existing {
        Some(note) => format!("{note} {addition}"),
        None => addition,
    }
}

/// The one value every item carries, or `None` the moment two of them differ.
fn agreed<T: PartialEq>(values: impl Iterator<Item = T>) -> Option<T> {
    let mut found: Option<T> = None;
    for value in values {
        match &found {
            Some(first) if *first != value => return None,
            Some(_) => {}
            None => found = Some(value),
        }
    }
    found
}

/// Which block a `return` sits inside, so a `return` in a function body is told apart from the module's own.
#[derive(Debug, PartialEq, Eq)]
enum Block {
    Function,
    Conditional,
    Repeat,
}

/// Every `return` reachable from the chunk body, at any block depth, in source order.
///
/// A module that returns from inside a top-level `if`/`else` is ordinary Luau, so scanning only column zero misses it.
fn module_returns(blanked: &str) -> Vec<(u32, String)> {
    let index = LineIndex::new(blanked);
    let mut stack: Vec<Block> = Vec::new();
    let mut found = Vec::new();

    for (offset, token) in keywords(blanked) {
        match token {
            "function" => stack.push(Block::Function),
            "if" if !opens_a_block(blanked, offset) => {}
            "if" | "do" => stack.push(Block::Conditional),
            "repeat" => stack.push(Block::Repeat),
            "end" => {
                if matches!(stack.last(), Some(Block::Function | Block::Conditional)) {
                    stack.pop();
                }
            }
            "until" => {
                if stack.last() == Some(&Block::Repeat) {
                    stack.pop();
                }
            }
            "return" if !stack.contains(&Block::Function) => {
                let rest = &blanked[offset + "return".len()..];
                let expression = rest.split('\n').next().unwrap_or_default().trim();
                found.push((index.line_of(offset) as u32 + 1, expression.to_string()));
            }
            _ => {}
        }
    }
    found
}

/// Whether the `if` at `offset` starts an `if ... end` statement rather than an `if ... then ... else` expression.
///
/// Luau's if-then-else expression carries no `end`. Pushing a block for it leaves an opener nothing ever closes, so
/// every later `end` pops the wrong frame and a function block stays on the stack for the rest of the file.
fn opens_a_block(blanked: &str, offset: usize) -> bool {
    const IN_EXPRESSION: &[u8] = b"=,({[+-*/%^#.<~&";

    let bytes = blanked.as_bytes();
    let mut at = offset;
    while at > 0 && bytes[at - 1].is_ascii_whitespace() {
        at -= 1;
    }
    let Some(&previous) = at.checked_sub(1).map(|index| &bytes[index]) else {
        return true;
    };

    if !is_identifier_byte(previous) {
        return !IN_EXPRESSION.contains(&previous);
    }

    let mut start = at;
    while start > 0 && is_identifier_byte(bytes[start - 1]) {
        start -= 1;
    }
    !matches!(&blanked[start..at], "return" | "and" | "or" | "not" | "in")
}

/// Every identifier in the source, with string literals skipped so a quoted `end` is not read as a block close.
fn keywords(blanked: &str) -> Vec<(usize, &str)> {
    let bytes = blanked.as_bytes();
    let mut found = Vec::new();
    let mut at = 0usize;

    while at < bytes.len() {
        let byte = bytes[at];
        if byte == b'"' || byte == b'\'' {
            at = skip_quoted(bytes, at);
        } else if byte == b'[' {
            at = skip_long_bracket(bytes, at).unwrap_or(at + 1);
        } else if byte.is_ascii_alphabetic() || byte == b'_' {
            let start = at;
            while at < bytes.len() && is_identifier_byte(bytes[at]) {
                at += 1;
            }
            found.push((start, &blanked[start..at]));
        } else if byte.is_ascii_digit() {
            while at < bytes.len() && is_identifier_byte(bytes[at]) {
                at += 1;
            }
        } else {
            at += 1;
        }
    }
    found
}

fn skip_quoted(bytes: &[u8], open: usize) -> usize {
    let quote = bytes[open];
    let mut at = open + 1;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at += 2,
            b'\n' => return at,
            byte if byte == quote => return at + 1,
            _ => at += 1,
        }
    }
    bytes.len()
}

fn skip_long_bracket(bytes: &[u8], open: usize) -> Option<usize> {
    let mut level = 0usize;
    let mut at = open + 1;
    while bytes.get(at) == Some(&b'=') {
        level += 1;
        at += 1;
    }
    if bytes.get(at) != Some(&b'[') {
        return None;
    }

    let mut close = Vec::with_capacity(level + 2);
    close.push(b']');
    close.extend(std::iter::repeat_n(b'=', level));
    close.push(b']');

    let body = at + 1;
    let end = bytes[body..]
        .windows(close.len())
        .position(|window| window == close.as_slice());
    Some(match end {
        Some(offset) => body + offset + close.len(),
        None => bytes.len(),
    })
}

fn returned_name(expression: &str) -> (Option<String>, Option<&'static str>) {
    let trimmed = expression.trim();
    if trimmed.starts_with("function") {
        return (None, Some("function"));
    }
    if trimmed.starts_with('{') {
        return (None, Some("table_literal"));
    }
    if let Some(rest) = trimmed.strip_prefix("setmetatable(") {
        let first = rest.split(',').next().unwrap_or_default().trim();
        return (identifier(first), None);
    }
    (identifier(trimmed), None)
}

fn identifier(text: &str) -> Option<String> {
    let trimmed = text.trim().trim_end_matches(')');
    (!trimmed.is_empty()
        && trimmed
            .chars()
            .all(|character| character.is_alphanumeric() || character == '_')
        && !trimmed.starts_with(|first: char| first.is_ascii_digit()))
    .then(|| trimmed.to_string())
}

fn top_level_symbol<'a>(symbols: &'a [SymbolNode], name: &str) -> Option<&'a SymbolNode> {
    symbols
        .iter()
        .find(|node| node.name == name || node.name_path == name)
}

fn export_type_pattern() -> &'static regex::Regex {
    static PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    PATTERN.get_or_init(|| {
        regex::Regex::new(r"(?m)^[ \t]*export[ \t]+type[ \t]+([A-Za-z_][A-Za-z0-9_]*)")
            .expect("the export type pattern is a literal")
    })
}

fn exported_types(source: &str, blanked: &str, limit: usize) -> Vec<ExportedType> {
    let original: Vec<&str> = source.lines().collect();
    let scanned: Vec<&str> = blanked.lines().collect();
    let index = LineIndex::new(blanked);

    let mut found = Vec::new();
    for capture in export_type_pattern().captures_iter(blanked) {
        if found.len() >= limit {
            break;
        }
        let whole = capture.get(0).expect("group zero always matches");
        let start = index.line_of(whole.start());

        let mut depth = 0isize;
        let mut declaration: Vec<&str> = Vec::new();
        for offset in 0..MAX_TYPE_DECLARATION_LINES {
            let Some(line) = scanned.get(start + offset) else {
                break;
            };
            declaration.push(original.get(start + offset).copied().unwrap_or(line));
            depth += bracket_delta(line);
            if depth <= 0 && !continues(line) {
                break;
            }
        }

        found.push(ExportedType {
            name: capture[1].to_string(),
            line: start as u32 + 1,
            declaration: declaration.join("\n").trim_end().to_string(),
        });
    }
    found
}

fn bracket_delta(line: &str) -> isize {
    line.chars().fold(0, |depth, character| match character {
        '{' | '(' | '[' => depth + 1,
        '}' | ')' | ']' => depth - 1,
        _ => depth,
    })
}

fn continues(line: &str) -> bool {
    matches!(
        line.trim_end().chars().next_back(),
        Some('=' | '|' | '&' | ',' | '<')
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_return_inside_a_function_body_is_not_the_modules_own() {
        let source = "local Combat = {}\n\
                      function Combat.hit()\n\
                      \treturn true\n\
                      end\n\
                      return Combat\n";
        assert_eq!(
            module_returns(source),
            vec![(5, "Combat".to_string())],
            "only the chunk's own return counts"
        );
    }

    #[test]
    fn a_return_inside_a_top_level_branch_is_still_the_modules_own() {
        let source = "local IS_SERVER = game:GetService(\"RunService\"):IsServer()\n\
                      if IS_SERVER then\n\
                      \treturn function(name: string)\n\
                      \t\treturn nil\n\
                      \tend\n\
                      else\n\
                      \treturn function(name: string)\n\
                      \t\treturn nil\n\
                      \tend\n\
                      end\n";
        let found = module_returns(source);
        assert_eq!(
            found.iter().map(|(line, _)| *line).collect::<Vec<_>>(),
            vec![3, 7],
            "both branches are reported: {found:?}"
        );
        assert!(
            found
                .iter()
                .all(|(_, expression)| { returned_name(expression).1 == Some("function") })
        );
    }

    #[test]
    fn an_if_then_else_expression_does_not_open_a_block() {
        let source = "local IS_SERVER = true\n\
                      function Remote:OnEvent(callback)\n\
                      \tlocal event = if IS_SERVER then 1 else 2\n\
                      \treturn if callback then 3 else 4\n\
                      end\n\
                      if IS_SERVER then\n\
                      \treturn function() end\n\
                      else\n\
                      \treturn function() end\n\
                      end\n";
        assert_eq!(
            module_returns(source),
            vec![
                (7, "function() end".to_string()),
                (9, "function() end".to_string()),
            ]
        );
    }

    #[test]
    fn an_if_is_a_statement_unless_a_value_is_expected_before_it() {
        let statement = "print()\nif x then\n\treturn 1\nend\n";
        assert_eq!(module_returns(statement), vec![(3, "1".to_string())]);

        let after_call = "local t = f(if x then 1 else 2)\nreturn t\n";
        assert_eq!(module_returns(after_call), vec![(2, "t".to_string())]);
    }

    #[test]
    fn a_return_inside_a_loop_or_a_do_block_is_reached_too() {
        let source = "do\n\treturn 1\nend\n";
        assert_eq!(module_returns(source), vec![(2, "1".to_string())]);

        let nested =
            "for _ = 1, 2 do\n\tlocal f = function()\n\t\treturn 0\n\tend\nend\nreturn {}\n";
        assert_eq!(module_returns(nested), vec![(6, "{}".to_string())]);
    }

    #[test]
    fn a_keyword_inside_a_string_does_not_close_a_block() {
        let source = "local label = \"end\"\n\
                      function noop()\n\
                      \tlocal other = [[end]]\n\
                      \treturn label\n\
                      end\n\
                      return noop\n";
        assert_eq!(module_returns(source), vec![(6, "noop".to_string())]);
    }

    #[test]
    fn a_repeat_block_closes_on_until_rather_than_end() {
        let source = "repeat\n\tlocal x = 1\nuntil true\nreturn x\n";
        assert_eq!(module_returns(source), vec![(4, "x".to_string())]);
    }

    #[test]
    fn a_file_that_returns_nothing_has_no_return_to_find() {
        assert!(module_returns("print(\"hello\")\n").is_empty());
    }

    #[test]
    fn agreement_holds_only_while_every_value_matches() {
        assert_eq!(agreed(["a", "a"].into_iter()), Some("a"));
        assert_eq!(agreed(["a", "b"].into_iter()), None);
        assert_eq!(agreed(std::iter::empty::<&str>()), None);
    }

    #[test]
    fn the_returned_value_is_named_through_the_wrappers_modules_use() {
        assert_eq!(returned_name("Combat").0.as_deref(), Some("Combat"));
        assert_eq!(
            returned_name("setmetatable(Class, Class)").0.as_deref(),
            Some("Class")
        );
        assert_eq!(returned_name("function(a) end").1, Some("function"));
        assert_eq!(returned_name("{").1, Some("table_literal"));
    }

    #[test]
    fn a_returned_expression_that_is_not_a_bare_name_names_nothing() {
        assert!(returned_name("Combat.new").0.is_none());
        assert!(returned_name("require(script.Other)").0.is_none());
    }

    #[test]
    fn an_exported_type_is_quoted_to_the_end_of_its_declaration() {
        let source = "export type Config = {\n\tName: string,\n\tCount: number,\n}\n\
                      export type Id = string\n";
        let found = exported_types(source, source, 10);

        assert_eq!(found.len(), 2);
        assert_eq!(found[0].name, "Config");
        assert_eq!(found[0].line, 1);
        assert!(
            found[0].declaration.ends_with('}'),
            "{:?}",
            found[0].declaration
        );
        assert_eq!(found[1].declaration, "export type Id = string");
    }

    #[test]
    fn a_commented_out_type_is_not_an_export() {
        let source = "-- export type Old = string\nexport type New = number\n";
        let blanked = crate::roblox::requires::blank_comments(source);
        let found = exported_types(source, &blanked, 10);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "New");
        assert_eq!(found[0].line, 2);
    }
}
