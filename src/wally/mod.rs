mod cli;
mod manifest;
mod registry;

use anyhow::Result;
use serde::Serialize;

use crate::bail_hint;
use crate::config::{Settings, WallySettings};
use crate::project::Project;
use crate::serde_skip::is_false;

pub use crate::wally::cli::WallyProgram;
pub use crate::wally::manifest::{DeclaredPackage, ManifestPackage, Realm};
pub use crate::wally::registry::SearchHit;

const NO_INSTALL_NOTE: &str = "wally.toml was edited but `wally install` was not run, because \
                               install was false. The manifest and the package tree disagree \
                               until it runs.";

const NO_PLACE_NOTE: &str = "wally.toml declares no [place] shared-packages, so Wally cannot link \
                             a server or dev package that depends on a shared one, and the \
                             install will fail for any package that does. Add, for example: \
                             [place] with shared-packages = \
                             \"game.ReplicatedStorage.Packages\".";

const PLACE_HINT: &str = "Wally needs wally.toml to say where shared packages sit in the datamodel \
                          before a server or dev package may depend on one. Add a [place] table \
                          with shared-packages = \"game.ReplicatedStorage.Packages\", pointing at \
                          wherever this place puts Packages.";

/// `wally install` empties the package tree before it rebuilds it, so a failure leaves packages
/// missing that had nothing to do with the call that prompted it.
const TREE_WARNING: &str = "`wally install` deletes Packages, ServerPackages, and DevPackages \
                            before it rebuilds them, so packages unrelated to this call may now be \
                            missing from disk. Run `wally install` again once the cause is fixed \
                            to restore them.";

/// Wally-shaped facts about a project, plus the throttled registry client shared across calls.
pub struct WallyTools {
    project: Project,
    settings: WallySettings,
    registry: registry::RegistryClient,
}

#[derive(Debug, Serialize)]
pub struct PackageListing {
    pub manifest_path: String,
    pub package: ManifestPackage,
    pub wally: WallyProgram,
    pub lockfile_present: bool,
    pub packages: Vec<DeclaredPackage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SearchAnswer {
    pub query: String,
    pub registry: String,
    pub results: Vec<SearchHit>,
    #[serde(skip_serializing_if = "is_false")]
    pub truncated: bool,
    /// How many packages matched before `max_results` cut the list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ManifestChange {
    pub manifest_path: String,
    pub alias: String,
    pub package: String,
    pub version_req: String,
    pub realm: Realm,
    /// The wally.toml table the entry was written to or removed from.
    pub section: &'static str,
    /// The requirement this one displaced, when it replaced an existing declaration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replaced: Option<String>,
    /// Whether `wally install` ran, which is not a claim about what is on disk. Ask
    /// `list_wally_packages` for that.
    pub install_ran: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install_output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

pub struct AddRequest<'a> {
    pub package: &'a str,
    pub version: Option<&'a str>,
    pub realm: Option<&'a str>,
    pub alias: Option<&'a str>,
    pub overwrite: bool,
    pub install: bool,
}

pub struct RemoveRequest<'a> {
    pub package: &'a str,
    pub realm: Option<&'a str>,
    pub install: bool,
}

impl WallyTools {
    pub fn new(project: Project, settings: &Settings) -> Self {
        Self {
            registry: registry::RegistryClient::new(settings.wally.clone()),
            settings: settings.wally.clone(),
            project,
        }
    }

    /// What wally.toml declares, what wally.lock resolved it to, and what is on disk.
    pub async fn list(&self) -> Result<PackageListing> {
        let program = self.program()?;
        let project = self.project.clone();

        tokio::task::spawn_blocking(move || {
            let manifest = manifest::Manifest::load(&project)?;
            let lockfile = manifest::Lockfile::load(&project, manifest.own_name().as_deref())?;
            let packages = manifest.declared(&project, lockfile.as_ref());

            let missing = packages.iter().filter(|entry| !entry.installed).count();
            let note = match (lockfile.is_some(), packages.len(), missing) {
                (_, 0, _) => Some(format!(
                    "{} declares no dependencies yet",
                    manifest.relative_path
                )),
                (false, _, _) => Some(
                    "there is no wally.lock, so nothing has been resolved or installed. Run \
                     add_wally_package, or `wally install` for what is already declared."
                        .to_string(),
                ),
                (true, _, 0) => None,
                (true, _, missing) => Some(format!(
                    "{missing} declared package(s) are not on disk, so requires into them will \
                     fail until `wally install` runs"
                )),
            };

            Ok(PackageListing {
                manifest_path: manifest.relative_path.clone(),
                package: manifest.package_summary(),
                wally: program,
                lockfile_present: lockfile.is_some(),
                packages,
                note,
            })
        })
        .await?
    }

