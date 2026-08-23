use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use super::symbols::SymbolNode;
use crate::config::ToolSettings;
use crate::project::Project;

pub const CACHE_DIR: &str = "cache";
const INDEX_FILE: &str = "symbols.json";
const GITIGNORE_CONTENTS: &str = "*\n";

/// Bumped whenever the stored shape changes. An index written by another version is dropped rather
/// than half-read, because a symbol tree that deserialises into a shape nobody expects is worse
/// than a cold start.
const FORMAT_VERSION: u32 = 1;

/// How many newly indexed files accumulate before the index is written out.
///
/// A project-wide `find_symbol` indexes hundreds of files in one sweep, and writing after each of
/// them would rewrite the whole index hundreds of times. Waiting for shutdown alone would lose the
/// whole sweep whenever the process is killed, which for a server speaking over stdio is a normal
/// way to end.
const FLUSH_EVERY: usize = 64;

/// Size and modification time of a source file, which together decide whether a stored tree still
/// describes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceStamp {
    modified_nanos: u64,
    len: u64,
}

impl SourceStamp {
    pub async fn of(path: &Path) -> Option<Self> {
        let metadata = tokio::fs::metadata(path).await.ok()?;
        Self::from_metadata(&metadata)
    }

    fn from_metadata(metadata: &std::fs::Metadata) -> Option<Self> {
        let since = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
        Some(Self {
            modified_nanos: since.as_nanos() as u64,
            len: metadata.len(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    #[serde(flatten)]
    stamp: SourceStamp,
    /// Value of the clock when this entry was last reached for, so eviction drops what sessions
    /// never ask about rather than whatever the map happens to iterate first.
    touched: u64,
    symbols: Vec<SymbolNode>,
}

#[derive(Debug, Deserialize)]
struct Index {
    version: u32,
    #[serde(default)]
    clock: u64,
    #[serde(default)]
    entries: HashMap<String, Entry>,
}

/// The write-side view, which borrows the entries instead of cloning the whole index to serialise
/// it. The entries are the largest thing Biskit holds in memory on a big project.
#[derive(Debug, Serialize)]
struct IndexRef<'a> {
    version: u32,
    clock: u64,
    entries: &'a HashMap<String, Entry>,
}

#[derive(Debug, Default)]
struct State {
    loaded: bool,
    /// Entries added since the last write.
    pending: usize,
    clock: u64,
    entries: HashMap<String, Entry>,
}

/// Symbol trees kept across sessions, keyed by path plus size plus modification time.
///
/// `find_symbol` with no `relative_path` issues one `documentSymbol` round trip per file that
/// survives the literal prefilter, through a single stdio pipe, every session from cold. Almost
/// none of those files have changed since the last session asked about them, so almost none of
/// those round trips buy anything.
pub struct SymbolCache {
    root: PathBuf,
    file: PathBuf,
    enabled: bool,
    /// Entries kept before the least recently reached are dropped. Zero means no ceiling.
    capacity: usize,
    state: Mutex<State>,
}

impl SymbolCache {
    pub fn new(project: &Project, settings: &ToolSettings) -> Self {
        Self {
            root: project.root().to_path_buf(),
            file: cache_dir(project).join(INDEX_FILE),
            enabled: settings.symbol_cache,
            capacity: settings.max_cached_symbol_files,
            state: Mutex::new(State::default()),
        }
    }

    /// The stored tree for `key`, or nothing when the file has moved since it was indexed.
    pub async fn get(&self, key: &str, stamp: SourceStamp) -> Option<Vec<SymbolNode>> {
        if !self.enabled {
            return None;
        }

        let mut state = self.state.lock().await;
        self.load(&mut state).await;
        state.clock += 1;
        let clock = state.clock;

        let entry = state.entries.get_mut(key)?;
        if entry.stamp != stamp {
            return None;
        }
        entry.touched = clock;
        Some(entry.symbols.clone())
    }

    pub async fn put(&self, key: &str, stamp: SourceStamp, symbols: &[SymbolNode]) {
        if !self.enabled {
            return;
        }

        let payload = {
            let mut state = self.state.lock().await;
            self.load(&mut state).await;
            state.clock += 1;

            let touched = state.clock;
            state.entries.insert(
                key.to_string(),
                Entry {
                    stamp,
                    touched,
                    symbols: symbols.to_vec(),
                },
            );
            state.pending += 1;

            if state.pending < FLUSH_EVERY {
                return;
            }
            self.snapshot(&mut state)
        };
        self.write(payload).await;
    }

    /// Writes the index out when anything has been indexed since the last write.
    pub async fn flush(&self) {
        if !self.enabled {
            return;
        }

        let payload = {
            let mut state = self.state.lock().await;
            if state.pending == 0 {
                return;
            }
            self.snapshot(&mut state)
        };
        self.write(payload).await;
    }

    /// How many trees are held, loading the index if nothing has needed it yet.
    pub async fn entry_count(&self) -> usize {
        let mut state = self.state.lock().await;
        self.load(&mut state).await;
        state.entries.len()
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Deletes a project's cache directory, reporting whether there was one.
    pub fn clear(project: &Project) -> Result<bool> {
        let directory = cache_dir(project);
        if !directory.exists() {
            return Ok(false);
        }
        std::fs::remove_dir_all(&directory)
            .with_context(|| format!("failed to remove {}", directory.display()))?;
        Ok(true)
    }

    /// Reads the index from disk once per process. A failure to read is a cold cache, not an
    /// error: every answer the cache would have given can still be asked of the language server.
    async fn load(&self, state: &mut State) {
        if state.loaded {
            return;
        }
        state.loaded = true;

        let file = self.file.clone();
        let root = self.root.clone();
        let read = tokio::task::spawn_blocking(move || read_index(&file, &root)).await;

        if let Ok(Some(index)) = read {
            state.clock = index.clock;
            state.entries = index.entries;
        }
    }

    fn snapshot(&self, state: &mut State) -> Option<Vec<u8>> {
        evict(&mut state.entries, self.capacity);
        state.pending = 0;
        serde_json::to_vec(&IndexRef {
            version: FORMAT_VERSION,
            clock: state.clock,
            entries: &state.entries,
        })
        .ok()
    }

    async fn write(&self, payload: Option<Vec<u8>>) {
        let Some(payload) = payload else {
            return;
        };
        let file = self.file.clone();

        match tokio::task::spawn_blocking(move || write_index(&file, &payload)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::warn!(target: "biskit::cache", "symbol index write failed: {error}");
            }
            Err(error) => {
                tracing::warn!(target: "biskit::cache", "symbol index write panicked: {error}");
            }
        }
    }
}

pub fn cache_dir(project: &Project) -> PathBuf {
    project.biskit_dir().join(CACHE_DIR)
}

fn read_index(file: &Path, root: &Path) -> Option<Index> {
    let raw = std::fs::read(file).ok()?;
    let mut index: Index = serde_json::from_slice(&raw).ok()?;
    if index.version != FORMAT_VERSION {
        return None;
    }

    // A file deleted since it was indexed can never be asked about again, so its tree is dead
    // weight in every later read of this index.
    index
        .entries
        .retain(|relative, _| root.join(relative).is_file());
    Some(index)
}

fn write_index(file: &Path, payload: &[u8]) -> Result<()> {
    let directory = file
        .parent()
        .context("the symbol index has no parent directory")?;
    std::fs::create_dir_all(directory)
        .with_context(|| format!("failed to create {}", directory.display()))?;

    // `.biskit` is checked in, and nothing under the cache belongs in a commit. Writing the rule
    // beside the index covers projects whose `.biskit/.gitignore` predates the cache.
    let gitignore = directory.join(".gitignore");
    if !gitignore.exists() {
        let _ = std::fs::write(&gitignore, GITIGNORE_CONTENTS);
    }

    // Written beside the index and renamed over it, so a process that dies mid-write leaves the
    // previous index intact rather than a half-written one that parses into nothing.
    let temporary = directory.join("symbols.json.writing");
    std::fs::write(&temporary, payload)
        .with_context(|| format!("failed to write {}", temporary.display()))?;
    std::fs::rename(&temporary, file)
        .with_context(|| format!("failed to replace {}", file.display()))?;
    Ok(())
}

/// Drops the least recently reached entries until the index fits its ceiling.
fn evict(entries: &mut HashMap<String, Entry>, capacity: usize) {
    if capacity == 0 || entries.len() <= capacity {
        return;
    }

    let excess = entries.len() - capacity;
    let mut touched: Vec<u64> = entries.values().map(|entry| entry.touched).collect();
    touched.sort_unstable();

    // Entries tied on the cutoff all go, which can take the index below the ceiling. Keeping some
    // of a tie and not the rest would need a second key that says nothing about usefulness.
    let cutoff = touched[excess - 1];
    entries.retain(|_, entry| entry.touched > cutoff);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::protocol::{Position, Range};

    fn settings() -> ToolSettings {
        ToolSettings::default()
    }

    fn tree(name: &str) -> Vec<SymbolNode> {
        let range = Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 1,
                character: 0,
            },
        };
        vec![SymbolNode {
            name: name.to_string(),
            name_path: name.to_string(),
            kind: 12,
            detail: None,
            range,
            selection_range: range,
            children: Vec::new(),
            member: false,
        }]
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        project: Project,
    }

    impl Fixture {
        fn build() -> Self {
            let dir = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(dir.path().join(".biskit")).unwrap();
            std::fs::write(dir.path().join("Module.luau"), "return {}\n").unwrap();
            let project = Project::open(dir.path()).unwrap();
            Self { _dir: dir, project }
        }

        fn stamp(&self, relative: &str) -> SourceStamp {
            let metadata = std::fs::metadata(self.project.root().join(relative)).unwrap();
            SourceStamp::from_metadata(&metadata).unwrap()
        }

        fn rewrite(&self, relative: &str, contents: &str) {
            let path = self.project.root().join(relative);
            // Two writes inside one filesystem tick would leave the stamp unchanged, which is the
            // one case the cache cannot see, so the length is moved as well.
            std::fs::write(path, contents).unwrap();
        }
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn a_stored_tree_comes_back_for_an_unchanged_file() {
        let fixture = Fixture::build();
        let cache = SymbolCache::new(&fixture.project, &settings());

        runtime().block_on(async {
            let stamp = fixture.stamp("Module.luau");
            assert!(cache.get("Module.luau", stamp).await.is_none());

            cache.put("Module.luau", stamp, &tree("update")).await;
            let found = cache.get("Module.luau", stamp).await.unwrap();
            assert_eq!(found[0].name, "update");
        });
    }

    #[test]
    fn a_file_that_has_moved_since_it_was_indexed_misses() {
        let fixture = Fixture::build();
        let cache = SymbolCache::new(&fixture.project, &settings());

        runtime().block_on(async {
            let stamp = fixture.stamp("Module.luau");
            cache.put("Module.luau", stamp, &tree("update")).await;

            fixture.rewrite("Module.luau", "local Added = 1\nreturn {}\n");
            let moved = fixture.stamp("Module.luau");
            assert_ne!(stamp, moved);
            assert!(cache.get("Module.luau", moved).await.is_none());
        });
    }

    #[test]
    fn the_index_survives_into_a_second_cache_over_the_same_project() {
        let fixture = Fixture::build();
        let stamp = fixture.stamp("Module.luau");

        runtime().block_on(async {
            let first = SymbolCache::new(&fixture.project, &settings());
            first.put("Module.luau", stamp, &tree("update")).await;
            first.flush().await;

            let second = SymbolCache::new(&fixture.project, &settings());
            let found = second.get("Module.luau", stamp).await.unwrap();
            assert_eq!(found[0].name, "update");
        });
    }

    #[test]
    fn entries_for_deleted_files_are_dropped_when_the_index_is_read() {
        let fixture = Fixture::build();
        let stamp = fixture.stamp("Module.luau");

        runtime().block_on(async {
            let first = SymbolCache::new(&fixture.project, &settings());
            first.put("Module.luau", stamp, &tree("update")).await;
            first.put("Gone.luau", stamp, &tree("removed")).await;
            first.flush().await;

            let second = SymbolCache::new(&fixture.project, &settings());
            assert_eq!(second.entry_count().await, 1);
            assert!(second.get("Gone.luau", stamp).await.is_none());
        });
    }

    #[test]
    fn an_index_written_by_another_format_version_is_ignored() {
        let fixture = Fixture::build();
        let directory = cache_dir(&fixture.project);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join(INDEX_FILE),
            serde_json::json!({"version": FORMAT_VERSION + 1, "entries": {}}).to_string(),
        )
        .unwrap();

        let cache = SymbolCache::new(&fixture.project, &settings());
        runtime().block_on(async {
            assert_eq!(cache.entry_count().await, 0);
        });
    }

    #[test]
    fn a_disabled_cache_stores_nothing_and_writes_nothing() {
        let fixture = Fixture::build();
        let cache = SymbolCache::new(
            &fixture.project,
            &ToolSettings {
                symbol_cache: false,
                ..ToolSettings::default()
            },
        );

        runtime().block_on(async {
            let stamp = fixture.stamp("Module.luau");
            cache.put("Module.luau", stamp, &tree("update")).await;
            cache.flush().await;

            assert!(cache.get("Module.luau", stamp).await.is_none());
        });
        assert!(!cache_dir(&fixture.project).exists());
    }

    #[test]
    fn clearing_removes_the_cache_directory() {
        let fixture = Fixture::build();
        let stamp = fixture.stamp("Module.luau");

        runtime().block_on(async {
            let cache = SymbolCache::new(&fixture.project, &settings());
            cache.put("Module.luau", stamp, &tree("update")).await;
            cache.flush().await;
        });

        assert!(cache_dir(&fixture.project).join(INDEX_FILE).is_file());
        assert!(SymbolCache::clear(&fixture.project).unwrap());
        assert!(!cache_dir(&fixture.project).exists());
        assert!(!SymbolCache::clear(&fixture.project).unwrap());
    }

    #[test]
    fn the_written_index_is_gitignored() {
        let fixture = Fixture::build();
        let stamp = fixture.stamp("Module.luau");

        runtime().block_on(async {
            let cache = SymbolCache::new(&fixture.project, &settings());
            cache.put("Module.luau", stamp, &tree("update")).await;
            cache.flush().await;
        });

        let gitignore = cache_dir(&fixture.project).join(".gitignore");
        assert_eq!(std::fs::read_to_string(gitignore).unwrap(), "*\n");
    }

    #[test]
    fn eviction_keeps_the_entries_that_were_reached_for_most_recently() {
        let mut entries = HashMap::new();
        for (name, touched) in [("a", 1u64), ("b", 2), ("c", 3), ("d", 4)] {
            entries.insert(
                name.to_string(),
                Entry {
                    stamp: SourceStamp {
                        modified_nanos: 0,
                        len: 0,
                    },
                    touched,
                    symbols: Vec::new(),
                },
            );
        }

        evict(&mut entries, 2);
        let mut kept: Vec<&String> = entries.keys().collect();
        kept.sort();
        assert_eq!(kept, vec!["c", "d"]);

        evict(&mut entries, 0);
        assert_eq!(entries.len(), 2, "a ceiling of zero is no ceiling");
    }
}
