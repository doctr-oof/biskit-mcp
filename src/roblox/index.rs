use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use anyhow::{Result, anyhow};

use super::api::RobloxApi;
use super::requires::{self, RequireGraph};
use super::sourcemap::Sourcemap;
use crate::config::Settings;
use crate::project::Project;

/// The Roblox-shaped facts about a project, held across tool calls.
pub struct RobloxIndex {
    project: Project,
    settings: Settings,
    sourcemap: Mutex<Option<Arc<Sourcemap>>>,
    graph: Mutex<Option<Arc<RequireGraph>>>,
    api: Mutex<Option<Arc<RobloxApi>>>,
}

impl RobloxIndex {
    pub fn new(project: Project, settings: Settings) -> Self {
        Self {
            project,
            settings,
            sourcemap: Mutex::new(None),
            graph: Mutex::new(None),
            api: Mutex::new(None),
        }
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    /// The instance tree, reloaded when the sourcemap on disk has moved.
    pub async fn sourcemap(&self) -> Result<Arc<Sourcemap>> {
        let cached = self.cached(&self.sourcemap)?;
        let project = self.project.clone();
        let settings = self.settings.clone();

        let loaded = tokio::task::spawn_blocking(move || -> Result<Arc<Sourcemap>> {
            if let Some(existing) = cached {
                let current = sourcemap_stamp(&project, &settings);
                if current.is_some() && current == existing.stamp() {
                    return Ok(existing);
                }
            }
            Ok(Arc::new(Sourcemap::load(&project, &settings)?))
        })
        .await
        .map_err(|error| anyhow!("sourcemap load panicked: {error}"))??;

        *self.sourcemap.lock().map_err(|_| poisoned("sourcemap"))? = Some(Arc::clone(&loaded));
        Ok(loaded)
    }

    /// The module dependency graph, rebuilt only when a Luau file has moved.
    pub async fn require_graph(&self) -> Result<Arc<RequireGraph>> {
        let sourcemap = self.sourcemap().await?;
        let cached = self.cached(&self.graph)?;
        let project = self.project.clone();
        let settings = self.settings.clone();

        let built = tokio::task::spawn_blocking(move || {
            requires::build_or_reuse(cached, &project, &settings, &sourcemap)
        })
        .await
        .map_err(|error| anyhow!("require graph build panicked: {error}"))??;

        *self.graph.lock().map_err(|_| poisoned("require graph"))? = Some(Arc::clone(&built));
        Ok(built)
    }

    /// The cached Roblox type definitions and API documentation.
    pub(crate) async fn api(&self) -> Result<Arc<RobloxApi>> {
        if let Some(existing) = self.cached(&self.api)? {
            return Ok(existing);
        }
        let settings = self.settings.lsp.clone();
        let loaded = tokio::task::spawn_blocking(move || RobloxApi::load(&settings))
            .await
            .map_err(|error| anyhow!("Roblox API load panicked: {error}"))??;

        let loaded = Arc::new(loaded);
        *self.api.lock().map_err(|_| poisoned("Roblox API"))? = Some(Arc::clone(&loaded));
        Ok(loaded)
    }

    fn cached<T>(&self, slot: &Mutex<Option<Arc<T>>>) -> Result<Option<Arc<T>>> {
        Ok(slot.lock().map_err(|_| poisoned("index"))?.clone())
    }
}

fn sourcemap_stamp(project: &Project, settings: &Settings) -> Option<(SystemTime, u64)> {
    let relative = settings.lsp.sourcemap.as_ref()?;
    let path = project.resolve(relative).ok()?;
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}

fn poisoned(what: &str) -> anyhow::Error {
    anyhow!("the {what} cache lock was poisoned")
}
