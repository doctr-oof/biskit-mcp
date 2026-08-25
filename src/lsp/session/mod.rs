mod handle;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tokio::time::{Duration, sleep};

use super::acquire::{self, LanguageServerInstall};
use super::client::{LspConnection, ServerEvent};
use super::protocol::{
    Diagnostic, DocumentDiagnosticReport, DocumentSymbolResponse, GotoResponse, Hover, InlayHint,
    Location, Position, Range, SignatureHelp,
};
use super::symbols::{SymbolNode, build_tree};
use super::uri;
use crate::config::Settings;
use crate::project::Project;

pub use handle::{LanguageServerHandle, ServerState, ensure_luau_file};

const LUAU_LANGUAGE_ID: &str = "luau";
const SOURCEMAP_POLL_INTERVAL: Duration = Duration::from_millis(1_500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    modified: std::time::SystemTime,
    len: u64,
}

struct OpenDocument {
    version: i64,
    content: Arc<str>,
    uri: Arc<str>,
    stamp: Option<FileStamp>,
    touched: u64,
}

/// The documents the server has been told about, ordered by how recently each was reached for.
#[derive(Default)]
struct OpenDocuments {
    clock: u64,
    open: HashMap<PathBuf, OpenDocument>,
}

/// A document the language server has been told about, and the text it was told.
#[derive(Debug, Clone)]
pub struct OpenFile {
    pub content: Arc<str>,
    pub uri: Arc<str>,
}

enum Sync {
    Opened,
    Changed(i64),
}

pub struct Session {
    connection: LspConnection,
    documents: Mutex<OpenDocuments>,
    max_open_documents: usize,
    drain: JoinHandle<()>,
    sourcemap_watch: std::sync::Mutex<Option<JoinHandle<()>>>,
    alive: Arc<AtomicBool>,
}

