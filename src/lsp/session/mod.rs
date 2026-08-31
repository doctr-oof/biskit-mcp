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

/// The stamps every project source file carried when the server last saw it, so a later sweep can
/// tell which files have moved underneath it.
#[derive(Default)]
struct KnownFiles {
    seeded: bool,
    stamps: HashMap<PathBuf, FileStamp>,
}

/// The files a sweep found to have moved, split by the event the server expects for each.
#[derive(Debug, Default, PartialEq, Eq)]
struct SyncPlan {
    created: Vec<PathBuf>,
    changed: Vec<PathBuf>,
    removed: Vec<PathBuf>,
}

impl SyncPlan {
    fn len(&self) -> usize {
        self.created.len() + self.changed.len() + self.removed.len()
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub struct Session {
    connection: LspConnection,
    documents: Mutex<OpenDocuments>,
    known: Mutex<KnownFiles>,
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
            known: Mutex::new(KnownFiles::default()),
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
        self.open_document(path, false).await
    }

    /// Re-reads `path` even when its size and modification time are the ones already held, for the
    /// edit that lands inside a single tick of the filesystem clock without changing the length.
    pub async fn reload(&self, path: &Path) -> Result<OpenFile> {
        self.open_document(path, true).await
    }

    async fn open_document(&self, path: &Path, force: bool) -> Result<OpenFile> {
        let stamp = file_stamp(path).await;

        if !force && stamp.is_some() {
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

    /// Records where `files` stand right now without telling the server anything, so the first
    /// sweep reports the edits made from here on rather than the whole project.
    pub async fn seed_disk_stamps(&self, files: &[PathBuf]) {
        let current = current_stamps(files).await;
        let mut known = self.known.lock().await;
        known.stamps = current.into_iter().collect();
        known.seeded = true;
    }

    /// Forwards every file that has moved since the last sweep, and answers with how many were
    /// reported.
    ///
    /// The server reads an open document from the text it was handed, so those go out as an edit;
    /// the rest go out as watched-file events, which is what makes the server drop its cached
    /// analysis of a file and of everything that requires it.
    pub async fn sync_disk_changes(&self, files: &[PathBuf]) -> Result<usize> {
        let current = current_stamps(files).await;

        let mut plan = {
            let mut known = self.known.lock().await;
            let plan = match known.seeded {
                true => plan_sync(&known.stamps, &current),
                false => SyncPlan::default(),
            };
            known.stamps = current.into_iter().collect();
            known.seeded = true;
            plan
        };

        if plan.is_empty() {
            return Ok(0);
        }

        for path in std::mem::take(&mut plan.removed) {
            match file_stamp(&path).await {
                Some(_) => plan.changed.push(path),
                None => plan.removed.push(path),
            }
        }

        self.report_disk_changes(plan).await
    }

    /// Sends the edits and the watched-file events a sweep asked for.
    async fn report_disk_changes(&self, plan: SyncPlan) -> Result<usize> {
        let reported = plan.len();
        let mut events = Vec::with_capacity(reported);

        for (paths, kind) in [(&plan.created, 1), (&plan.changed, 2), (&plan.removed, 3)] {
            for path in paths {
                let open = self.documents.lock().await.open.contains_key(path);
                if open && kind != 3 {
                    if let Err(error) = self.reload(path).await {
                        tracing::warn!(
                            target: "biskit::lsp",
                            "failed to resend {}: {error}",
                            path.display()
                        );
                    }
                    continue;
                }
                if open {
                    self.close(path).await;
                }
                let Ok(uri) = uri::from_path(path) else {
                    continue;
                };
                events.push(json!({"uri": uri, "type": kind}));
            }
        }

        if !events.is_empty() {
            self.connection
                .notify(
                    "workspace/didChangeWatchedFiles",
                    json!({"changes": events}),
                )
                .await?;
        }
        Ok(reported)
    }

    /// Drops a document the server is holding, for a file that is no longer on disk.
    async fn close(&self, path: &Path) {
        let Some(open) = self.documents.lock().await.open.remove(path) else {
            return;
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

/// Stamps a whole sweep's worth of files on one blocking thread, rather than handing every
/// metadata call to the pool on its own.
async fn current_stamps(files: &[PathBuf]) -> Vec<(PathBuf, FileStamp)> {
    let files = files.to_vec();
    tokio::task::spawn_blocking(move || {
        files
            .into_iter()
            .filter_map(|path| {
                let stamp = std::fs::metadata(&path).ok().and_then(stamp_of)?;
                Some((path, stamp))
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

fn plan_sync(known: &HashMap<PathBuf, FileStamp>, current: &[(PathBuf, FileStamp)]) -> SyncPlan {
    let mut plan = SyncPlan::default();

    for (path, stamp) in current {
        match known.get(path) {
            Some(held) if held == stamp => {}
            Some(_) => plan.changed.push(path.clone()),
            None => plan.created.push(path.clone()),
        }
    }

    let seen: std::collections::HashSet<&PathBuf> = current.iter().map(|(path, _)| path).collect();
    for path in known.keys() {
        if !seen.contains(path) {
            plan.removed.push(path.clone());
        }
    }

    plan.created.sort();
    plan.changed.sort();
    plan.removed.sort();
    plan
}

async fn file_stamp(path: &Path) -> Option<FileStamp> {
    stamp_of(tokio::fs::metadata(path).await.ok()?)
}

fn stamp_of(metadata: std::fs::Metadata) -> Option<FileStamp> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp(seconds: u64, len: u64) -> FileStamp {
        FileStamp {
            modified: std::time::UNIX_EPOCH + Duration::from_secs(seconds),
            len,
        }
    }

    fn known(entries: &[(&str, FileStamp)]) -> HashMap<PathBuf, FileStamp> {
        entries
            .iter()
            .map(|(path, stamp)| (PathBuf::from(path), *stamp))
            .collect()
    }

    fn current(entries: &[(&str, FileStamp)]) -> Vec<(PathBuf, FileStamp)> {
        entries
            .iter()
            .map(|(path, stamp)| (PathBuf::from(path), *stamp))
            .collect()
    }

    fn paths(entries: &[&str]) -> Vec<PathBuf> {
        entries.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn a_file_that_has_not_moved_is_not_reported() {
        let plan = plan_sync(
            &known(&[("A.luau", stamp(1, 10))]),
            &current(&[("A.luau", stamp(1, 10))]),
        );
        assert!(plan.is_empty());
    }

    #[test]
    fn every_kind_of_move_lands_in_its_own_bucket() {
        let plan = plan_sync(
            &known(&[
                ("A.luau", stamp(1, 10)),
                ("B.luau", stamp(1, 10)),
                ("C.luau", stamp(1, 10)),
            ]),
            &current(&[
                ("A.luau", stamp(1, 10)),
                ("B.luau", stamp(2, 10)),
                ("D.luau", stamp(1, 10)),
            ]),
        );

        assert_eq!(plan.changed, paths(&["B.luau"]));
        assert_eq!(plan.created, paths(&["D.luau"]));
        assert_eq!(plan.removed, paths(&["C.luau"]));
        assert_eq!(plan.len(), 3);
    }

    #[test]
    fn an_edit_that_keeps_the_length_is_still_a_move() {
        let plan = plan_sync(
            &known(&[("A.luau", stamp(1, 10))]),
            &current(&[("A.luau", stamp(2, 10))]),
        );
        assert_eq!(plan.changed, paths(&["A.luau"]));
    }
}
