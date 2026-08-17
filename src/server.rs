use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerInfo};
use rmcp::{ErrorData as McpError, ServerHandler, schemars, tool, tool_handler, tool_router};
use serde::Deserialize;
use serde::Serialize;

use crate::config::Settings;
use crate::files::{FileTools, PatternSearchRequest};
use crate::lsp::queries::{FindSymbolRequest, SymbolQuery, severity_from_input};
use crate::lsp::session::LanguageServerHandle;
use crate::memory::MemoryStore;
use crate::project::Project;
use crate::prompts;

#[derive(Clone)]
pub struct Biskit {
    inner: Arc<Inner>,
    tool_router: ToolRouter<Biskit>,
}

struct Inner {
    settings: Settings,
    memories: MemoryStore,
    files: FileTools,
    language_server: LanguageServerHandle,
}

fn ok<T: Serialize>(value: &T) -> Result<CallToolResult, McpError> {
    let rendered = serde_json::to_string_pretty(value)
        .map_err(|error| McpError::internal_error(error.to_string(), None))?;
    Ok(CallToolResult::success(vec![ContentBlock::text(rendered)]))
}

fn text(value: impl Into<String>) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::success(vec![ContentBlock::text(
        value.into(),
    )]))
}

fn fail(error: anyhow::Error) -> McpError {
    McpError::invalid_params(format!("{error:#}"), None)
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
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct EditMemoryRequest {
    pub memory_name: String,
    /// Regular expression matched against the memory body.
    pub pattern: String,
    /// Replacement text. Capture groups are available as `$1`, `$2`, and so on.
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
    /// How many levels of nested symbols to include. 0 lists top-level symbols only.
    #[serde(default)]
    pub depth: u32,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FindSymbolRequestInput {
    /// Name path such as "update", "PlayerService/update", or "/PlayerService".
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
    /// Name path of the symbol.
    pub name_path: String,
    /// File containing the symbol, relative to the project root.
    pub relative_path: String,
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
pub struct NoArguments {}

/// Tools backed by the language server. Memory-only mode drops these routes entirely.
const LANGUAGE_SERVER_TOOLS: [&str; 7] = [
    "get_symbols_overview",
    "find_symbol",
    "find_declaration",
    "find_referencing_symbols",
    "get_file_diagnostics",
    "get_symbol_diagnostics",
    "restart_language_server",
];

fn project_root() -> String {
    ".".to_string()
}

fn default_max_matches() -> usize {
    50
}

#[tool_router]
impl Biskit {
    pub fn new(project: Project, settings: Settings) -> Self {
        let memories = MemoryStore::new(project.clone());
        let files = FileTools::new(project.clone(), settings.clone());
        let language_server = LanguageServerHandle::new(project, settings.clone());

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
            }),
            tool_router,
        }
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
    ) -> Result<CallToolResult, McpError> {
        let memories = self.inner.memories.list().map_err(fail)?;
        text(prompts::initial_instructions(
            &memories,
            self.inner.settings.project.memory_only,
        ))
    }

    #[tool(description = "Lists the names of every memory stored for this project.")]
    async fn list_memories(
        &self,
        Parameters(NoArguments {}): Parameters<NoArguments>,
    ) -> Result<CallToolResult, McpError> {
        ok(&self.inner.memories.list().map_err(fail)?)
    }

    #[tool(description = "Reads the full markdown content of one memory.")]
    async fn read_memory(
        &self,
        Parameters(request): Parameters<MemoryNameRequest>,
    ) -> Result<CallToolResult, McpError> {
        text(
            self.inner
                .memories
                .read(&request.memory_name)
                .map_err(fail)?,
        )
    }

    #[tool(
        description = "Writes a memory recording durable knowledge about this project, in markdown. Use a meaningful, nestable name."
    )]
    async fn create_memory(
        &self,
        Parameters(request): Parameters<CreateMemoryRequest>,
    ) -> Result<CallToolResult, McpError> {
        let name = self
            .inner
            .memories
            .create(&request.memory_name, &request.content)
            .map_err(fail)?;
        text(format!("Wrote memory {name}."))
    }

    #[tool(description = "Deletes a memory.")]
    async fn delete_memory(
        &self,
        Parameters(request): Parameters<MemoryNameRequest>,
    ) -> Result<CallToolResult, McpError> {
        let name = self
            .inner
            .memories
            .delete(&request.memory_name)
            .map_err(fail)?;
        text(format!("Deleted memory {name}."))
    }

    #[tool(
        description = "Replaces content matching a regular expression inside an existing memory. Prefer this over rewriting a memory wholesale."
    )]
    async fn edit_memory(
        &self,
        Parameters(request): Parameters<EditMemoryRequest>,
    ) -> Result<CallToolResult, McpError> {
        let outcome = self
            .inner
            .memories
            .edit(
                &request.memory_name,
                &request.pattern,
                &request.replacement,
                request.allow_multiple_occurrences,
            )
            .map_err(fail)?;
        text(format!(
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
    ) -> Result<CallToolResult, McpError> {
        let outcome = self
            .inner
            .memories
            .rename(&request.old_name, &request.new_name)
            .map_err(fail)?;
        ok(&serde_json::json!({
            "from": outcome.from,
            "to": outcome.to,
            "updated_references": outcome.updated_references,
        }))
    }

    #[tool(description = "Lists files and directories under a project-relative path.")]
    async fn list_dir(
        &self,
        Parameters(request): Parameters<ListDirRequest>,
    ) -> Result<CallToolResult, McpError> {
        ok(&self
            .inner
            .files
            .list_dir(&request.relative_path, request.recursive)
            .map_err(fail)?)
    }

    #[tool(description = "Finds files whose name matches a glob mask.")]
    async fn find_file(
        &self,
        Parameters(request): Parameters<FindFileRequest>,
    ) -> Result<CallToolResult, McpError> {
        ok(&self
            .inner
            .files
            .find_file(&request.file_mask, &request.relative_path)
            .map_err(fail)?)
    }

    #[tool(
        description = "Searches file contents with a regular expression. Use this for text that is not a symbol; use find_symbol for definitions."
    )]
    async fn search_for_pattern(
        &self,
        Parameters(request): Parameters<SearchForPatternRequest>,
    ) -> Result<CallToolResult, McpError> {
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
            .map_err(fail)?;
        ok(&result)
    }

    #[tool(
        description = "Lists the symbols defined in a Luau file. Use this before reading a file to decide what is worth reading."
    )]
    async fn get_symbols_overview(
        &self,
        Parameters(request): Parameters<SymbolsOverviewRequest>,
    ) -> Result<CallToolResult, McpError> {
        let query = SymbolQuery::new(&self.inner.language_server);
        ok(&query
            .symbols_overview(&request.relative_path, request.depth)
            .await
            .map_err(fail)?)
    }

    #[tool(
        description = "Finds symbols by name path across the project or within one file or directory."
    )]
    async fn find_symbol(
        &self,
        Parameters(request): Parameters<FindSymbolRequestInput>,
    ) -> Result<CallToolResult, McpError> {
        let query = SymbolQuery::new(&self.inner.language_server);
        ok(&query
            .find_symbol(FindSymbolRequest {
                name_path: request.name_path,
                relative_path: request.relative_path,
                depth: request.depth,
                include_body: request.include_body,
                include_kinds: request.include_kinds,
                exclude_kinds: request.exclude_kinds,
                substring_matching: request.substring_matching,
                max_matches: request
                    .max_matches
                    .min(self.inner.settings.tools.max_listing_entries),
            })
            .await
            .map_err(fail)?)
    }

    #[tool(description = "Finds where a symbol is declared.")]
    async fn find_declaration(
        &self,
        Parameters(request): Parameters<SymbolLocationRequest>,
    ) -> Result<CallToolResult, McpError> {
        let query = SymbolQuery::new(&self.inner.language_server);
        ok(&query
            .find_declaration(&request.name_path, &request.relative_path)
            .await
            .map_err(fail)?)
    }

    #[tool(description = "Finds every symbol that references the given symbol.")]
    async fn find_referencing_symbols(
        &self,
        Parameters(request): Parameters<SymbolLocationRequest>,
    ) -> Result<CallToolResult, McpError> {
        let query = SymbolQuery::new(&self.inner.language_server);
        ok(&query
            .find_referencing_symbols(
                &request.name_path,
                &request.relative_path,
                self.inner.settings.tools.max_listing_entries,
            )
            .await
            .map_err(fail)?)
    }

    #[tool(
        description = "Gets diagnostics for a file, optionally limited to a line range, grouped by severity and containing symbol."
    )]
    async fn get_file_diagnostics(
        &self,
        Parameters(request): Parameters<FileDiagnosticsRequest>,
    ) -> Result<CallToolResult, McpError> {
        let severity = severity_from_input(request.min_severity).map_err(fail)?;
        let query = SymbolQuery::new(&self.inner.language_server);
        ok(&query
            .file_diagnostics(
                &request.relative_path,
                request.start_line,
                request.end_line,
                severity,
            )
            .await
            .map_err(fail)?)
    }

    #[tool(
        description = "Gets diagnostics for one symbol and, optionally, for every file that references it. Use after editing a symbol."
    )]
    async fn get_symbol_diagnostics(
        &self,
        Parameters(request): Parameters<SymbolDiagnosticsRequest>,
    ) -> Result<CallToolResult, McpError> {
        let severity = severity_from_input(request.min_severity).map_err(fail)?;
        let query = SymbolQuery::new(&self.inner.language_server);
        ok(&query
            .symbol_diagnostics(
                &request.name_path,
                &request.relative_path,
                request.check_symbol_references,
                severity,
            )
            .await
            .map_err(fail)?)
    }

    #[tool(
        description = "Restarts the Luau language server. Use when symbol results look stale or empty for a file you know has symbols."
    )]
    async fn restart_language_server(
        &self,
        Parameters(NoArguments {}): Parameters<NoArguments>,
    ) -> Result<CallToolResult, McpError> {
        self.inner.language_server.restart().await.map_err(fail)?;
        text("Language server restarted.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(memory_only: bool) -> (tempfile::TempDir, Biskit) {
        let dir = tempfile::tempdir().unwrap();
        let project = Project::open(dir.path()).unwrap();
        let settings = Settings {
            project: crate::config::ProjectSettings {
                memory_only,
                ..Default::default()
            },
            ..Default::default()
        };
        (dir, Biskit::new(project, settings))
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
        for name in ["initial_instructions", "list_memories", "search_for_pattern"] {
            assert!(biskit.tool_router.has_route(name), "missing {name}");
        }
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