    /// Searches the registry by scope, name, and description.
    pub async fn search(&self, query: &str, max_results: usize) -> Result<SearchAnswer> {
        self.program()?;

        let query = query.trim();
        if query.is_empty() {
            bail_hint!(
                "pass a name, scope, or word from a description, such as \"promise\"";
                "the search query is empty"
            );
        }
        if max_results == 0 {
            bail_hint!(
                "pass a max_results of 1 or more, or omit it to take the wally.max_search_results \
                 default";
                "max_results is 0, so a search could return nothing whatever matched"
            );
        }

        let mut results = self.registry.search(query).await?;
        let matched_count = results.len();
        let truncated = matched_count > max_results;
        results.truncate(max_results);

        Ok(SearchAnswer {
            query: query.to_string(),
            registry: self.settings.registry_api_url.clone(),
            results,
            truncated,
            matched_count: truncated.then_some(matched_count),
            note: match (truncated, matched_count) {
                (true, _) => Some(format!(
                    "{matched_count} packages matched and the list was cut to {max_results}. \
                     Narrow the query or raise max_results."
                )),
                (false, 0) => Some(
                    "no packages matched. The registry matches on scope, name, and description, \
                     so try a shorter or more general word."
                        .to_string(),
                ),
                _ => None,
            },
        })
    }

    /// Writes a dependency into wally.toml and, unless asked not to, installs it.
    pub async fn add(&self, request: AddRequest<'_>) -> Result<ManifestChange> {
        let program = self.program()?;
        let (name, inline_version) = manifest::split_requirement(request.package)?;

        let version_req = match (request.version, inline_version) {
            (Some(explicit), Some(inline)) if explicit.trim() != inline => bail_hint!(
                "give the version once: either inside package as \"scope/name@^1.0.0\" or as the \
                 version argument";
                "two different versions were given: {inline:?} in package and {explicit:?} in \
                 version"
            ),
            (Some(explicit), _) => manifest::check_version_req(explicit)?,
            (None, Some(inline)) => manifest::check_version_req(inline)?,
            (None, None) => format!("^{}", self.registry.latest_version(&name).await?),
        };

        let realm = Realm::parse(request.realm)?;
        let alias = match request.alias {
            Some(alias) => manifest::check_alias(alias)?.to_string(),
            None => manifest::default_alias(&name),
        };

        let project = self.project.clone();
        let overwrite = request.overwrite;
        let edit = tokio::task::spawn_blocking({
            let name = name.clone();
            let alias = alias.clone();
            let version_req = version_req.clone();
            move || {
                let mut manifest = manifest::Manifest::load(&project)?;
                let original = manifest.original().to_string();
                let placed = manifest.declares_shared_place();
                let replaced = manifest.insert(&alias, &name, &version_req, realm, overwrite)?;
                manifest.save()?;
                Ok::<_, anyhow::Error>(Edit {
                    manifest_path: manifest.relative_path,
                    original,
                    placed,
                    replaced,
                })
            }
        })
        .await??;

        let install_output = match request.install {
            true => Some(self.install_or_roll_back(&program, edit.original).await?),
            false => None,
        };

        let unplaced = realm != Realm::Shared && !edit.placed;
        let notes = [
            (!request.install).then_some(NO_INSTALL_NOTE),
            unplaced.then_some(NO_PLACE_NOTE),
        ];

        Ok(ManifestChange {
            manifest_path: edit.manifest_path,
            alias,
            package: name.to_string(),
            version_req,
            realm,
            section: realm.section(),
            replaced: edit.replaced,
            install_ran: install_output.is_some(),
            install_output,
            note: join(notes),
        })
    }