impl Session {
    pub async fn start(project: &Project, settings: &Settings) -> Result<Arc<Self>> {
        let lsp = settings.lsp.clone();
        let install = tokio::task::spawn_blocking(move || acquire::ensure_installed(&lsp))
            .await
            .context("language server acquisition panicked")??;

        let (events, receiver) = mpsc::unbounded_channel();
        let configuration = settings.lsp.workspace_configuration(&settings.project);
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
        let alive = Arc::new(AtomicBool::new(true));
        let drain = tokio::spawn(drain_events(
            receiver,
            Arc::clone(&ready),
            Arc::clone(&alive),
        ));

        let session = Arc::new(Self {
            connection,
            documents: Mutex::new(OpenDocuments::default()),
            max_open_documents: settings.lsp.max_open_documents,
            drain,
            sourcemap_watch: std::sync::Mutex::new(None),
            alive,
        });

        session
            .initialize(project, &configuration, settings)
            .await?;

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
                    "workspaceEdit": {"documentChanges": true, "failureHandling": "abort"},
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
                    "typeDefinition": {"dynamicRegistration": false, "linkSupport": true},
                    "references": {"dynamicRegistration": false},
                    "hover": {
                        "dynamicRegistration": false,
                        "contentFormat": ["markdown", "plaintext"],
                    },
                    "inlayHint": {"dynamicRegistration": false},
                    "signatureHelp": {
                        "dynamicRegistration": false,
                        "contextSupport": false,
                        "signatureInformation": {
                            "documentationFormat": ["markdown", "plaintext"],
                            "parameterInformation": {"labelOffsetSupport": true},
                            "activeParameterSupport": true,
                        },
                    },
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

    /// Makes sure the server holds the current text of `path`, and hands back that text.
    pub async fn ensure_open(&self, path: &Path) -> Result<OpenFile> {
        let stamp = file_stamp(path).await;

        if stamp.is_some() {
            let mut documents = self.documents.lock().await;
            let touched = documents.tick();
            if let Some(open) = documents
                .open
                .get_mut(path)
                .filter(|open| open.stamp == stamp)
            {
                open.touched = touched;
                return Ok(open.as_file());
            }
        }

        let content: Arc<str> = tokio::fs::read_to_string(path)
            .await
            .with_context(|| format!("failed to read {}", path.display()))?
            .into();

        let (file, sync) = {
            let mut documents = self.documents.lock().await;
            let touched = documents.tick();
            let synced = match documents.open.get_mut(path) {
                Some(open) if open.content == content => {
                    open.stamp = stamp;
                    open.touched = touched;
                    return Ok(open.as_file());
                }
                Some(open) => {
                    open.version += 1;
                    open.content = Arc::clone(&content);
                    open.stamp = stamp;
                    open.touched = touched;
                    (open.as_file(), Sync::Changed(open.version))
                }
                None => {
                    let document = OpenDocument {
                        version: 1,
                        content: Arc::clone(&content),
                        uri: Arc::from(uri::from_path(path)?),
                        stamp,
                        touched,
                    };
                    let file = document.as_file();
                    documents.open.insert(path.to_path_buf(), document);
                    (file, Sync::Opened)
                }
            };

            if matches!(synced.1, Sync::Opened) {
                self.close_least_recent(&mut documents, path).await;
            }
            synced
        };

        let sent = match sync {
            Sync::Changed(version) => {
                self.connection
                    .notify(
                        "textDocument/didChange",
                        json!({
                            "textDocument": {"uri": file.uri, "version": version},
                            "contentChanges": [{"text": file.content}],
                        }),
                    )
                    .await
            }
            Sync::Opened => {
                self.connection
                    .notify(
                        "textDocument/didOpen",
                        json!({
                            "textDocument": {
                                "uri": file.uri,
                                "languageId": LUAU_LANGUAGE_ID,
                                "version": 1,
                                "text": file.content,
                            }
                        }),
                    )
                    .await
            }
        };

        if let Err(error) = sent {
            self.documents.lock().await.open.remove(path);
            return Err(error);
        }
        Ok(file)
    }

    /// Retracts the documents past the ceiling, newest kept, `keep` never chosen.
    ///
    /// The notification goes out under the guard so a later reopen of the same path cannot
    /// have its `didOpen` overtaken by this `didClose`.
    async fn close_least_recent(&self, documents: &mut OpenDocuments, keep: &Path) {
        if self.max_open_documents == 0 || documents.open.len() <= self.max_open_documents {
            return;
        }

        let excess = documents.open.len() - self.max_open_documents;
        let mut ranked: Vec<(u64, PathBuf)> = documents
            .open
            .iter()
            .filter(|(path, _)| path.as_path() != keep)
            .map(|(path, open)| (open.touched, path.clone()))
            .collect();
        ranked.sort_unstable();

        for (_, path) in ranked.into_iter().take(excess) {
            let Some(open) = documents.open.remove(&path) else {
                continue;
            };
            if let Err(error) = self
                .connection
                .notify(
                    "textDocument/didClose",
                    json!({"textDocument": {"uri": open.uri}}),
                )
                .await
            {
                tracing::warn!(
                    target: "biskit::lsp",
                    "failed to close {}: {error}",
                    path.display()
                );
            }
        }
    }

    /// The symbol tree of `path`, alongside the text it was built from.
    pub async fn document_symbols(&self, path: &Path) -> Result<(Vec<SymbolNode>, Arc<str>)> {
        let file = self.ensure_open(path).await?;
        let response: Option<DocumentSymbolResponse> = self
            .connection
            .request(
                "textDocument/documentSymbol",
                json!({"textDocument": {"uri": file.uri}}),
            )
            .await?;
        Ok((response.map(build_tree).unwrap_or_default(), file.content))
    }

    pub async fn definition(&self, path: &Path, position: Position) -> Result<Vec<Location>> {
        let file = self.ensure_open(path).await?;
        let response: Option<GotoResponse> = self
            .connection
            .request(
                "textDocument/definition",
                json!({
                    "textDocument": {"uri": file.uri},
                    "position": position,
                }),
            )
            .await?;
        Ok(response
            .map(GotoResponse::into_locations)
            .unwrap_or_default())
    }

    pub async fn type_definition(&self, path: &Path, position: Position) -> Result<Vec<Location>> {
        let file = self.ensure_open(path).await?;
        let response: Option<GotoResponse> = self
            .connection
            .request(
                "textDocument/typeDefinition",
                json!({
                    "textDocument": {"uri": file.uri},
                    "position": position,
                }),
            )
            .await?;
        Ok(response
            .map(GotoResponse::into_locations)
            .unwrap_or_default())
    }

    pub async fn hover(&self, path: &Path, position: Position) -> Result<Option<Hover>> {
        let file = self.ensure_open(path).await?;
        self.connection
            .request(
                "textDocument/hover",
                json!({
                    "textDocument": {"uri": file.uri},
                    "position": position,
                }),
            )
            .await
    }

    pub async fn signature_help(
        &self,
        path: &Path,
        position: Position,
    ) -> Result<Option<SignatureHelp>> {
        let file = self.ensure_open(path).await?;
        self.connection
            .request(
                "textDocument/signatureHelp",
                json!({
                    "textDocument": {"uri": file.uri},
                    "position": position,
                }),
            )
            .await
    }

    pub async fn inlay_hints(&self, path: &Path, range: Range) -> Result<Vec<InlayHint>> {
        let file = self.ensure_open(path).await?;
        let response: Option<Vec<InlayHint>> = self
            .connection
            .request(
                "textDocument/inlayHint",
                json!({
                    "textDocument": {"uri": file.uri},
                    "range": range,
                }),
            )
            .await?;
        Ok(response.unwrap_or_default())
    }

    pub async fn references(
        &self,
        path: &Path,
        position: Position,
        include_declaration: bool,
    ) -> Result<Vec<Location>> {
        let file = self.ensure_open(path).await?;
        let response: Option<Vec<Location>> = self
            .connection
            .request(
                "textDocument/references",
                json!({
                    "textDocument": {"uri": file.uri},
                    "position": position,
                    "context": {"includeDeclaration": include_declaration},
                }),
            )
            .await?;
        Ok(response.unwrap_or_default())
    }

    pub async fn diagnostics(&self, path: &Path) -> Result<Vec<Diagnostic>> {
        let file = self.ensure_open(path).await?;
        let report: DocumentDiagnosticReport = self
            .connection
            .request(
                "textDocument/diagnostic",
                json!({"textDocument": {"uri": file.uri}}),
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

    /// False once the child process has gone, so a caller can replace the session rather than wait on it.
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    pub async fn shutdown(&self) {
        self.alive.store(false, Ordering::Release);
        self.drain.abort();
        if let Ok(mut guard) = self.sourcemap_watch.lock()
            && let Some(watch) = guard.take()
        {
            watch.abort();
        }
        self.connection.shutdown().await;
    }
}

impl OpenDocuments {
    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }
}

impl OpenDocument {
    fn as_file(&self) -> OpenFile {
        OpenFile {
            content: Arc::clone(&self.content),
            uri: Arc::clone(&self.uri),
        }
    }
}

async fn file_stamp(path: &Path) -> Option<FileStamp> {
    let metadata = tokio::fs::metadata(path).await.ok()?;
    Some(FileStamp {
        modified: metadata.modified().ok()?,
        len: metadata.len(),
    })
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
    alive: Arc<AtomicBool>,
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
                alive.store(false, Ordering::Release);
                tracing::warn!(target: "biskit::lsp", "language server exited");
                return;
            }
        }
    }
    alive.store(false, Ordering::Release);
}

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
    if let Err(error) = uri::from_path(&sourcemap) {
        tracing::warn!(target: "biskit::lsp", "sourcemap cannot be watched: {error}");
        return None;
    }

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
            if let Err(error) = session.notify_sourcemap_changed(&sourcemap).await {
                tracing::warn!(target: "biskit::lsp", "sourcemap notification failed: {error}");
                continue;
            }
            last_seen = current;
            tracing::debug!(target: "biskit::lsp", "sourcemap change forwarded");
        }
    }))
}

async fn modified_at(path: &Path) -> Option<(std::time::SystemTime, u64)> {
    let metadata = tokio::fs::metadata(path).await.ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}
