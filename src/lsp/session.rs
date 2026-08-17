use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tokio::time::{Duration, Instant, sleep};

use super::acquire::{self, LanguageServerInstall};
use super::client::{LspConnection, ServerEvent};
use super::protocol::{
    Diagnostic, DocumentDiagnosticReport, DocumentSymbolResponse, GotoResponse, Location, Position,
};
use super::symbols::{SymbolNode, build_tree};
use super::uri;
use crate::config::Settings;
use crate::project::Project;

const LUAU_LANGUAGE_ID: &str = "luau";
const SOURCEMAP_POLL_INTERVAL: Duration = Duration::from_millis(1_500);

struct OpenDocument {
    version: i64,
    content: String,
}

pub struct Session {
    connection: LspConnection,
    documents: Mutex<HashMap<PathBuf, OpenDocument>>,
    drain: JoinHandle<()>,
    sourcemap_watch: std::sync::Mutex<Option<JoinHandle<()>>>,
}

impl Session {
    pub async fn start(project: &Project, settings: &Settings) -> Result<Arc<Self>> {
        let lsp = settings.lsp.clone();
        let install = tokio::task::spawn_blocking(move || acquire::ensure_installed(&lsp))
            .await
            .context("language server acquisition panicked")??;

        let (events, receiver) = mpsc::unbounded_channel();
        let configuration = settings.lsp.workspace_configuration();
        let request_timeout = Duration::from_millis(settings.lsp.request_timeout_ms);

        let connection = LspConnection::spawn(
            &install.binary,
            &build_arguments(project, settings, &install)?,
            project.root(),
            request_timeout,
            events,
            configuration.clone(),
        )
        .await?;

        let ready = Arc::new(tokio::sync::Notify::new());
        let drain = tokio::spawn(drain_events(receiver, Arc::clone(&ready)));

        let session = Arc::new(Self {
            connection,
            documents: Mutex::new(HashMap::new()),
            drain,
            sourcemap_watch: std::sync::Mutex::new(None),
        });

        session
            .initialize(project, &configuration, settings)
            .await?;

        // The watcher holds a weak reference so it never keeps a dead session alive.
        let watcher = spawn_sourcemap_watch(project, settings, Arc::downgrade(&session));
        *session
            .sourcemap_watch
            .lock()
            .map_err(|_| anyhow!("sourcemap watch lock was poisoned"))? = watcher;

        Ok(session)
    }

    async fn initialize(
        &self,
        project: &Project,
        configuration: &Value,
        settings: &Settings,
    ) -> Result<()> {
        let root_uri = uri::from_path(project.root())?;
        let parameters = json!({
            "processId": std::process::id(),
            "clientInfo": {
                "name": "biskit-mcp",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "rootUri": root_uri,
            "workspaceFolders": [{
                "uri": root_uri,
                "name": project
                    .root()
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "workspace".to_string()),
            }],
            "initializationOptions": {},
            "capabilities": {
                "workspace": {
                    "configuration": true,
                    "workspaceFolders": true,
                    "didChangeConfiguration": {"dynamicRegistration": true},
                    "didChangeWatchedFiles": {"dynamicRegistration": true},
                    "symbol": {"dynamicRegistration": false},
                },
                "textDocument": {
                    "synchronization": {
                        "dynamicRegistration": false,
                        "didSave": true,
                        "willSave": false,
                    },
                    "documentSymbol": {
                        "dynamicRegistration": false,
                        "hierarchicalDocumentSymbolSupport": true,
                    },
                    "definition": {"dynamicRegistration": false, "linkSupport": true},
                    "references": {"dynamicRegistration": false},
                    "publishDiagnostics": {"relatedInformation": true},
                    "diagnostic": {
                        "dynamicRegistration": false,
                        "relatedDocumentSupport": false,
                    },
                },
            },
        });

        let startup = Duration::from_millis(settings.lsp.startup_timeout_ms);
        let _: Value = self
            .connection
            .request_with_timeout("initialize", parameters, startup)
            .await?;
        self.connection.notify("initialized", json!({})).await?;
        self.connection
            .notify(
                "workspace/didChangeConfiguration",
                json!({"settings": configuration}),
            )
            .await?;
        Ok(())
    }

