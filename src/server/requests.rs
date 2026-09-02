use rmcp::schemars;
use serde::Deserialize;

use crate::files::SearchMode;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct MemoryNameRequest {
    /// Memory name, without the .md extension. Nest with `/`.
    pub memory_name: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateMemoryRequest {
    /// Memory name, without the .md extension. Nest with `/`.
    pub memory_name: String,
    /// Markdown body. Reference other memories with `mem:name` in backticks.
    pub content: String,
    /// Replace a memory that already exists. Prefer edit_memory over a wholesale rewrite.
    #[serde(default)]
    pub overwrite: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct EditMemoryRequest {
    pub memory_name: String,
    /// Regular expression matched against the memory body.
    pub pattern: String,
    /// Replacement text. Capture groups are available as `$1`, `$2`, and `${name}`; write `$$` for
    /// a literal dollar sign.
    pub replacement: String,
    /// Replace every match instead of erroring when the pattern is ambiguous.
    #[serde(default)]
    pub allow_multiple_occurrences: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RenameMemoryRequest {
    pub old_name: String,
    pub new_name: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListDirRequest {
    /// Directory relative to the project root. Use "." for the root itself.
    pub relative_path: String,
    /// Descend into subdirectories.
    pub recursive: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FindFileRequest {
    /// Filename glob, for example "*.luau" or "init.*".
    pub file_mask: String,
    /// Directory to search under, relative to the project root.
    #[serde(default = "project_root")]
    pub relative_path: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchForPatternRequest {
    /// Regular expression matched against file contents.
    pub substring_pattern: String,
    #[serde(default)]
    pub context_lines_before: usize,
    #[serde(default)]
    pub context_lines_after: usize,
    /// Restrict to paths matching this glob, for example "src/**".
    #[serde(default)]
    pub paths_include_glob: Option<String>,
    /// Skip paths matching this glob. Takes precedence over the include glob.
    #[serde(default)]
    pub paths_exclude_glob: Option<String>,
    /// Directory or file to search under, relative to the project root.
    #[serde(default = "project_root")]
    pub relative_path: String,
    /// Only search .luau, .lua, and .luaurc files.
    #[serde(default)]
    pub restrict_search_to_code_files: bool,
    /// How much to report: "snippets" (the default) returns the matching lines, "files" returns
    /// only the paths that match, "counts" returns a match count per file.
    #[serde(default)]
    pub mode: SearchOutputMode,
    /// Match without regard to case.
    #[serde(default)]
    pub case_insensitive: bool,
    /// Let "." match a newline, so one pattern can span lines. Off by default, because with it on
    /// a plain ".*" runs to the end of the file and returns the whole file as one match.
    #[serde(default)]
    pub dot_matches_newline: bool,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchOutputMode {
    #[default]
    Snippets,
    Files,
    Counts,
}

impl SearchOutputMode {
    pub(super) fn as_mode(self) -> SearchMode {
        match self {
            Self::Snippets => SearchMode::Snippets,
            Self::Files => SearchMode::Files,
            Self::Counts => SearchMode::Counts,
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SymbolsOverviewRequest {
    /// Luau source file relative to the project root.
    pub relative_path: String,
    /// How many levels of nested symbols to include. 0 lists top-level symbols only. Defaults to
    /// 1, which is where the members of a table live.
    #[serde(default = "default_overview_depth")]
    pub depth: u32,
    /// Include each symbol's resolved type signature. Off by default because signatures are long
    /// and each one costs the language server a request.
    #[serde(default)]
    pub include_detail: bool,
    /// Include variables declared inside a function body. Off by default, where each symbol
    /// instead reports how many children were left out as "omitted_children".
    #[serde(default)]
    pub include_locals: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FindSymbolRequestInput {
    /// Name path such as "update", "PlayerService/update", "PlayerService:update", or
    /// "/PlayerService". Append "[n]" to a segment to pick one of several same-named symbols.
    pub name_path: String,
    /// File or directory to search. Omit to search the whole project.
    #[serde(default)]
    pub relative_path: Option<String>,
    /// Levels of children to include alongside each match.
    #[serde(default)]
    pub depth: u32,
    /// Include each matched symbol's source text.
    #[serde(default)]
    pub include_body: bool,
    /// Include each symbol's resolved type signature. Off by default because signatures are long
    /// and each one costs the language server a request.
    #[serde(default)]
    pub include_detail: bool,
    /// Include variables declared inside a function body. Off by default, where each symbol
    /// instead reports how many children were left out as "omitted_children".
    #[serde(default)]
    pub include_locals: bool,
    /// LSP SymbolKind numbers to keep. Empty means all kinds.
    #[serde(default)]
    pub include_kinds: Vec<u32>,
    /// LSP SymbolKind numbers to drop.
    #[serde(default)]
    pub exclude_kinds: Vec<u32>,
    /// Match the final name path segment as a substring.
    #[serde(default)]
    pub substring_matching: bool,
    /// Cap on returned matches.
    #[serde(default = "default_max_matches")]
    pub max_matches: usize,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SymbolLocationRequest {
    /// Name path of the symbol. Append "[n]" to a segment to pick one of several same-named symbols.
    pub name_path: String,
    /// File containing the symbol, relative to the project root.
    pub relative_path: String,
    /// Source lines to show either side of each reference. 0 shows the reference line alone.
    #[serde(default)]
    pub context_lines: usize,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FindDeclarationRequest {
    /// Name path of a symbol the file itself declares. Append "[n]" to a segment to pick one of
    /// several same-named symbols. Omit to point with line and column instead, which is how a
    /// symbol declared in another file is asked about.
    #[serde(default)]
    pub name_path: Option<String>,
    /// File containing the symbol, relative to the project root.
    pub relative_path: String,
    /// 1-based line of a use of the symbol. Omit when name_path is given.
    #[serde(default)]
    pub line: Option<u32>,
    /// 1-based column on that line. Defaults to the start of the line.
    #[serde(default)]
    pub column: Option<u32>,
    /// Include a source snippet around each result.
    #[serde(default)]
    pub include_body: bool,
    /// Include each symbol's type signature. Off by default because signatures are long.
    #[serde(default)]
    pub include_detail: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FileDiagnosticsRequest {
    pub relative_path: String,
    /// First line to report on, 1-based.
    #[serde(default)]
    pub start_line: Option<u32>,
    /// Last line to report on, 1-based.
    #[serde(default)]
    pub end_line: Option<u32>,
    /// 1 error, 2 warning, 3 information, 4 hint. Defaults to 2.
    #[serde(default)]
    pub min_severity: Option<u32>,
    /// Re-read the file even when its size and modification time say it has not moved. Only worth
    /// setting when an edit that kept the file's length appears to have been missed.
    #[serde(default)]
    pub refresh: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SymbolDiagnosticsRequest {
    pub name_path: String,
    pub relative_path: String,
    /// Also report diagnostics in every file that references this symbol, the declaring file
    /// included, each swept whole rather than clipped to the symbol.
    #[serde(default)]
    pub check_symbol_references: bool,
    /// 1 error, 2 warning, 3 information, 4 hint. Defaults to 2.
    #[serde(default)]
    pub min_severity: Option<u32>,
    /// Re-read the files even when their size and modification time say they have not moved. Only
    /// worth setting when an edit that kept a file's length appears to have been missed.
    #[serde(default)]
    pub refresh: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ExplainSymbolRequest {
    /// Name path of the symbol. Omit to point with line and column instead.
    #[serde(default)]
    pub name_path: Option<String>,
    /// File containing the position, relative to the project root.
    pub relative_path: String,
    /// 1-based line, used instead of name_path. Aim it at a use of the symbol.
    #[serde(default)]
    pub line: Option<u32>,
    /// 1-based column on that line. Defaults to 1.
    #[serde(default)]
    pub column: Option<u32>,
    /// Include the doc comment alongside the type. Off by default because docs are long.
    #[serde(default)]
    pub include_documentation: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SignatureHelpRequest {
    /// File containing the call, relative to the project root.
    pub relative_path: String,
    /// 1-based line of the call. A declaration is never inside a call's parentheses, so this
    /// tool takes a position only, never a name path.
    pub line: u32,
    /// 1-based column on that line, inside the call's parentheses. Defaults to 1.
    #[serde(default)]
    pub column: Option<u32>,
    /// Include the doc comment alongside the type. Off by default because docs are long.
    #[serde(default)]
    pub include_documentation: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TypeDefinitionRequest {
    /// Name path of a type. Omit to point with line and column instead, which is what a type
    /// written as an annotation needs.
    #[serde(default)]
    pub name_path: Option<String>,
    /// File containing the position, relative to the project root.
    pub relative_path: String,
    /// 1-based line. Aim it at the type's own name: in `local config: PlayerConfig`, at
    /// `PlayerConfig` rather than at `config`.
    #[serde(default)]
    pub line: Option<u32>,
    /// 1-based column on that line. Defaults to 1.
    #[serde(default)]
    pub column: Option<u32>,
    /// Include a source snippet around each result.
    #[serde(default)]
    pub include_body: bool,
    /// Include each symbol's type signature. Off by default because signatures are long.
    #[serde(default)]
    pub include_detail: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct InlayHintsRequest {
    /// Luau source file relative to the project root.
    pub relative_path: String,
    /// First line to report on, 1-based. Defaults to the start of the file.
    #[serde(default)]
    pub start_line: Option<u32>,
    /// Last line to report on, 1-based. Defaults to the end of the file.
    #[serde(default)]
    pub end_line: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ResolveInstancePathRequest {
    /// DataModel path such as "game.ReplicatedStorage.Shared.Combat". Omit to translate a file
    /// path instead.
    #[serde(default)]
    pub instance_path: Option<String>,
    /// Luau file relative to the project root, such as "src/Shared/Combat/init.luau". Omit to
    /// translate an instance path instead.
    #[serde(default)]
    pub relative_path: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RequireGraphRequest {
    /// Module to centre the graph on, relative to the project root. Omit for a project-wide
    /// answer, which reports cycles and unresolved requires rather than every edge.
    #[serde(default)]
    pub relative_path: Option<String>,
    /// "dependencies" for what it requires, "dependents" for what requires it, or "both".
    #[serde(default)]
    pub direction: Option<String>,
    /// How many hops to follow. 1 is direct edges only.
    #[serde(default = "default_graph_depth")]
    pub depth: u32,
    /// Report require cycles. On by default for a project-wide answer.
    #[serde(default)]
    pub include_cycles: Option<bool>,
    /// Report requires that could not be resolved statically.
    #[serde(default = "default_true")]
    pub include_unresolved: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ModuleContextRequest {
    /// Luau file relative to the project root.
    pub relative_path: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RobloxApiRequest {
    /// A class ("BasePart"), a service ("TweenService"), a member ("TweenService:Create" or
    /// "BasePart.Anchored"), or an enum ("Enum.EasingStyle").
    pub query: String,
    /// Keep only members whose name contains this, case-insensitively.
    #[serde(default)]
    pub member_filter: Option<String>,
    /// Include members a class inherits from its ancestors. Off by default because Instance alone
    /// carries dozens.
    #[serde(default)]
    pub include_inherited: bool,
    /// Include the documentation prose. Off by default for a class listing; a single member
    /// carries it regardless.
    #[serde(default)]
    pub include_documentation: bool,
    /// Cap on members returned for a class or items for an enum.
    #[serde(default = "default_max_members")]
    pub max_members: usize,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchWallyPackagesRequest {
    /// What to match. The registry searches scope, name, and description, so "promise" and
    /// "evaera" both work.
    pub query: String,
    /// Cap on returned packages, 1 or more. Omit to take the wally.max_search_results default; 0 is
    /// refused rather than returning nothing.
    #[serde(default)]
    pub max_results: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AddWallyPackageRequest {
    /// The package as "scope/name", such as "roblox/roact". A version may be written inline as
    /// "scope/name@^1.4.2" instead of passing version.
    pub package: String,
    /// Semver requirement to pin, such as "^1.4.2". Omit to take the newest published release,
    /// which costs one registry lookup. Prereleases are never chosen for you.
    #[serde(default)]
    pub version: Option<String>,
    /// "shared" for [dependencies], "server" for [server-dependencies], "dev" for
    /// [dev-dependencies]. Defaults to "shared". "server" and "dev" need wally.toml to carry a
    /// [place] shared-packages entry before Wally will install a package that depends on a shared
    /// one, which most do.
    #[serde(default)]
    pub realm: Option<String>,
    /// The name the package is required by, which is its name under Packages/. Defaults to the
    /// package's own name in PascalCase: "roblox/roact" becomes "Roact".
    #[serde(default)]
    pub alias: Option<String>,
    /// Replace an existing declaration of the same package instead of erroring. A package is
    /// declared in one realm only, so this is also what moves one between realms.
    #[serde(default)]
    pub overwrite: bool,
    /// Run `wally install` after editing wally.toml. On by default. Setting it false leaves the
    /// manifest and the package tree disagreeing, which is only useful when adding several
    /// packages before one install.
    #[serde(default = "default_true")]
    pub install: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RemoveWallyPackageRequest {
    /// The package as "scope/name", or the alias it was declared under.
    pub package: String,
    /// Which section to remove it from: "shared", "server", or "dev". Leave it off in normal use:
    /// add_wally_package keeps a package in one realm, so this is only for a hand written
    /// wally.toml that declares the same name twice.
    #[serde(default)]
    pub realm: Option<String>,
    /// Run `wally install` after editing wally.toml. On by default; without it the removed
    /// package stays on disk.
    #[serde(default = "default_true")]
    pub install: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct NoArguments {}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct InitialInstructionsRequest {
    /// Send the manual even when a SessionStart hook already delivered it. Only set this when the
    /// manual is genuinely absent from your context.
    #[serde(default)]
    pub force: bool,
}

fn project_root() -> String {
    ".".to_string()
}

fn default_max_matches() -> usize {
    50
}

fn default_overview_depth() -> u32 {
    1
}

fn default_graph_depth() -> u32 {
    1
}

fn default_max_members() -> usize {
    200
}

fn default_true() -> bool {
    true
}
