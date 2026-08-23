use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerInfo};
use rmcp::{ServerHandler, schemars, tool, tool_handler, tool_router};
use serde::Deserialize;
use serde::Serialize;

use crate::config::Settings;
use crate::errors;
use crate::files::{FileTools, PatternSearchRequest};
use crate::lsp::queries::{FindSymbolRequest, SymbolPoint, SymbolQuery, severity_from_input};
use crate::lsp::session::LanguageServerHandle;
use crate::memory::MemoryStore;
use crate::project::Project;
use crate::roblox::RobloxIndex;
use crate::roblox::api::ApiQuery;
use crate::roblox::context;
use crate::roblox::requires::{Direction, GraphRequest};
use crate::{prompts, status};

#[derive(Clone)]
pub struct Biskit {
    inner: Arc<Inner>,
    tool_router: ToolRouter<Biskit>,
}

struct Inner {
    settings: Settings,
    memories: MemoryStore,
    files: FileTools,
    language_server: Arc<LanguageServerHandle>,
    /// Sourcemap, require graph, and Roblox API, each built once and reused.
    roblox: Arc<RobloxIndex>,
    /// How the project root was chosen, reported by `get_status`.
    root_source: &'static str,
}

/// Tool failures travel back as `isError` results rather than JSON-RPC errors, so clients render
/// the message itself instead of an `MCP error -32602:` envelope.
type ToolResult = Result<CallToolResult, String>;

fn fail(tool: &'static str) -> impl Fn(anyhow::Error) -> String {
    move |error| errors::render(tool, &error)
}

const OVERRUN_HINT: &str = "ask for less: narrow relative_path, lower max_matches, drop \
                            include_body, or raise tools.max_answer_chars in .biskit/settings.yml";