    pub async fn ensure_open(&self, path: &Path) -> Result<String> {
        let content = tokio::fs::read_to_string(path)
            .await
            .with_context(|| format!("failed to read {}", path.display()))?;
        let target = uri::from_path(path)?;
        let mut documents = self.documents.lock().await;

        match documents.get_mut(path) {
            Some(open) if open.content == content => Ok(open.content.clone()),
            Some(open) => {
                open.version += 1;
                open.content = content.clone();
                self.connection
                    .notify(
                        "textDocument/didChange",
                        json!({
                            "textDocument": {"uri": target, "version": open.version},
                            "contentChanges": [{"text": content}],
                        }),
                    )
                    .await?;
                Ok(content)
            }
            None => {
                self.connection
                    .notify(
                        "textDocument/didOpen",
                        json!({
                            "textDocument": {
                                "uri": target,
                                "languageId": LUAU_LANGUAGE_ID,
                                "version": 1,
                                "text": content,
                            }
                        }),
                    )
                    .await?;
                documents.insert(
                    path.to_path_buf(),
                    OpenDocument {
                        version: 1,
                        content: content.clone(),
                    },
                );
                Ok(content)
            }
        }
    }

    pub async fn document_symbols(&self, path: &Path) -> Result<Vec<SymbolNode>> {
        self.ensure_open(path).await?;
        let response: Option<DocumentSymbolResponse> = self
            .connection
            .request(
                "textDocument/documentSymbol",
                json!({"textDocument": {"uri": uri::from_path(path)?}}),
            )
            .await?;
        Ok(response.map(build_tree).unwrap_or_default())
    }

    pub async fn definition(&self, path: &Path, position: Position) -> Result<Vec<Location>> {
        self.ensure_open(path).await?;
        let response: Option<GotoResponse> = self
            .connection
            .request(
                "textDocument/definition",
                json!({
                    "textDocument": {"uri": uri::from_path(path)?},
                    "position": position,
                }),
            )
            .await?;
        Ok(response
            .map(GotoResponse::into_locations)
            .unwrap_or_default())
    }

    pub async fn references(
        &self,
        path: &Path,
        position: Position,
        include_declaration: bool,
    ) -> Result<Vec<Location>> {
        self.ensure_open(path).await?;
        let response: Option<Vec<Location>> = self
            .connection
            .request(
                "textDocument/references",
                json!({
                    "textDocument": {"uri": uri::from_path(path)?},
                    "position": position,
                    "context": {"includeDeclaration": include_declaration},
                }),
            )
            .await?;
        Ok(response.unwrap_or_default())
    }

    pub async fn diagnostics(&self, path: &Path) -> Result<Vec<Diagnostic>> {
        self.ensure_open(path).await?;
        let report: DocumentDiagnosticReport = self
            .connection
            .request(
                "textDocument/diagnostic",
                json!({"textDocument": {"uri": uri::from_path(path)?}}),
            )
            .await?;
        Ok(report.items)
    }

    pub async fn notify_sourcemap_changed(&self, sourcemap: &Path) -> Result<()> {
        self.connection
            .notify(
                "workspace/didChangeWatchedFiles",
                json!({"changes": [{"uri": uri::from_path(sourcemap)?, "type": 2}]}),
            )
            .await
    }

    pub async fn shutdown(&self) {
        self.drain.abort();
        if let Ok(mut guard) = self.sourcemap_watch.lock()
            && let Some(watch) = guard.take()
        {
            watch.abort();
        }
        self.connection.shutdown().await;
    }
}

fn build_arguments(
    project: &Project,
    settings: &Settings,
    install: &LanguageServerInstall,
) -> Result<Vec<String>> {
    let mut arguments = vec!["lsp".to_string()];

    for (alias, path) in &install.definition_files {
        arguments.push(format!("--definitions:{alias}={}", path.display()));
    }
    for entry in &settings.lsp.definition_files {
        let (alias, relative) = entry.split_once('=').ok_or_else(|| {
            anyhow!("lsp.definition_files entries must look like @alias=path: {entry}")
        })?;
        let resolved = project.resolve(relative)?;
        arguments.push(format!("--definitions:{alias}={}", resolved.display()));
    }

    for path in &install.documentation_files {
        arguments.push(format!("--docs={}", path.display()));
    }
    for relative in &settings.lsp.documentation_files {
        arguments.push(format!("--docs={}", project.resolve(relative)?.display()));
    }

    if let Some(base_luaurc) = &settings.lsp.base_luaurc {
        arguments.push(format!(
            "--base-luaurc={}",
            project.resolve(base_luaurc)?.display()
        ));
    }

    arguments.extend(settings.lsp.extra_args.iter().cloned());
    Ok(arguments)
}