    /// Drops a dependency from wally.toml and, unless asked not to, rebuilds the package tree.
    pub async fn remove(&self, request: RemoveRequest<'_>) -> Result<ManifestChange> {
        let program = self.program()?;
        let realm = request
            .realm
            .map(|realm| Realm::parse(Some(realm)))
            .transpose()?;

        let project = self.project.clone();
        let target = request.package.trim().to_string();
        let (manifest_path, original, removed) = tokio::task::spawn_blocking(move || {
            let mut manifest = manifest::Manifest::load(&project)?;
            let original = manifest.original().to_string();
            let removed = manifest.remove(&target, realm)?;
            manifest.save()?;
            Ok::<_, anyhow::Error>((manifest.relative_path, original, removed))
        })
        .await??;

        let install_output = match request.install {
            true => Some(self.install_or_roll_back(&program, original).await?),
            false => None,
        };

        Ok(ManifestChange {
            manifest_path,
            alias: removed.alias,
            package: removed.package,
            version_req: removed.version_req,
            realm: removed.realm,
            section: removed.realm.section(),
            replaced: None,
            install_ran: install_output.is_some(),
            install_output,
            note: match request.install {
                true => None,
                false => Some(format!(
                    "{NO_INSTALL_NOTE} The removed package is still on disk until then."
                )),
            },
        })
    }

    /// Runs `wally install`, and puts wally.toml back the way it was when the install fails.
    ///
    /// A failed install is not a no-op: Wally has already emptied the package tree by the time it
    /// gives up. Undoing the manifest edit at least leaves the manifest telling the truth, and the
    /// error says what is still missing from disk.
    async fn install_or_roll_back(
        &self,
        program: &WallyProgram,
        original: String,
    ) -> Result<String> {
        let failure = match cli::install(program, &self.project, &self.settings).await {
            Ok(output) => return Ok(output),
            Err(failure) => failure,
        };

        let printed = failure.to_string();
        let project = self.project.clone();
        let (rolled_back, missing) = tokio::task::spawn_blocking(move || {
            let rolled_back = manifest::restore(&project, &original).is_ok();
            (rolled_back, missing_from_disk(&project))
        })
        .await
        .unwrap_or((false, Vec::new()));

        let mut hint = match rolled_back {
            true => "the wally.toml edit was rolled back, so the manifest reads as it did before \
                     this call. "
                .to_string(),
            false => "the wally.toml edit could not be rolled back, so the manifest still holds \
                      it. "
                .to_string(),
        };
        hint.push_str(TREE_WARNING);
        if !missing.is_empty() {
            hint.push_str(&format!(" Not on disk now: {}.", missing.join(", ")));
        }
        if printed.contains("shared dependency") || printed.contains("shared packages are placed") {
            hint.push(' ');
            hint.push_str(PLACE_HINT);
        }

        Err(crate::errors::rehinted(failure, hint))
    }

    /// Wally must be on PATH and must run before any of these tools answer.
    fn program(&self) -> Result<WallyProgram> {
        cli::locate(&self.settings)
    }
}

/// What the manifest edit produced, carried past the blocking task that made it.
struct Edit {
    manifest_path: String,
    original: String,
    placed: bool,
    replaced: Option<String>,
}

fn join<const N: usize>(notes: [Option<&str>; N]) -> Option<String> {
    let joined = notes.into_iter().flatten().collect::<Vec<_>>().join(" ");
    (!joined.is_empty()).then_some(joined)
}

/// Every declared package the lockfile resolved that is not on disk, for reporting after a failed
/// install took the tree down with it.
fn missing_from_disk(project: &Project) -> Vec<String> {
    let Ok(manifest) = manifest::Manifest::load(project) else {
        return Vec::new();
    };
    let lockfile = manifest::Lockfile::load(project, manifest.own_name().as_deref())
        .ok()
        .flatten();

    manifest
        .declared(project, lockfile.as_ref())
        .into_iter()
        .filter(|entry| !entry.installed && entry.resolved_version.is_some())
        .map(|entry| entry.package)
        .collect()
}
