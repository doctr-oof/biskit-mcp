use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Result, anyhow};
use tokio::sync::Mutex;
use tokio::time::Instant;

use super::Session;
use crate::bail_hint;
use crate::config::Settings;
use crate::lsp::cache::{SourceStamp, SymbolCache};
use crate::lsp::symbols::SymbolNode;
use crate::project::Project;

/// Wraps the session so it can be restarted without tearing down the MCP server.
pub struct LanguageServerHandle {
    project: Project,
    settings: Settings,
    session: Mutex<Option<Arc<Session>>>,
    symbols: SymbolCache,
}

/// Whether a language server is up, reported without waiting on one that is coming up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerState {
    Disabled,
    Running,
    NotStarted,
    Starting,
    Crashed,
}

impl ServerState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Running => "running",
            Self::NotStarted => "not started",
            Self::Starting => "starting",
            Self::Crashed => "exited, restarts on the next request",
        }
    }
}

impl LanguageServerHandle {
    pub fn new(project: Project, settings: Settings) -> Self {
        let symbols = SymbolCache::new(&project, &settings.tools);
        Self {
            project,
            settings,
            session: Mutex::new(None),
            symbols,
        }
    }

    /// The symbol tree of `path` and the text it was built from, served from the persistent index when the file has not moved since it was last indexed.
    pub async fn document_symbols(
        &self,
        session: &Session,
        path: &Path,
    ) -> Result<(Vec<SymbolNode>, Arc<str>)> {
        let key = match self.symbols.enabled() {
            true => self.project.relativize(path).ok(),
            false => None,
        };
        let stamp = match &key {
            Some(_) => SourceStamp::of(path).await,
            None => None,
        };

        if let (Some(key), Some(stamp)) = (&key, stamp)
            && let Some(cached) = self.symbols.get(key, stamp).await
            && let Ok(content) = tokio::fs::read_to_string(path).await
            && SourceStamp::of(path).await == Some(stamp)
        {
            return Ok((cached, Arc::from(content)));
        }

        let (symbols, content) = session.document_symbols(path).await?;
        if let (Some(key), Some(stamp)) = (&key, stamp)
            && SourceStamp::of(path).await == Some(stamp)
        {
            self.symbols.put(key, stamp, &symbols).await;
        }
        Ok((symbols, content))
    }

    pub fn symbol_cache(&self) -> &SymbolCache {
        &self.symbols
    }

    pub async fn session(&self) -> Result<Arc<Session>> {
        if self.settings.project.memory_only {
            bail_hint!(
                "set project.memory_only to false in .biskit/settings.yml and restart the server, \
                 or use search_for_pattern and find_file";
                "Biskit is in memory-only mode, so the Luau language server is not available"
            );
        }

        let mut guard = self.session.lock().await;
        match guard.as_ref() {
            Some(existing) if existing.is_alive() => return Ok(Arc::clone(existing)),
            Some(_) => {
                tracing::warn!(
                    target: "biskit::lsp",
                    "the language server had exited, starting a replacement"
                );
                if let Some(dead) = guard.take() {
                    dead.shutdown().await;
                }
            }
            None => {}
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

    /// The state of the session without waiting for it.
    pub fn state(&self) -> ServerState {
        if self.settings.project.memory_only {
            return ServerState::Disabled;
        }
        match self.session.try_lock() {
            Ok(guard) => match guard.as_ref() {
                Some(session) if session.is_alive() => ServerState::Running,
                Some(_) => ServerState::Crashed,
                None => ServerState::NotStarted,
            },
            Err(_) => ServerState::Starting,
        }
    }

    pub async fn restart(&self) -> Result<()> {
        self.stop().await;
        self.session().await.map(|_| ())
    }

    pub async fn stop(&self) {
        self.symbols.flush().await;

        let mut guard = self.session.lock().await;
        if let Some(existing) = guard.take() {
            existing.shutdown().await;
        }
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Every `.luau` and `.lua` file under `base`, or under the project root when `base` is absent.
    pub async fn resolve_luau_files(&self, base: Option<&Path>) -> Result<Vec<PathBuf>> {
        let root = self.project.root().to_path_buf();
        let start = base.unwrap_or(self.project.root()).to_path_buf();
        let settings = self.settings.project.clone();

        tokio::task::spawn_blocking(move || {
            let mut found = Vec::new();
            for entry in crate::project::walk_builder(&root, &start, &settings)?
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
