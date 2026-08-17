use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
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
use crate::bail_hint;
use crate::config::Settings;
use crate::project::Project;

const LUAU_LANGUAGE_ID: &str = "luau";
const SOURCEMAP_POLL_INTERVAL: Duration = Duration::from_millis(1_500);

/// Size and modification time of a file as of the last time it was read.
///
/// Comparing this against the file on disk decides whether the body has to be read at all. Within
/// one agent session the same files are visited over and over and almost never change between
/// visits, so the read that used to happen on every call is the read worth avoiding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    modified: std::time::SystemTime,
    len: u64,
}

struct OpenDocument {
    version: i64,
    content: Arc<str>,
    /// Encoded once per document rather than per request against it.
    uri: Arc<str>,
    /// Absent when the platform did not report a modification time, which forces the full read.
    stamp: Option<FileStamp>,
}

/// A document the language server has been told about, and the text it was told.
#[derive(Debug, Clone)]
pub struct OpenFile {
    pub content: Arc<str>,
    pub uri: Arc<str>,
}

/// What has to be sent to the server after the document map has been updated.
enum Sync {
    Opened,
    Changed(i64),
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

    /// Makes sure the server holds the current text of `path`, and hands back that text.
    ///
    /// The body is only read from disk when the file's size or modification time differs from the
    /// stamp taken the last time it was read. The notification is written after the document map
    /// lock is released, so nothing waits on the stdin mutex while holding it.
    pub async fn ensure_open(&self, path: &Path) -> Result<OpenFile> {
        let stamp = file_stamp(path).await;

        if stamp.is_some() {
            let documents = self.documents.lock().await;
            if let Some(open) = documents.get(path).filter(|open| open.stamp == stamp) {
                return Ok(open.as_file());
            }
        }

        let content: Arc<str> = tokio::fs::read_to_string(path)
            .await
            .with_context(|| format!("failed to read {}", path.display()))?
            .into();

        let (file, sync) = {
            let mut documents = self.documents.lock().await;
            match documents.get_mut(path) {
                // A stamp that moved without the bytes moving still means nothing to send.
                Some(open) if open.content == content => {
                    open.stamp = stamp;
                    return Ok(open.as_file());
                }
                Some(open) => {
                    open.version += 1;
                    open.content = Arc::clone(&content);
                    open.stamp = stamp;
                    (open.as_file(), Sync::Changed(open.version))
                }
                None => {
                    let document = OpenDocument {
                        version: 1,
                        content: Arc::clone(&content),
                        uri: Arc::from(uri::from_path(path)?),
                        stamp,
                    };
                    let file = document.as_file();
                    documents.insert(path.to_path_buf(), document);
                    (file, Sync::Opened)
                }
            }
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

        // A document the server was never told about must not stay in the map claiming otherwise.
        if let Err(error) = sent {
            self.documents.lock().await.remove(path);
            return Err(error);
        }
        Ok(file)
    }

    /// The symbol tree of `path`, alongside the text it was built from.
    ///
    /// The text comes back because `ensure_open` has already produced it: every caller needs both,
    /// and asking for them separately read the same file from disk twice.
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
            bail_hint!(
                "set project.memory_only to false in .biskit/settings.yml and restart the server, \
                 or use search_for_pattern and find_file instead";
                "Biskit is in memory-only mode, so the Luau language server is not available"
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

    /// Starts the language server in the background so the first tool call does not pay for it.
    ///
    /// Acquisition, `initialize`, definition file loading and the server's own workspace indexing
    /// add up to seconds at exactly the moment an agent is trying to do its first piece of work.
    /// The session mutex means a real caller that arrives mid-startup waits on this attempt rather
    /// than beginning a second one, so the only cost is starting a server that is never used.
    ///
    /// Failures are logged and dropped: the first real tool call runs the same path and reports
    /// the failure properly, with its hint, to the caller who asked for it.
    pub fn warm_up(self: &Arc<Self>) {
        if self.settings.project.memory_only {
            return;
        }
        let handle = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(error) = handle.session().await {
                tracing::warn!(
                    target: "biskit::lsp",
                    "background language server start failed, retrying on first use: {error}"
                );
            }
        });
    }

    pub async fn restart(&self) -> Result<()> {
        self.stop().await;
        self.session().await.map(|_| ())
    }

