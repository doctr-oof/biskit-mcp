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

#[derive(Debug, Clone, Deserialize)]
pub struct MarkupContent {
    #[serde(default)]
    pub kind: Option<String>,
    pub value: String,
}

/// The deprecated `MarkedString` shape, which luau-lsp may still answer hover with.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum MarkedString {
    Plain(String),
    Fenced { value: String },
}

impl MarkedString {
    fn into_value(self) -> String {
        match self {
            Self::Plain(text) => text,
            Self::Fenced { value } => value,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum HoverContents {
    Markup(MarkupContent),
    Many(Vec<MarkedString>),
    One(MarkedString),
}

impl HoverContents {
    /// The markdown of the hover, with the three shapes the specification allows flattened into
    /// the one an agent can read.
    pub fn into_markdown(self) -> String {
        match self {
            Self::Markup(content) => content.value,
            Self::Many(parts) => parts
                .into_iter()
                .map(MarkedString::into_value)
                .collect::<Vec<_>>()
                .join("\n\n"),
            Self::One(part) => part.into_value(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Hover {
    pub contents: HoverContents,
    #[serde(default)]
    pub range: Option<Range>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InlayHintLabelPart {
    pub value: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum InlayHintLabel {
    Plain(String),
    Parts(Vec<InlayHintLabelPart>),
}

impl InlayHintLabel {
    pub fn into_text(self) -> String {
        match self {
            Self::Plain(text) => text,
            Self::Parts(parts) => parts.into_iter().map(|part| part.value).collect(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InlayHint {
    pub position: Position,
    pub label: InlayHintLabel,
    #[serde(default)]
    pub kind: Option<u32>,
}

/// LSP `InlayHintKind`. Anything else is reported without a kind rather than guessed at.
pub fn inlay_hint_kind_label(kind: Option<u32>) -> Option<&'static str> {
    match kind {
        Some(1) => Some("type"),
        Some(2) => Some("parameter"),
        _ => None,
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Documentation {
    Plain(String),
    Markup(MarkupContent),
}

impl Documentation {
    pub fn into_text(self) -> String {
        match self {
            Self::Plain(text) => text,
            Self::Markup(content) => content.value,
        }
    }
}

/// A parameter is named either by its own text or by a half-open offset pair into the signature
/// label it belongs to.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ParameterLabel {
    Text(String),
    Offsets([u32; 2]),
}

impl ParameterLabel {
    pub fn resolve(&self, signature: &str) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::Offsets([start, end]) => signature
                .chars()
                .skip(*start as usize)
                .take((*end).saturating_sub(*start) as usize)
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParameterInformation {
    pub label: ParameterLabel,
    #[serde(default)]
    pub documentation: Option<Documentation>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureInformation {
    pub label: String,
    #[serde(default)]
    pub documentation: Option<Documentation>,
    #[serde(default)]
    pub parameters: Vec<ParameterInformation>,
    #[serde(default)]
    pub active_parameter: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureHelp {
    #[serde(default)]
    pub signatures: Vec<SignatureInformation>,
    #[serde(default)]
    pub active_signature: Option<u32>,
    #[serde(default)]
    pub active_parameter: Option<u32>,
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
    fn hover_contents_flatten_to_one_markdown_block() {
        let markup: Hover = serde_json::from_str(
            r#"{"contents":{"kind":"markdown","value":"```luau\ntype X\n```"}}"#,
        )
        .unwrap();
        assert_eq!(markup.contents.into_markdown(), "```luau\ntype X\n```");

        let many: Hover =
            serde_json::from_str(r#"{"contents":[{"language":"luau","value":"a"},"b"]}"#).unwrap();
        assert_eq!(many.contents.into_markdown(), "a\n\nb");

        let plain: Hover = serde_json::from_str(r#"{"contents":"number"}"#).unwrap();
        assert_eq!(plain.contents.into_markdown(), "number");
    }

    #[test]
    fn inlay_hint_labels_arrive_as_text_or_as_parts() {
        let plain: InlayHint = serde_json::from_str(
            r#"{"position":{"line":3,"character":9},"label":": number","kind":1}"#,
        )
        .unwrap();
        assert_eq!(plain.label.into_text(), ": number");
        assert_eq!(inlay_hint_kind_label(plain.kind), Some("type"));

        let parts: InlayHint = serde_json::from_str(
            r#"{"position":{"line":0,"character":0},"label":[{"value":"count"},{"value":": "}]}"#,
        )
        .unwrap();
        assert_eq!(parts.label.into_text(), "count: ");
        assert_eq!(inlay_hint_kind_label(parts.kind), None);
    }

    #[test]
    fn a_parameter_named_by_offsets_is_cut_out_of_its_signature() {
        let label: ParameterLabel = serde_json::from_str("[10, 24]").unwrap();
        assert_eq!(
            label.resolve("function (player: Player, amount: number)"),
            "player: Player"
        );

        let text: ParameterLabel = serde_json::from_str(r#""amount: number""#).unwrap();
        assert_eq!(text.resolve("ignored"), "amount: number");
    }

    #[test]
    fn severity_defaults_to_warning() {
        assert_eq!(Severity::from_code(None), Severity::Warning);
        assert_eq!(Severity::from_code(Some(2)), Severity::Warning);
        assert_eq!(Severity::from_code(Some(1)), Severity::Error);
    }
}