/// Largest prefix of `value` that fits in `limit` bytes without splitting a character.
fn truncate_at_char_boundary(value: &str, limit: usize) -> &str {
    if value.len() <= limit {
        return value;
    }
    let mut end = limit;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

impl Biskit {
    /// Ceiling on the size of one tool result, in bytes. Zero disables it.
    fn answer_limit(&self) -> usize {
        self.inner.settings.tools.max_answer_chars
    }

    /// Results are serialised compactly: pretty printing costs the caller a newline and a growing
    /// indent per field for no information gain.
    ///
    /// An oversized result is refused rather than truncated, because half a JSON document is not
    /// readable at all, and the refusal names what to narrow.
    fn ok<T: Serialize>(&self, tool: &'static str, value: &T) -> ToolResult {
        let rendered = serde_json::to_string(value)
            .map_err(|error| format!("failed to serialise the tool result: {error}"))?;

        let limit = self.answer_limit();
        if limit > 0 && rendered.len() > limit {
            let overrun = errors::hinted(
                format!(
                    "the result is {} characters, over the tools.max_answer_chars limit of {limit}",
                    rendered.len()
                ),
                OVERRUN_HINT,
            );
            return Err(errors::render(tool, &overrun));
        }
        Ok(CallToolResult::success(vec![ContentBlock::text(rendered)]))
    }

    /// Prose survives being cut in a way JSON does not, so an oversized text result is truncated
    /// and says so rather than being refused outright.
    fn text(&self, value: impl Into<String>) -> ToolResult {
        let value = value.into();
        let limit = self.answer_limit();

        let rendered = if limit > 0 && value.len() > limit {
            format!(
                "{}\n\n[truncated: {limit} of {} characters shown, limited by \
                 tools.max_answer_chars]",
                truncate_at_char_boundary(&value, limit),
                value.len()
            )
        } else {
            value
        };
        Ok(CallToolResult::success(vec![ContentBlock::text(rendered)]))
    }
}

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
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SymbolsOverviewRequest {
    /// Luau source file relative to the project root.
    pub relative_path: String,
    /// How many levels of nested symbols to include. 0 lists top-level symbols only. Defaults to
    /// 1, which is where the members of a table live.
    #[serde(default = "default_overview_depth")]
    pub depth: u32,
    /// Include each symbol's type signature. Off by default because signatures are long.
    #[serde(default)]
    pub include_detail: bool,
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
    /// Include each symbol's type signature. Off by default because signatures are long.
    #[serde(default)]
    pub include_detail: bool,
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
    /// Name path of the symbol. Append "[n]" to a segment to pick one of several same-named symbols.
    pub name_path: String,
    /// File containing the symbol, relative to the project root.
    pub relative_path: String,
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
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SymbolDiagnosticsRequest {
    pub name_path: String,
    pub relative_path: String,
    /// Also report diagnostics in every file that references this symbol.
    #[serde(default)]
    pub check_symbol_references: bool,
    /// 1 error, 2 warning, 3 information, 4 hint. Defaults to 2.
    #[serde(default)]
    pub min_severity: Option<u32>,
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
pub struct PlanSymbolRenameRequest {
    /// Name path of the symbol to rename. Append "[n]" to a segment to pick one of several
    /// same-named symbols.
    pub name_path: String,
    /// File containing the symbol, relative to the project root.
    pub relative_path: String,
    /// The new name. Must be a valid Luau identifier.
    pub new_name: String,
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
pub struct ModuleApiRequest {
    /// ModuleScript relative to the project root.
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
pub struct NoArguments {}

/// Tools backed by the language server. Memory-only mode drops these routes entirely.
///
/// The Roblox tools are here despite three of them never speaking to luau-lsp, because everything
/// they read is downloaded and kept up to date for the language server's sake. In memory-only mode
/// there is no sourcemap loaded and no type definition cache to answer from, so routing them would
/// only offer an agent five tools that each fail the same way.
const LANGUAGE_SERVER_TOOLS: [&str; 17] = [
    "get_symbols_overview",
    "find_symbol",
    "find_declaration",
    "find_referencing_symbols",
    "get_file_diagnostics",
    "get_symbol_diagnostics",
    "restart_language_server",
    "explain_symbol",
    "get_type_definition",
    "get_inlay_hints",
    "get_signature_help",
    "plan_symbol_rename",
    "resolve_instance_path",
    "get_require_graph",
    "get_module_context",
    "get_module_api",
    "query_roblox_api",
];

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

#[tool_router]
impl Biskit {
    pub fn new(project: Project, settings: Settings, root_source: &'static str) -> Self {
        let memories = MemoryStore::new(project.clone());
        let files = FileTools::new(project.clone(), settings.clone());
        let roblox = Arc::new(RobloxIndex::new(project.clone(), settings.clone()));
        let language_server = Arc::new(LanguageServerHandle::new(project, settings.clone()));

        let memory_only = settings.project.memory_only;
        let mut tool_router = Self::tool_router();
        if memory_only {
            for name in LANGUAGE_SERVER_TOOLS {
                tool_router.remove_route(name);
            }
            tracing::info!(
                target: "biskit",
                "memory-only mode: the language server and its {} tools are disabled",
                LANGUAGE_SERVER_TOOLS.len()
            );
        }

        for excluded in &settings.tools.excluded {
            if tool_router.has_route(excluded) {
                tool_router.remove_route(excluded);
            } else if !(memory_only && LANGUAGE_SERVER_TOOLS.contains(&excluded.as_str())) {
                tracing::warn!(
                    target: "biskit",
                    "tools.excluded lists an unknown tool: {excluded}"
                );
            }
        }

        Self {
            inner: Arc::new(Inner {
                settings,
                memories,
                files,
                language_server,
                roblox,
                root_source,
            }),
            tool_router,
        }
    }

    /// Brings the language server up in the background so the first LSP-backed tool call does not
    /// pay for its startup. Does nothing in memory-only mode.
    pub fn warm_up(&self) {
        self.inner.language_server.warm_up();
    }

    pub async fn shutdown(&self) {
        self.inner.language_server.stop().await;
    }

    #[tool(
        description = "Returns Biskit's usage manual and the index of memories stored for this project. Call this before using any other Biskit tool."
    )]
    async fn initial_instructions(
        &self,
        Parameters(NoArguments {}): Parameters<NoArguments>,
    ) -> ToolResult {
        let memories = self
            .inner
            .memories
            .list()
            .map_err(fail("initial_instructions"))?;
        self.text(prompts::initial_instructions(
            &memories,
            self.inner.settings.project.memory_only,
        ))
    }

    #[tool(description = "Lists the names of every memory stored for this project.")]
    async fn list_memories(
        &self,
        Parameters(NoArguments {}): Parameters<NoArguments>,
    ) -> ToolResult {
        self.ok(
            "list_memories",
            &self.inner.memories.list().map_err(fail("list_memories"))?,
        )
    }

    #[tool(description = "Reads the full markdown content of one memory.")]
    async fn read_memory(&self, Parameters(request): Parameters<MemoryNameRequest>) -> ToolResult {
        self.text(
            self.inner
                .memories
                .read(&request.memory_name)
                .map_err(fail("read_memory"))?,
        )
    }

    #[tool(
        description = "Writes a memory recording durable knowledge about this project, in markdown. Use a meaningful, nestable name. Errors if the name is taken unless overwrite is set."
    )]
    async fn create_memory(
        &self,
        Parameters(request): Parameters<CreateMemoryRequest>,
    ) -> ToolResult {
        let outcome = self
            .inner
            .memories
            .create(&request.memory_name, &request.content, request.overwrite)
            .map_err(fail("create_memory"))?;
        let verb = if outcome.replaced {
            "Replaced"
        } else {
            "Wrote"
        };
        self.text(format!("{verb} memory {}.", outcome.memory))
    }

    #[tool(description = "Deletes a memory.")]
    async fn delete_memory(
        &self,
        Parameters(request): Parameters<MemoryNameRequest>,
    ) -> ToolResult {
        let name = self
            .inner
            .memories
            .delete(&request.memory_name)
            .map_err(fail("delete_memory"))?;
        self.text(format!("Deleted memory {name}."))
    }

    #[tool(
        description = "Replaces content matching a regular expression inside an existing memory. Prefer this over rewriting a memory wholesale."
    )]
    async fn edit_memory(&self, Parameters(request): Parameters<EditMemoryRequest>) -> ToolResult {
        let outcome = self
            .inner
            .memories
            .edit(
                &request.memory_name,
                &request.pattern,
                &request.replacement,
                request.allow_multiple_occurrences,
            )
            .map_err(fail("edit_memory"))?;
        self.text(format!(
            "Replaced {} occurrence(s) in memory {}.",
            outcome.replacements, outcome.memory
        ))
    }

    #[tool(
        description = "Renames or moves a memory, rewriting every `mem:` reference to it in other memories."
    )]
    async fn rename_memory(
        &self,
        Parameters(request): Parameters<RenameMemoryRequest>,
    ) -> ToolResult {
        let outcome = self
            .inner
            .memories
            .rename(&request.old_name, &request.new_name)
            .map_err(fail("rename_memory"))?;
        self.ok(
            "rename_memory",
            &serde_json::json!({
                "from": outcome.from,
                "to": outcome.to,
                "updated_references": outcome.updated_references,
            }),
        )
    }

    #[tool(description = "Lists files and directories under a project-relative path.")]
    async fn list_dir(&self, Parameters(request): Parameters<ListDirRequest>) -> ToolResult {
        self.ok(
            "list_dir",
            &self
                .inner
                .files
                .list_dir(&request.relative_path, request.recursive)
                .map_err(fail("list_dir"))?,
        )
    }

    #[tool(description = "Finds files whose name matches a glob mask.")]
    async fn find_file(&self, Parameters(request): Parameters<FindFileRequest>) -> ToolResult {
        self.ok(
            "find_file",
            &self
                .inner
                .files
                .find_file(&request.file_mask, &request.relative_path)
                .map_err(fail("find_file"))?,
        )
    }

    #[tool(
        description = "Searches file contents with a regular expression. Use this for text that is not a symbol; use find_symbol for definitions."
    )]
    async fn search_for_pattern(
        &self,
        Parameters(request): Parameters<SearchForPatternRequest>,
    ) -> ToolResult {
        let result = self
            .inner
            .files
            .search_for_pattern(PatternSearchRequest {
                pattern: &request.substring_pattern,
                relative_path: &request.relative_path,
                context_lines_before: request.context_lines_before,
                context_lines_after: request.context_lines_after,
                paths_include_glob: request.paths_include_glob.as_deref(),
                paths_exclude_glob: request.paths_exclude_glob.as_deref(),
                restrict_to_code_files: request.restrict_search_to_code_files,
                max_matches: self.inner.settings.tools.max_pattern_matches,
            })
            .map_err(fail("search_for_pattern"))?;
        self.ok("search_for_pattern", &result)
    }

    #[tool(
        description = "Lists the symbols defined in a Luau file. Use this before reading a file to decide what is worth reading."
    )]
    async fn get_symbols_overview(
        &self,
        Parameters(request): Parameters<SymbolsOverviewRequest>,
    ) -> ToolResult {
        let query = SymbolQuery::new(&self.inner.language_server);
        self.ok(
            "get_symbols_overview",
            &query
                .symbols_overview(
                    &request.relative_path,
                    request.depth,
                    request.include_detail,
                )
                .await
                .map_err(fail("get_symbols_overview"))?,
        )
    }

    #[tool(
        description = "Finds symbols by name path across the project or within one file or directory."
    )]
    async fn find_symbol(
        &self,
        Parameters(request): Parameters<FindSymbolRequestInput>,
    ) -> ToolResult {
        let query = SymbolQuery::new(&self.inner.language_server);
        self.ok(
            "find_symbol",
            &query
                .find_symbol(FindSymbolRequest {
                    name_path: request.name_path,
                    relative_path: request.relative_path,
                    depth: request.depth,
                    include_body: request.include_body,
                    include_detail: request.include_detail,
                    include_kinds: request.include_kinds,
                    exclude_kinds: request.exclude_kinds,
                    substring_matching: request.substring_matching,
                    max_matches: request
                        .max_matches
                        .min(self.inner.settings.tools.max_listing_entries),
                })
                .await
                .map_err(fail("find_symbol"))?,
        )
    }

    #[tool(description = "Finds where a symbol is declared.")]
    async fn find_declaration(
        &self,
        Parameters(request): Parameters<FindDeclarationRequest>,
    ) -> ToolResult {
        let query = SymbolQuery::new(&self.inner.language_server);
        self.ok(
            "find_declaration",
            &query
                .find_declaration(
                    &request.name_path,
                    &request.relative_path,
                    request.include_body,
                    request.include_detail,
                )
                .await
                .map_err(fail("find_declaration"))?,
        )
    }

    #[tool(description = "Finds every symbol that references the given symbol.")]
    async fn find_referencing_symbols(
        &self,
        Parameters(request): Parameters<SymbolLocationRequest>,
    ) -> ToolResult {
        let query = SymbolQuery::new(&self.inner.language_server);
        self.ok(
            "find_referencing_symbols",
            &query
                .find_referencing_symbols(
                    &request.name_path,
                    &request.relative_path,
                    self.inner.settings.tools.max_reference_matches,
                    request.context_lines,
                )
                .await
                .map_err(fail("find_referencing_symbols"))?,
        )
    }

    #[tool(
        description = "Gets diagnostics for a file, optionally limited to a line range, grouped by severity and containing symbol."
    )]
    async fn get_file_diagnostics(
        &self,
        Parameters(request): Parameters<FileDiagnosticsRequest>,
    ) -> ToolResult {
        let severity =
            severity_from_input(request.min_severity).map_err(fail("get_file_diagnostics"))?;
        let query = SymbolQuery::new(&self.inner.language_server);
        self.ok(
            "get_file_diagnostics",
            &query
                .file_diagnostics(
                    &request.relative_path,
                    request.start_line,
                    request.end_line,
                    severity,
                )
                .await
                .map_err(fail("get_file_diagnostics"))?,
        )
    }

    #[tool(
        description = "Gets diagnostics for one symbol and, optionally, for every file that references it. Use after editing a symbol."
    )]
    async fn get_symbol_diagnostics(
        &self,
        Parameters(request): Parameters<SymbolDiagnosticsRequest>,
    ) -> ToolResult {
        let severity =
            severity_from_input(request.min_severity).map_err(fail("get_symbol_diagnostics"))?;
        let query = SymbolQuery::new(&self.inner.language_server);
        self.ok(
            "get_symbol_diagnostics",
            &query
                .symbol_diagnostics(
                    &request.name_path,
                    &request.relative_path,
                    request.check_symbol_references,
                    severity,
                )
                .await
                .map_err(fail("get_symbol_diagnostics"))?,
        )
    }

    #[tool(
        description = "Explains what a symbol resolves to: the type the language server inferred for it, not the type written in the source. Point at it with name_path or with line and column."
    )]
    async fn explain_symbol(
        &self,
        Parameters(request): Parameters<ExplainSymbolRequest>,
    ) -> ToolResult {
        let point = SymbolPoint::parse(request.name_path.as_deref(), request.line, request.column)
            .map_err(fail("explain_symbol"))?;

        let query = SymbolQuery::new(&self.inner.language_server);
        self.ok(
            "explain_symbol",
            &query
                .explain_symbol(point, &request.relative_path, request.include_documentation)
                .await
                .map_err(fail("explain_symbol"))?,
        )
    }

    #[tool(
        description = "Finds where a type is declared, which is usually an `export type` in another module. Point line and column at the type's own name in an annotation. Use find_declaration instead for where a value is declared."
    )]
    async fn get_type_definition(
        &self,
        Parameters(request): Parameters<TypeDefinitionRequest>,
    ) -> ToolResult {
        let point = SymbolPoint::parse(request.name_path.as_deref(), request.line, request.column)
            .map_err(fail("get_type_definition"))?;

        let query = SymbolQuery::new(&self.inner.language_server);
        self.ok(
            "get_type_definition",
            &query
                .type_definition(
                    point,
                    &request.relative_path,
                    request.include_body,
                    request.include_detail,
                )
                .await
                .map_err(fail("get_type_definition"))?,
        )
    }

    #[tool(
        description = "Lists the inferred types the language server would draw inline over a line range. The cheapest way to see what a function's variables, arguments, and returns resolve to without reading its body."
    )]
    async fn get_inlay_hints(
        &self,
        Parameters(request): Parameters<InlayHintsRequest>,
    ) -> ToolResult {
        let query = SymbolQuery::new(&self.inner.language_server);
        self.ok(
            "get_inlay_hints",
            &query
                .inlay_hints(
                    &request.relative_path,
                    request.start_line,
                    request.end_line,
                    self.inner.settings.tools.max_listing_entries,
                )
                .await
                .map_err(fail("get_inlay_hints"))?,
        )
    }

    #[tool(
        description = "Reports the parameters of a call without reading the callee. Aim line and column inside the parentheses of the call; the result names which argument that position is."
    )]
    async fn get_signature_help(
        &self,
        Parameters(request): Parameters<ExplainSymbolRequest>,
    ) -> ToolResult {
        let point = SymbolPoint::parse(request.name_path.as_deref(), request.line, request.column)
            .map_err(fail("get_signature_help"))?;

        let query = SymbolQuery::new(&self.inner.language_server);
        self.ok(
            "get_signature_help",
            &query
                .signature_help(point, &request.relative_path, request.include_documentation)
                .await
                .map_err(fail("get_signature_help"))?,
        )
    }

    #[tool(
        description = "Plans a symbol rename across the project and returns the edits without applying any of them. Apply them yourself with your own edit tools, working upwards from the last edit in each file. Use this instead of renaming by search and replace."
    )]
    async fn plan_symbol_rename(
        &self,
        Parameters(request): Parameters<PlanSymbolRenameRequest>,
    ) -> ToolResult {
        let query = SymbolQuery::new(&self.inner.language_server);
        self.ok(
            "plan_symbol_rename",
            &query
                .plan_rename(
                    &request.name_path,
                    &request.relative_path,
                    &request.new_name,
                    self.inner.settings.tools.max_reference_matches,
                )
                .await
                .map_err(fail("plan_symbol_rename"))?,
        )
    }

    #[tool(
        description = "Translates between the Roblox DataModel and the files on disk, in either direction. Pass instance_path to find the file behind game.ReplicatedStorage.Shared.Combat, or relative_path to find where a file ends up in the game. Use this before assuming a file's instance path."
    )]
    async fn resolve_instance_path(
        &self,
        Parameters(request): Parameters<ResolveInstancePathRequest>,
    ) -> ToolResult {
        let sourcemap = self
            .inner
            .roblox
            .sourcemap()
            .await
            .map_err(fail("resolve_instance_path"))?;
        self.ok(
            "resolve_instance_path",
            &sourcemap
                .resolve(
                    request.instance_path.as_deref(),
                    request.relative_path.as_deref(),
                )
                .map_err(fail("resolve_instance_path"))?,
        )
    }

    #[tool(
        description = "Reports which modules a module requires and which modules require it, resolved through the sourcemap rather than by text search. Omit relative_path for a project-wide answer naming every require cycle and every require that could not be resolved. Use this before editing a module, because find_referencing_symbols sees symbol uses and not module-level coupling."
    )]
    async fn get_require_graph(
        &self,
        Parameters(request): Parameters<RequireGraphRequest>,
    ) -> ToolResult {
        let directions =
            Direction::parse(request.direction.as_deref()).map_err(fail("get_require_graph"))?;
        let sourcemap = self
            .inner
            .roblox
            .sourcemap()
            .await
            .map_err(fail("get_require_graph"))?;
        let graph = self
            .inner
            .roblox
            .require_graph()
            .await
            .map_err(fail("get_require_graph"))?;

        self.ok(
            "get_require_graph",
            &graph
                .answer(
                    GraphRequest {
                        relative_path: request.relative_path.as_deref(),
                        directions,
                        depth: request.depth,
                        // A project-wide answer with no cycles in it would report almost nothing,
                        // which is the one question it exists to answer.
                        include_cycles: request
                            .include_cycles
                            .unwrap_or(request.relative_path.is_none()),
                        include_unresolved: request.include_unresolved,
                        limit: self.inner.settings.tools.max_listing_entries,
                    },
                    &sourcemap,
                )
                .map_err(fail("get_require_graph"))?,
        )
    }

    #[tool(
        description = "Orients you on one module in a single call: its instance path, whether it runs on the server, the client, or both, what it requires, what requires it, what its returned table exposes, and how many diagnostics it carries. Call this when you open a module you have not seen before, instead of four or five separate calls."
    )]
    async fn get_module_context(
        &self,
        Parameters(request): Parameters<ModuleContextRequest>,
    ) -> ToolResult {
        self.ok(
            "get_module_context",
            &context::module_context(
                &self.inner.roblox,
                &self.inner.language_server,
                &request.relative_path,
                self.inner.settings.tools.max_listing_entries,
            )
            .await
            .map_err(fail("get_module_context"))?,
        )
    }

    #[tool(
        description = "Reports only what a ModuleScript hands back: the members of its returned table with their types, plus its exported types. None of the body. Use this instead of reading a module you only intend to call."
    )]
    async fn get_module_api(
        &self,
        Parameters(request): Parameters<ModuleApiRequest>,
    ) -> ToolResult {
        let query = SymbolQuery::new(&self.inner.language_server);
        self.ok(
            "get_module_api",
            &query
                .module_api(
                    &request.relative_path,
                    self.inner.settings.tools.max_listing_entries,
                )
                .await
                .map_err(fail("get_module_api"))?,
        )
    }

    #[tool(
        description = "Looks up the real Roblox API from the type definitions Biskit already caches: the members of a class, the signature and documentation of one member, whether something is deprecated and what replaced it, or the items of an enum. Use this instead of recalling the Roblox API from memory."
    )]
    async fn query_roblox_api(
        &self,
        Parameters(request): Parameters<RobloxApiRequest>,
    ) -> ToolResult {
        let api = self
            .inner
            .roblox
            .api()
            .await
            .map_err(fail("query_roblox_api"))?;
        self.ok(
            "query_roblox_api",
            &api.answer(ApiQuery {
                query: &request.query,
                member_filter: request.member_filter.as_deref(),
                include_inherited: request.include_inherited,
                include_documentation: request.include_documentation,
                max_members: request
                    .max_members
                    .min(self.inner.settings.tools.max_listing_entries),
            })
            .map_err(fail("query_roblox_api"))?,
        )
    }

    #[tool(
        description = "Reports what Biskit is working with: project root, language server state, sourcemap freshness, memory count, and settings that differ from the defaults. Call this when a tool returns nothing and you cannot tell whether that means no matches."
    )]
    async fn get_status(&self, Parameters(NoArguments {}): Parameters<NoArguments>) -> ToolResult {
        let status = status::collect(
            &self.inner.language_server,
            &self.inner.settings,
            &self.inner.memories,
            self.inner.root_source,
        )
        .await
        .map_err(fail("get_status"))?;
        self.ok("get_status", &status)
    }

    #[tool(
        description = "Restarts the Luau language server. Use when symbol results look stale or empty for a file you know has symbols."
    )]
    async fn restart_language_server(
        &self,
        Parameters(NoArguments {}): Parameters<NoArguments>,
    ) -> ToolResult {
        self.inner
            .language_server
            .restart()
            .await
            .map_err(fail("restart_language_server"))?;
        self.text("Language server restarted.")
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Biskit {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::from_build_env())
            .with_instructions(prompts::connection_instructions(
                self.inner.settings.project.memory_only,
            ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ToolSettings;

    fn open(memory_only: bool) -> (tempfile::TempDir, Biskit) {
        open_with(memory_only, ToolSettings::default())
    }

    fn open_with(memory_only: bool, tools: ToolSettings) -> (tempfile::TempDir, Biskit) {
        let dir = tempfile::tempdir().unwrap();
        let project = Project::open(dir.path()).unwrap();
        let settings = Settings {
            project: crate::config::ProjectSettings {
                memory_only,
                ..Default::default()
            },
            tools,
            ..Default::default()
        };
        (dir, Biskit::new(project, settings, "test"))
    }

    fn rendered(result: &CallToolResult) -> String {
        match &result.content[0] {
            ContentBlock::Text(block) => block.text.clone(),
            other => panic!("expected a text block, got {other:?}"),
        }
    }

    #[test]
    fn an_oversized_serialised_result_is_refused_rather_than_cut() {
        let (_dir, biskit) = open_with(
            true,
            ToolSettings {
                max_answer_chars: 16,
                ..Default::default()
            },
        );

        let error = biskit.ok("find_symbol", &vec!["a".repeat(64)]).unwrap_err();
        assert!(error.starts_with("find_symbol failed: the result is 68 characters"));
        assert!(error.contains("limit of 16"));
        assert!(error.contains("hint: ask for less"));
    }

    #[test]
    fn an_oversized_text_result_is_cut_and_says_so() {
        let (_dir, biskit) = open_with(
            true,
            ToolSettings {
                max_answer_chars: 8,
                ..Default::default()
            },
        );

        let answer = rendered(&biskit.text("mémoire trop longue").unwrap());
        // The cap falls inside the multi-byte "é", so the cut lands on the boundary below it.
        assert!(answer.starts_with("mémoire"));
        assert!(answer.contains("[truncated: 8 of 20 characters shown"));
    }

    #[test]
    fn a_zero_limit_disables_the_ceiling() {
        let (_dir, biskit) = open_with(
            true,
            ToolSettings {
                max_answer_chars: 0,
                ..Default::default()
            },
        );

        let answer = rendered(&biskit.text("x".repeat(10_000)).unwrap());
        assert_eq!(answer.len(), 10_000);
        assert!(biskit.ok("list_dir", &vec!["y".repeat(10_000)]).is_ok());
    }

    #[test]
    fn language_server_tools_are_routed_by_default() {
        let (_dir, biskit) = open(false);
        for name in LANGUAGE_SERVER_TOOLS {
            assert!(biskit.tool_router.has_route(name), "missing {name}");
        }
    }

    #[test]
    fn memory_only_drops_language_server_tools() {
        let (_dir, biskit) = open(true);
        for name in LANGUAGE_SERVER_TOOLS {
            assert!(!biskit.tool_router.has_route(name), "still routed: {name}");
        }
        for name in [
            "initial_instructions",
            "list_memories",
            "search_for_pattern",
        ] {
            assert!(biskit.tool_router.has_route(name), "missing {name}");
        }
    }

    /// `get_status` exists to explain an empty answer, and "the language server is disabled" is
    /// exactly such an answer, so it has to survive memory-only mode.
    #[test]
    fn get_status_is_routed_in_both_modes() {
        for memory_only in [false, true] {
            let (_dir, biskit) = open(memory_only);
            assert!(
                biskit.tool_router.has_route("get_status"),
                "missing in memory_only={memory_only}"
            );
        }
    }

    #[test]
    fn a_status_report_names_the_root_and_the_mode() {
        let (dir, biskit) = open(true);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let rendered = rendered(
            &runtime
                .block_on(biskit.get_status(Parameters(NoArguments {})))
                .unwrap(),
        );

        let status: serde_json::Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(status["mode"], "memory-only");
        assert_eq!(status["root_source"], "test");
        assert_eq!(status["language_server"]["state"], "disabled");
        assert_eq!(status["memories"]["count"], 0);
        assert!(
            status.get("sourcemap").is_none(),
            "memory-only mode loads no sourcemap, so it has none to report on"
        );
        assert!(
            status["project_root"]
                .as_str()
                .unwrap()
                .ends_with(dir.path().file_name().unwrap().to_str().unwrap())
        );
    }
}