    pub async fn stop(&self) {
        let mut guard = self.session.lock().await;
        // Shut down through the `Arc` rather than requiring sole ownership of it. Demanding
        // ownership meant that any tool call still holding a clone silently skipped the shutdown,
        // leaving the old luau-lsp process resident with every document it had open.
        if let Some(existing) = guard.take() {
            existing.shutdown().await;
        }
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Every `.luau` and `.lua` file under `base`, or under the project root when `base` is absent.
    ///
    /// Taking a base means a query scoped to one directory walks that directory instead of walking
    /// the whole project and discarding everything outside it.
    pub async fn resolve_luau_files(&self, base: Option<&Path>) -> Result<Vec<PathBuf>> {
        let root = base.unwrap_or(self.project.root()).to_path_buf();
        let settings = self.settings.project.clone();

        tokio::task::spawn_blocking(move || {
            let mut found = Vec::new();
            for entry in crate::project::walk_builder(&root, &settings)?
                .build()
                .filter_map(Result::ok)
            {
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
    bail_hint!(
        "the symbol tools only read .luau and .lua; locate one with find_file using the mask \
         \"*.luau\", or use search_for_pattern for other file types";
        "not a Luau source file: {}",
        path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A project shaped like a real repository: a fat `.git`, a `.biskit`, a vendored tree, and
    /// Luau spread over two directories.
    fn fixture() -> (tempfile::TempDir, Project) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        let objects = root.join(".git").join("objects").join("ab");
        std::fs::create_dir_all(&objects).unwrap();
        for index in 0..8 {
            std::fs::write(objects.join(format!("object{index}.luau")), "return {}\n").unwrap();
        }
        std::fs::create_dir_all(root.join(".biskit")).unwrap();
        std::fs::write(root.join(".biskit").join("cached.luau"), "return {}\n").unwrap();

        for directory in ["src/Services", "src/Shared", "Packages"] {
            std::fs::create_dir_all(root.join(directory)).unwrap();
            std::fs::write(
                root.join(directory).join("Module.luau"),
                "local Module = {}\nreturn Module\n",
            )
            .unwrap();
        }
        std::fs::write(root.join("src").join("legacy.lua"), "return {}\n").unwrap();
        std::fs::write(root.join("README.md"), "not luau\n").unwrap();

        let project = Project::open(root).unwrap();
        (dir, project)
    }

    fn scan(project: &Project, settings: Settings, base: Option<&Path>) -> Vec<String> {
        let handle = LanguageServerHandle::new(project.clone(), settings);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime
            .block_on(handle.resolve_luau_files(base))
            .unwrap()
            .iter()
            .map(|path| project.relativize(path).unwrap())
            .collect()
    }

    #[test]
    fn the_scan_skips_git_and_biskit() {
        let (_dir, project) = fixture();
        assert_eq!(
            scan(&project, Settings::default(), None),
            vec![
                "Packages/Module.luau".to_string(),
                "src/Services/Module.luau".to_string(),
                "src/Shared/Module.luau".to_string(),
                "src/legacy.lua".to_string(),
            ]
        );
    }

    #[test]
    fn ignored_paths_are_honoured_by_the_scan() {
        let (_dir, project) = fixture();
        let mut settings = Settings::default();
        settings.project.ignored_paths = vec!["Packages/".to_string(), "**/Shared".to_string()];

        assert_eq!(
            scan(&project, settings, None),
            vec![
                "src/Services/Module.luau".to_string(),
                "src/legacy.lua".to_string(),
            ]
        );
    }

    #[test]
    fn a_base_narrows_the_scan_to_that_subtree() {
        let (_dir, project) = fixture();
        let base = project.root().join("src").join("Services");

        assert_eq!(
            scan(&project, Settings::default(), Some(&base)),
            vec!["src/Services/Module.luau".to_string()]
        );
    }

    #[test]
    fn an_invalid_ignored_path_fails_the_scan_rather_than_being_dropped() {
        let (_dir, project) = fixture();
        let mut settings = Settings::default();
        settings.project.ignored_paths = vec!["[".to_string()];

        let handle = LanguageServerHandle::new(project, settings);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let error = runtime
            .block_on(handle.resolve_luau_files(None))
            .unwrap_err()
            .to_string();
        assert!(error.contains("ignored_paths"), "unexpected error: {error}");
    }
}