async fn drain_events(
    mut receiver: mpsc::UnboundedReceiver<ServerEvent>,
    ready: Arc<tokio::sync::Notify>,
) {
    while let Some(event) = receiver.recv().await {
        match event {
            ServerEvent::LogMessage(message) => {
                if message.contains("workspace ready") || message.contains("initialized") {
                    ready.notify_waiters();
                }
                tracing::debug!(target: "biskit::lsp", "{message}");
            }
            ServerEvent::Exited => {
                tracing::warn!(target: "biskit::lsp", "language server exited");
                return;
            }
        }
    }
}

/// luau-lsp only reloads the sourcemap when told; poll its mtime and forward changes.
fn spawn_sourcemap_watch(
    project: &Project,
    settings: &Settings,
    session: std::sync::Weak<Session>,
) -> Option<JoinHandle<()>> {
    if !settings.lsp.watch_sourcemap {
        return None;
    }
    let relative = settings.lsp.sourcemap.as_ref()?;
    let sourcemap = project.resolve(relative).ok()?;

    Some(tokio::spawn(async move {
        let mut last_seen = modified_at(&sourcemap).await;
        loop {
            sleep(SOURCEMAP_POLL_INTERVAL).await;
            let Some(session) = session.upgrade() else {
                return;
            };
            let current = modified_at(&sourcemap).await;
            if current == last_seen {
                continue;
            }
            last_seen = current;
            if let Err(error) = session.notify_sourcemap_changed(&sourcemap).await {
                tracing::warn!(target: "biskit::lsp", "sourcemap notification failed: {error}");
                return;
            }
            tracing::debug!(target: "biskit::lsp", "sourcemap change forwarded");
        }
    }))
}

async fn modified_at(path: &Path) -> Option<(std::time::SystemTime, u64)> {
    let metadata = tokio::fs::metadata(path).await.ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}

/// Wraps the session so it can be restarted without tearing down the MCP server.
pub struct LanguageServerHandle {
    project: Project,
    settings: Settings,
    session: Mutex<Option<Arc<Session>>>,
}

impl LanguageServerHandle {
    pub fn new(project: Project, settings: Settings) -> Self {
        Self {
            project,
            settings,
            session: Mutex::new(None),
        }
    }

    pub async fn session(&self) -> Result<Arc<Session>> {
        if self.settings.project.memory_only {
            bail!(
                "Biskit is in memory-only mode, so the Luau language server is not available; \
                 unset project.memory_only in .biskit/settings.yml to enable it"
            );
        }

        let mut guard = self.session.lock().await;
        if let Some(existing) = guard.as_ref() {
            return Ok(Arc::clone(existing));
        }

        let started = Instant::now();
        let session = Session::start(&self.project, &self.settings).await?;
        tracing::info!(
            target: "biskit::lsp",
            "language server ready in {}ms",
            started.elapsed().as_millis()
        );
        *guard = Some(Arc::clone(&session));
        Ok(session)
    }

    pub async fn restart(&self) -> Result<()> {
        {
            let mut guard = self.session.lock().await;
            if let Some(existing) = guard.take()
                && let Some(owned) = Arc::into_inner(existing)
            {
                owned.shutdown().await;
            }
        }
        self.session().await.map(|_| ())
    }

    pub async fn stop(&self) {
        let mut guard = self.session.lock().await;
        if let Some(existing) = guard.take()
            && let Some(owned) = Arc::into_inner(existing)
        {
            owned.shutdown().await;
        }
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    pub async fn resolve_luau_files(&self) -> Result<Vec<PathBuf>> {
        let project = self.project.clone();
        let respect_gitignore = self.settings.project.respect_gitignore;
        let ignored = self.settings.project.ignored_paths.clone();

        tokio::task::spawn_blocking(move || {
            let mut builder = ignore::WalkBuilder::new(project.root());
            builder
                .hidden(false)
                .git_ignore(respect_gitignore)
                .git_exclude(respect_gitignore)
                .git_global(false)
                .require_git(false)
                .follow_links(false);
            for pattern in &ignored {
                builder.add_ignore(pattern);
            }

            let mut found = Vec::new();
            for entry in builder.build().filter_map(Result::ok) {
                if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                    continue;
                }
                let path = entry.into_path();
                if path
                    .extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|extension| extension == "luau" || extension == "lua")
                {
                    found.push(path);
                }
            }
            found.sort();
            Ok(found)
        })
        .await
        .map_err(|error| anyhow!("project scan panicked: {error}"))?
    }
}

pub fn ensure_luau_file(path: &Path) -> Result<()> {
    let extension = path.extension().and_then(|value| value.to_str());
    if matches!(extension, Some("luau" | "lua")) {
        return Ok(());
    }
    bail!(
        "{} is not a Luau source file; Biskit's symbol tools only work on .luau and .lua",
        path.display()
    )
}
