use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

impl Range {
    pub fn contains(&self, position: Position) -> bool {
        position >= self.start && position <= self.end
    }

    pub fn overlaps_lines(&self, start_line: u32, end_line: u32) -> bool {
        self.start.line <= end_line && self.end.line >= start_line
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Location {
    pub uri: String,
    pub range: Range,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocationLink {
    pub target_uri: String,
    pub target_selection_range: Range,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum GotoResponse {
    Scalar(Location),
    Array(Vec<Location>),
    Links(Vec<LocationLink>),
}

impl GotoResponse {
    pub fn into_locations(self) -> Vec<Location> {
        match self {
            Self::Scalar(location) => vec![location],
            Self::Array(locations) => locations,
            Self::Links(links) => links
                .into_iter()
                .map(|link| Location {
                    uri: link.target_uri,
                    range: link.target_selection_range,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSymbol {
    pub name: String,
    #[serde(default)]
    pub detail: Option<String>,
    pub kind: u32,
    pub range: Range,
    pub selection_range: Range,
    #[serde(default)]
    pub children: Vec<DocumentSymbol>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SymbolInformation {
    pub name: String,
    pub kind: u32,
    pub location: Location,
    #[serde(default)]
    pub container_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum DocumentSymbolResponse {
    Nested(Vec<DocumentSymbol>),
    Flat(Vec<SymbolInformation>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub range: Range,
    #[serde(default)]
    pub severity: Option<u32>,
    #[serde(default)]
    pub code: Option<Value>,
    #[serde(default)]
    pub source: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentDiagnosticReport {
    #[serde(default)]
    pub items: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error = 1,
    Warning = 2,
    Information = 3,
    Hint = 4,
}

impl Severity {
    pub fn from_code(code: Option<u32>) -> Self {
        match code {
            Some(1) => Self::Error,
            Some(3) => Self::Information,
            Some(4) => Self::Hint,
            _ => Self::Warning,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Information => "information",
            Self::Hint => "hint",
        }
    }
}

/// LSP `SymbolKind`, 1-indexed per the specification.
pub fn symbol_kind_label(kind: u32) -> &'static str {
    match kind {
        1 => "File",
        2 => "Module",
        3 => "Namespace",
        4 => "Package",
        5 => "Class",
        6 => "Method",
        7 => "Property",
        8 => "Field",
        9 => "Constructor",
        10 => "Enum",
        11 => "Interface",
        12 => "Function",
        13 => "Variable",
        14 => "Constant",
        15 => "String",
        16 => "Number",
        17 => "Boolean",
        18 => "Array",
        19 => "Object",
        20 => "Key",
        21 => "Null",
        22 => "EnumMember",
        23 => "Struct",
        24 => "Event",
        25 => "Operator",
        26 => "TypeParameter",
        _ => "Unknown",
    }
}

/// Symbols whose declarations are rarely worth surfacing in a file overview.
pub fn is_low_level_kind(kind: u32) -> bool {
    matches!(kind, 13 | 14 | 15 | 16 | 17 | 18 | 19 | 20 | 21 | 26)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goto_response_normalizes_all_three_shapes() {
        let scalar: GotoResponse =
            serde_json::from_str(r#"{"uri":"file:///a","range":{"start":{"line":1,"character":0},"end":{"line":1,"character":4}}}"#)
                .unwrap();
        assert_eq!(scalar.into_locations().len(), 1);

        let links: GotoResponse = serde_json::from_str(
            r#"[{"targetUri":"file:///a","targetRange":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},"targetSelectionRange":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}}}]"#,
        )
        .unwrap();
        assert_eq!(links.into_locations()[0].uri, "file:///a");
    }

    #[test]
    fn severity_defaults_to_warning() {
        assert_eq!(Severity::from_code(None), Severity::Warning);
        assert_eq!(Severity::from_code(Some(2)), Severity::Warning);
        assert_eq!(Severity::from_code(Some(1)), Severity::Error);
    }
}
