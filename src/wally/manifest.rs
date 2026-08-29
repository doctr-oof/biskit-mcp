use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use toml_edit::{DocumentMut, Item, Table, value};

use crate::bail_hint;
use crate::project::Project;
use crate::serde_skip::is_false;

pub const MANIFEST_FILE: &str = "wally.toml";
pub const LOCKFILE_FILE: &str = "wally.lock";

const NO_MANIFEST_HINT: &str = "run `wally init` in the project root to create one, or check that \
                                Biskit's project root is the directory that owns wally.toml";

const ALREADY_DECLARED_HINT: &str = "pass overwrite to replace the requirement, or call \
                                     remove_wally_package first";

const NOT_DECLARED_HINT: &str = "call list_wally_packages to see what wally.toml declares";

/// Which dependency table an entry lives in, and which directory Wally installs it into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Realm {
    Shared,
    Server,
    Dev,
}

impl Realm {
    pub const ALL: [Realm; 3] = [Realm::Shared, Realm::Server, Realm::Dev];

    /// The wally.toml table this realm's dependencies are declared in.
    pub fn section(self) -> &'static str {
        match self {
            Self::Shared => "dependencies",
            Self::Server => "server-dependencies",
            Self::Dev => "dev-dependencies",
        }
    }

    /// The directory `wally install` writes this realm's packages into.
    pub fn directory(self) -> &'static str {
        match self {
            Self::Shared => "Packages",
            Self::Server => "ServerPackages",
            Self::Dev => "DevPackages",
        }
    }

    pub fn parse(value: Option<&str>) -> Result<Self> {
        let Some(value) = value else {
            return Ok(Self::Shared);
        };
        match value.trim().to_ascii_lowercase().as_str() {
            "shared" | "dependencies" => Ok(Self::Shared),
            "server" | "server-dependencies" => Ok(Self::Server),
            "dev" | "dev-dependencies" => Ok(Self::Dev),
            other => bail_hint!(
                "realm is one of \"shared\", \"server\", or \"dev\"";
                "unknown realm: {other:?}"
            ),
        }
    }
}

/// A `scope/name` package name, validated the way Wally validates it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageName {
    scope: String,
    name: String,
}

impl PackageName {
    pub fn parse(value: &str) -> Result<Self> {
        let value = value.trim();
        let Some((scope, name)) = value.split_once('/') else {
            bail_hint!(
                "a package is named SCOPE/NAME, such as \"roblox/roact\"";
                "package name has no scope: {value:?}"
            );
        };
        if name.contains('/') {
            bail_hint!(
                "a package is named SCOPE/NAME, such as \"roblox/roact\"";
                "package name has more than one slash: {value:?}"
            );
        }

        check_segment("scope", scope)?;
        check_segment("name", name)?;
        Ok(Self {
            scope: scope.to_string(),
            name: name.to_string(),
        })
    }

    pub fn scope(&self) -> &str {
        &self.scope
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The `scope_name@version` directory Wally unpacks a package into under `_Index`.
    pub fn index_directory(&self, version: &str) -> String {
        format!("{}_{}@{version}", self.scope, self.name)
    }
}

impl fmt::Display for PackageName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.scope, self.name)
    }
}

fn check_segment(label: &str, value: &str) -> Result<()> {
    let valid = !value.is_empty()
        && value.len() <= 64
        && value.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        });
    if valid {
        return Ok(());
    }
    if value
        .chars()
        .any(|character| character.is_ascii_uppercase())
    {
        bail_hint!(
            format!("Wally names are lower case: try {:?}", value.to_ascii_lowercase());
            "a package {label} may only contain lower-case letters, digits, and hyphens: {value:?}"
        );
    }
    bail!(
        "a package {label} may only contain lower-case letters, digits, and hyphens, and must be \
         1 to 64 characters: {value:?}"
    )
}

/// Splits `scope/name` or `scope/name@^1.0.0` into its two halves.
pub fn split_requirement(value: &str) -> Result<(PackageName, Option<&str>)> {
    let value = value.trim();
    match value.split_once('@') {
        Some((name, version)) => Ok((PackageName::parse(name)?, Some(version.trim()))),
        None => Ok((PackageName::parse(value)?, None)),
    }
}

/// Refuses a version requirement `wally install` would later reject.
pub fn check_version_req(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        bail_hint!(
            "write a semver requirement such as \"^1.4.2\", or omit version to take the newest \
             published release";
            "the version requirement is empty"
        );
    }
    VersionReq::parse(value)
        .with_context(|| format!("invalid version requirement: {value:?}"))
        .map_err(|error| {
            crate::errors::hinted(
                error.to_string(),
                "Wally takes Cargo-style semver requirements, such as \"^1.4.2\", \"1.4\", or \
                 \"=1.4.2\"",
            )
        })?;
    Ok(value.to_string())
}

/// Refuses an alias that would not be a usable Luau identifier at the require site.
pub fn check_alias(value: &str) -> Result<&str> {
    let value = value.trim();
    let valid = !value.is_empty()
        && !value.starts_with(|character: char| character.is_ascii_digit())
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_');
    if valid {
        return Ok(value);
    }
    bail_hint!(
        "the alias becomes the module's name under Packages/, so it must read as a Luau \
         identifier, such as \"Roact\"";
        "invalid alias: {value:?}"
    )
}

/// `roblox/roact` becomes `Roact`, which is what a require site expects to write.
pub fn default_alias(name: &PackageName) -> String {
    let mut characters = name.name().chars();
    let mut alias = String::with_capacity(name.name().len());
    if let Some(first) = characters.next() {
        alias.extend(first.to_uppercase());
    }
    for character in characters {
        match character {
            '-' => {}
            other => alias.push(other),
        }
    }
    alias
}

#[derive(Debug, Clone, Serialize)]
pub struct ManifestPackage {
    pub name: String,
    pub version: String,
    pub realm: String,
    pub registry: String,
}

/// One entry of a wally.toml dependency table, joined to what wally.lock and disk say about it.
#[derive(Debug, Clone, Serialize)]
pub struct DeclaredPackage {
    /// The key in wally.toml, which is the name the package is required by.
    pub alias: String,
    pub package: String,
    pub scope: String,
    pub name: String,
    pub version_req: String,
    pub realm: Realm,
    pub section: &'static str,
    /// The version wally.lock resolved the requirement to, absent when nothing has resolved it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_version: Option<String>,
    pub installed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install_path: Option<String>,
    /// True when the resolved version no longer satisfies the requirement in wally.toml.
    #[serde(skip_serializing_if = "is_false")]
    pub lock_out_of_date: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// One declaration found by a lookup: its realm, alias, requirement as written, and package name.
type Declaration = (Realm, String, String, String);

fn listed(declarations: &[Declaration]) -> String {
    declarations
        .iter()
        .map(|(realm, alias, _, _)| format!("{alias} under [{}]", realm.section()))
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Debug)]
pub struct RemovedPackage {
    pub alias: String,
    pub package: String,
    pub version_req: String,
    pub realm: Realm,
}

#[derive(Debug)]
pub struct Manifest {
    document: DocumentMut,
    original: String,
    pub relative_path: String,
    path: PathBuf,
}

/// Puts wally.toml back to text it held earlier, for undoing an edit whose install then failed.
pub fn restore(project: &Project, contents: &str) -> Result<()> {
    let path = project.resolve(MANIFEST_FILE)?;
    std::fs::write(&path, contents).with_context(|| format!("failed to restore {MANIFEST_FILE}"))
}

impl Manifest {
    pub fn load(project: &Project) -> Result<Self> {
        let path = project.resolve(MANIFEST_FILE)?;
        if !path.is_file() {
            bail_hint!(NO_MANIFEST_HINT; "this project has no {MANIFEST_FILE}");
        }

        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {MANIFEST_FILE}"))?;
        let document = raw
            .parse::<DocumentMut>()
            .with_context(|| format!("failed to parse {MANIFEST_FILE}"))?;

        Ok(Self {
            document,
            original: raw,
            relative_path: MANIFEST_FILE.to_string(),
            path,
        })
    }

    pub fn save(&self) -> Result<()> {
        std::fs::write(&self.path, self.document.to_string())
            .with_context(|| format!("failed to write {MANIFEST_FILE}"))
    }

    /// The file exactly as it was read, which is what a rolled-back edit is restored to.
    pub fn original(&self) -> &str {
        &self.original
    }

    /// Whether wally.toml says where shared packages sit in the datamodel. Wally refuses to link a
    /// server or dev package that depends on a shared one until it does.
    pub fn declares_shared_place(&self) -> bool {
        self.document
            .get("place")
            .and_then(Item::as_table_like)
            .and_then(|table| table.get("shared-packages"))
            .and_then(Item::as_str)
            .is_some_and(|value| !value.trim().is_empty())
    }

    /// The `[package] name` this manifest declares, which names the root entry in wally.lock.
    pub fn own_name(&self) -> Option<String> {
        self.document
            .get("package")
            .and_then(Item::as_table_like)
            .and_then(|table| table.get("name"))
            .and_then(Item::as_str)
            .map(str::to_string)
    }

    pub fn package_summary(&self) -> ManifestPackage {
        let read = |key: &str| {
            self.document
                .get("package")
                .and_then(Item::as_table_like)
                .and_then(|table| table.get(key))
                .and_then(Item::as_str)
                .unwrap_or_default()
                .to_string()
        };
        ManifestPackage {
            name: read("name"),
            version: read("version"),
            realm: read("realm"),
            registry: read("registry"),
        }
    }

    /// Every declared dependency, in realm order, joined to the lockfile and to disk.
    pub fn declared(&self, project: &Project, lockfile: Option<&Lockfile>) -> Vec<DeclaredPackage> {
        let mut declared = Vec::new();

        for realm in Realm::ALL {
            let Some(table) = self
                .document
                .get(realm.section())
                .and_then(Item::as_table_like)
            else {
                continue;
            };

            for (alias, item) in table.iter() {
                let Some(requirement) = item.as_str() else {
                    declared.push(unreadable(alias, realm));
                    continue;
                };
                declared.push(self.describe(project, lockfile, alias, requirement, realm));
            }
        }

        declared
    }

    fn describe(
        &self,
        project: &Project,
        lockfile: Option<&Lockfile>,
        alias: &str,
        requirement: &str,
        realm: Realm,
    ) -> DeclaredPackage {
        let Ok((name, version_req)) = split_requirement(requirement) else {
            return DeclaredPackage {
                alias: alias.to_string(),
                package: requirement.to_string(),
                scope: String::new(),
                name: String::new(),
                version_req: String::new(),
                realm,
                section: realm.section(),
                resolved_version: None,
                installed: false,
                install_path: None,
                lock_out_of_date: false,
                note: Some(format!(
                    "this entry is not a package requirement Wally can parse; it should read \
                     SCOPE/NAME@VERSION, such as \"roblox/roact@^1.4.2\", not {requirement:?}"
                )),
            };
        };

        let version_req = version_req.unwrap_or_default().to_string();
        let resolved = lockfile.and_then(|lockfile| lockfile.resolved(alias, &name));

        let install_path = resolved.as_ref().map(|version| {
            format!(
                "{}/_Index/{}",
                realm.directory(),
                name.index_directory(version)
            )
        });
        let index = install_path
            .as_ref()
            .and_then(|relative| project.resolve(relative).ok());
        // An index directory can exist and hold nothing, which a failed install leaves behind. What
        // the alias link file requires is the package's own folder inside it, so that is the test.
        let present = index.as_ref().is_some_and(|path| path.is_dir());
        let installed = index.is_some_and(|path| path.join(name.name()).exists());

        let lock_out_of_date = match (&resolved, VersionReq::parse(&version_req)) {
            (Some(version), Ok(request)) => Version::parse(version)
                .map(|version| !request.matches(&version))
                .unwrap_or(false),
            _ => false,
        };

        let note = match (&resolved, installed, present, lock_out_of_date) {
            (_, _, _, true) => Some(
                "wally.lock resolved a version the requirement in wally.toml no longer allows. Run \
                 `wally install` to re-resolve."
                    .to_string(),
            ),
            (None, _, _, _) => Some(
                "wally.lock does not resolve this requirement, so it has never been installed"
                    .to_string(),
            ),
            (Some(_), false, true, _) => Some(format!(
                "the index directory exists but holds no {:?} folder, so it was left half written \
                 by an install that did not finish; run `wally install` to rebuild it",
                name.name()
            )),
            (Some(_), false, false, _) => Some(
                "resolved in wally.lock but absent from disk; run `wally install` to restore it"
                    .to_string(),
            ),
            _ => None,
        };

        DeclaredPackage {
            alias: alias.to_string(),
            package: name.to_string(),
            scope: name.scope().to_string(),
            name: name.name().to_string(),
            version_req,
            realm,
            section: realm.section(),
            resolved_version: resolved,
            installed,
            install_path: install_path.filter(|_| present),
            lock_out_of_date,
            note,
        }
    }

    /// Writes a requirement into a realm's table, returning what it replaced.
    ///
    /// A package lives in exactly one realm here: an insert sweeps its declarations out of the
    /// other two, so `overwrite` is what moves a package between realms rather than duplicating it.
    pub fn insert(
        &mut self,
        alias: &str,
        name: &PackageName,
        version_req: &str,
        realm: Realm,
        overwrite: bool,
    ) -> Result<Option<String>> {
        if let Some((section, existing_alias, existing)) = self.find(name)
            && !overwrite
        {
            let target = realm.section();
            if section == target {
                bail_hint!(
                    ALREADY_DECLARED_HINT;
                    "{name} is already declared as {existing_alias} = {existing:?} under [{section}]"
                );
            }
            bail_hint!(
                format!(
                    "a package is declared in one realm only, so pass overwrite to move it into \
                     [{target}], or call remove_wally_package first"
                );
                "{name} is already declared as {existing_alias} = {existing:?} under [{section}], \
                 and this call would put it under [{target}]"
            );
        }

        // Every alias that names this package anywhere, plus the alias about to be written, so a
        // package moving realm or changing alias leaves no second declaration behind.
        let stale: Vec<(Realm, Vec<String>)> = Realm::ALL
            .into_iter()
            .map(|other| {
                let mut aliases = self.aliases_for(other, name);
                if other == realm && !aliases.iter().any(|found| found == alias) {
                    aliases.push(alias.to_string());
                }
                (other, aliases)
            })
            .collect();

        let mut replaced = None;
        for (other, aliases) in stale {
            let Some(table) = self
                .document
                .get_mut(other.section())
                .and_then(Item::as_table_like_mut)
            else {
                continue;
            };
            for alias in aliases {
                if let Some(removed) = table.remove(&alias) {
                    replaced = replaced.or_else(|| removed.as_str().map(str::to_string));
                }
            }
        }

        let table = self
            .document
            .entry(realm.section())
            .or_insert_with(|| Item::Table(Table::new()))
            .as_table_like_mut()
            .with_context(|| {
                format!(
                    "[{}] in {MANIFEST_FILE} is not a table, so no dependency can be written to it",
                    realm.section()
                )
            })?;

        table.insert(alias, value(format!("{name}@{version_req}")));
        Ok(replaced)
    }

    /// Removes a dependency named by package name or by alias.
    pub fn remove(&mut self, target: &str, realm: Option<Realm>) -> Result<RemovedPackage> {
        let realms: Vec<Realm> = match realm {
            Some(realm) => vec![realm],
            None => Realm::ALL.to_vec(),
        };

        let mut matches = self.declarations_of(target, &realms);

        if matches.is_empty() {
            // Saying "no such dependency" when it is declared one table over reads as a lie, so a
            // realm-scoped miss names where the package actually sits.
            if let Some(requested) = realm {
                let elsewhere = self.declarations_of(target, &Realm::ALL);
                if !elsewhere.is_empty() {
                    bail_hint!(
                        format!(
                            "omit realm, or pass the realm it is declared in. A package is \
                             declared in one realm only, so realm is needed only when \
                             {MANIFEST_FILE} was hand written to say otherwise"
                        );
                        "{MANIFEST_FILE} declares {target:?} as {}, not under [{}]",
                        listed(&elsewhere),
                        requested.section()
                    );
                }
            }
            bail_hint!(NOT_DECLARED_HINT; "{MANIFEST_FILE} declares no dependency {target:?}");
        }
        if matches.len() > 1 {
            bail_hint!(
                "pass realm to say which one to remove";
                "{target:?} is declared more than once: {}",
                listed(&matches)
            );
        }

        let (realm, alias, requirement, package) = matches.remove(0);
        if let Some(table) = self
            .document
            .get_mut(realm.section())
            .and_then(Item::as_table_like_mut)
        {
            table.remove(&alias);
        }

        Ok(RemovedPackage {
            alias,
            version_req: split_requirement(&requirement)
                .ok()
                .and_then(|(_, version)| version.map(str::to_string))
                .unwrap_or_default(),
            package,
            realm,
        })
    }

    /// Every declaration the target names, by alias or by package name, across the given realms.
    fn declarations_of(&self, target: &str, realms: &[Realm]) -> Vec<Declaration> {
        let mut found = Vec::new();
        for realm in realms.iter().copied() {
            let Some(table) = self
                .document
                .get(realm.section())
                .and_then(Item::as_table_like)
            else {
                continue;
            };
            for (alias, item) in table.iter() {
                let Some(requirement) = item.as_str() else {
                    continue;
                };
                let package = split_requirement(requirement)
                    .map(|(name, _)| name.to_string())
                    .unwrap_or_default();
                if alias == target || package == target {
                    found.push((realm, alias.to_string(), requirement.to_string(), package));
                }
            }
        }
        found
    }

    /// The first declaration of a package, wherever it sits.
    fn find(&self, name: &PackageName) -> Option<(&'static str, String, String)> {
        for realm in Realm::ALL {
            let Some(table) = self
                .document
                .get(realm.section())
                .and_then(Item::as_table_like)
            else {
                continue;
            };
            for (alias, item) in table.iter() {
                let Some(requirement) = item.as_str() else {
                    continue;
                };
                if split_requirement(requirement).is_ok_and(|(found, _)| &found == name) {
                    return Some((realm.section(), alias.to_string(), requirement.to_string()));
                }
            }
        }
        None
    }

    fn aliases_for(&self, realm: Realm, name: &PackageName) -> Vec<String> {
        let Some(table) = self
            .document
            .get(realm.section())
            .and_then(Item::as_table_like)
        else {
            return Vec::new();
        };
        table
            .iter()
            .filter(|(_, item)| {
                item.as_str()
                    .and_then(|requirement| split_requirement(requirement).ok())
                    .is_some_and(|(found, _)| &found == name)
            })
            .map(|(alias, _)| alias.to_string())
            .collect()
    }
}

/// The parts of wally.lock Biskit reads: what each package resolved to, and the root's aliases.
pub struct Lockfile {
    /// Alias to resolved version, taken from the root package's dependency list.
    by_alias: BTreeMap<String, String>,
    /// Package name to every resolved version, for the entries no alias reaches.
    by_name: BTreeMap<String, Vec<String>>,
}

impl Lockfile {
    pub fn load(project: &Project, root_name: Option<&str>) -> Result<Option<Self>> {
        let path = project.resolve(LOCKFILE_FILE)?;
        if !path.is_file() {
            return Ok(None);
        }
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {LOCKFILE_FILE}"))?;
        Ok(Some(Self::parse(&raw, root_name)?))
    }

    /// `root_name` is the manifest's own `[package] name`, which is the one lockfile entry whose
    /// dependency list carries the aliases wally.toml wrote.
    pub fn parse(raw: &str, root_name: Option<&str>) -> Result<Self> {
        let document = raw
            .parse::<DocumentMut>()
            .with_context(|| format!("failed to parse {LOCKFILE_FILE}"))?;

        let mut by_alias = BTreeMap::new();
        let mut by_name: BTreeMap<String, Vec<String>> = BTreeMap::new();

        let Some(packages) = document.get("package").and_then(Item::as_array_of_tables) else {
            return Ok(Self { by_alias, by_name });
        };

        for entry in packages {
            let Some(name) = entry.get("name").and_then(Item::as_str) else {
                continue;
            };
            if let Some(version) = entry.get("version").and_then(Item::as_str) {
                by_name
                    .entry(name.to_string())
                    .or_default()
                    .push(version.to_string());
            }

            if Some(name) != root_name {
                continue;
            }

            let Some(dependencies) = entry.get("dependencies").and_then(Item::as_array) else {
                continue;
            };
            for dependency in dependencies {
                let Some(pair) = dependency.as_array() else {
                    continue;
                };
                let alias = pair.get(0).and_then(|value| value.as_str());
                let id = pair.get(1).and_then(|value| value.as_str());
                if let (Some(alias), Some(id)) = (alias, id)
                    && let Some((_, version)) = id.rsplit_once('@')
                {
                    by_alias.insert(alias.to_string(), version.to_string());
                }
            }
        }

        Ok(Self { by_alias, by_name })
    }

    /// The version this alias resolved to, falling back to the name when the root entry is absent.
    pub fn resolved(&self, alias: &str, name: &PackageName) -> Option<String> {
        if let Some(version) = self.by_alias.get(alias) {
            return Some(version.clone());
        }
        match self.by_name.get(&name.to_string()) {
            Some(versions) if versions.len() == 1 => Some(versions[0].clone()),
            _ => None,
        }
    }
}

fn unreadable(alias: &str, realm: Realm) -> DeclaredPackage {
    DeclaredPackage {
        alias: alias.to_string(),
        package: String::new(),
        scope: String::new(),
        name: String::new(),
        version_req: String::new(),
        realm,
        section: realm.section(),
        resolved_version: None,
        installed: false,
        install_path: None,
        lock_out_of_date: false,
        note: Some(
            "this entry is not a string, so it is not a requirement Wally can read".to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = "\
# Hand written, and it should stay that way.
[package]
name = \"me/game\"
version = \"0.1.0\"
registry = \"https://github.com/UpliftGames/wally-index\"
realm = \"shared\"

[dependencies]
Roact = \"roblox/roact@^1.4.2\"  # the UI library

[server-dependencies]
ProfileService = \"madstudioroblox/profileservice@^1.0.0\"
";

    fn open(contents: &str) -> (tempfile::TempDir, Project, Manifest) {
        let directory = tempfile::tempdir().unwrap();
        let project = Project::open(directory.path()).unwrap();
        std::fs::write(project.root().join(MANIFEST_FILE), contents).unwrap();
        let manifest = Manifest::load(&project).unwrap();
        (directory, project, manifest)
    }

    #[test]
    fn a_missing_manifest_says_so_rather_than_reporting_no_dependencies() {
        let directory = tempfile::tempdir().unwrap();
        let project = Project::open(directory.path()).unwrap();
        let error = Manifest::load(&project).unwrap_err();
        assert!(error.to_string().contains("has no wally.toml"));
    }

    #[test]
    fn comments_and_formatting_survive_an_edit() {
        let (_directory, project, mut manifest) = open(MANIFEST);
        let name = PackageName::parse("evaera/promise").unwrap();
        manifest
            .insert("Promise", &name, "^4.0.0", Realm::Shared, false)
            .unwrap();
        manifest.save().unwrap();

        let written = std::fs::read_to_string(project.root().join(MANIFEST_FILE)).unwrap();
        assert!(written.contains("# Hand written, and it should stay that way."));
        assert!(
            written.contains("Roact = \"roblox/roact@^1.4.2\"  # the UI library"),
            "an untouched line kept its trailing comment: {written}"
        );
        assert!(written.contains("Promise = \"evaera/promise@^4.0.0\""));
    }

    #[test]
    fn a_package_already_declared_is_refused_until_overwrite_is_passed() {
        let (_directory, _project, mut manifest) = open(MANIFEST);
        let name = PackageName::parse("roblox/roact").unwrap();

        let error = manifest
            .insert("Roact", &name, "^1.5.0", Realm::Shared, false)
            .unwrap_err();
        assert!(error.to_string().contains("already declared as Roact"));

        let replaced = manifest
            .insert("Roact", &name, "^1.5.0", Realm::Shared, true)
            .unwrap();
        assert_eq!(replaced.as_deref(), Some("roblox/roact@^1.4.2"));
    }

    #[test]
    fn moving_a_package_to_another_realm_leaves_no_second_declaration() {
        let (_directory, project, mut manifest) = open(MANIFEST);
        let name = PackageName::parse("roblox/roact").unwrap();

        manifest
            .insert("Roact", &name, "^1.4.2", Realm::Server, true)
            .unwrap();
        manifest.save().unwrap();

        let written = std::fs::read_to_string(project.root().join(MANIFEST_FILE)).unwrap();
        assert_eq!(
            written.matches("roblox/roact").count(),
            1,
            "the package must be declared once, not in both realms: {written}"
        );

        let reopened = Manifest::load(&project).unwrap();
        let declared = reopened.declared(&project, None);
        let roact = declared.iter().find(|entry| entry.name == "roact").unwrap();
        assert_eq!(roact.realm, Realm::Server);
        assert_eq!(roact.section, "server-dependencies");
    }

    #[test]
    fn renaming_the_alias_of_a_declared_package_does_not_leave_the_old_key() {
        let (_directory, project, mut manifest) = open(MANIFEST);
        let name = PackageName::parse("roblox/roact").unwrap();

        manifest
            .insert("RoactUi", &name, "^1.4.2", Realm::Shared, true)
            .unwrap();
        manifest.save().unwrap();

        let written = std::fs::read_to_string(project.root().join(MANIFEST_FILE)).unwrap();
        assert!(written.contains("RoactUi = \"roblox/roact@^1.4.2\""));
        assert!(
            !written.contains("\nRoact = "),
            "the old alias should be gone: {written}"
        );
    }

    #[test]
    fn a_dependency_table_that_does_not_exist_yet_is_created() {
        let (_directory, project, mut manifest) = open(MANIFEST);
        let name = PackageName::parse("evaera/promise").unwrap();
        manifest
            .insert("Promise", &name, "^4.0.0", Realm::Dev, false)
            .unwrap();
        manifest.save().unwrap();

        let written = std::fs::read_to_string(project.root().join(MANIFEST_FILE)).unwrap();
        assert!(written.contains("[dev-dependencies]"));
        assert!(written.contains("Promise = \"evaera/promise@^4.0.0\""));
    }

    #[test]
    fn a_package_can_be_removed_by_name_or_by_alias() {
        for target in ["roblox/roact", "Roact"] {
            let (_directory, project, mut manifest) = open(MANIFEST);
            let removed = manifest.remove(target, None).unwrap();
            assert_eq!(removed.alias, "Roact");
            assert_eq!(removed.package, "roblox/roact");
            assert_eq!(removed.version_req, "^1.4.2");
            assert_eq!(removed.realm, Realm::Shared);

            manifest.save().unwrap();
            let written = std::fs::read_to_string(project.root().join(MANIFEST_FILE)).unwrap();
            assert!(!written.contains("roblox/roact"), "removing {target:?}");
            assert!(written.contains("madstudioroblox/profileservice"));
        }
    }

    #[test]
    fn removing_something_undeclared_names_it_rather_than_succeeding_quietly() {
        let (_directory, _project, mut manifest) = open(MANIFEST);
        let error = manifest.remove("nobody/nothing", None).unwrap_err();
        assert!(error.to_string().contains("declares no dependency"));
    }

    #[test]
    fn a_name_declared_in_two_realms_asks_which_one_to_remove() {
        let both = format!("{MANIFEST}\n[dev-dependencies]\nRoact = \"roblox/roact@^1.4.2\"\n");
        let (_directory, _project, mut manifest) = open(&both);

        let error = manifest.remove("roblox/roact", None).unwrap_err();
        let rendered = crate::errors::render("remove_wally_package", &error);
        assert!(rendered.contains("declared more than once"));
        assert!(rendered.contains("Roact under [dependencies]"));
        assert!(rendered.contains("hint: pass realm"));

        assert_eq!(
            manifest
                .remove("roblox/roact", Some(Realm::Dev))
                .unwrap()
                .realm,
            Realm::Dev
        );
    }

    #[test]
    fn a_declared_package_reports_what_the_lockfile_resolved_and_whether_it_is_on_disk() {
        let (_directory, project, manifest) = open(MANIFEST);
        let lock = "\
registry = \"https://github.com/UpliftGames/wally-index\"

[[package]]
name = \"me/game\"
version = \"0.1.0\"
dependencies = [
\t[\"Roact\", \"roblox/roact@1.4.4\"],
]

[[package]]
name = \"roblox/roact\"
version = \"1.4.4\"
dependencies = []
";
        let lockfile = Lockfile::parse(lock, Some("me/game")).unwrap();
        std::fs::create_dir_all(
            project
                .root()
                .join("Packages/_Index/roblox_roact@1.4.4/roact"),
        )
        .unwrap();

        let declared = manifest.declared(&project, Some(&lockfile));
        let roact = declared.iter().find(|entry| entry.name == "roact").unwrap();

        assert_eq!(roact.resolved_version.as_deref(), Some("1.4.4"));
        assert!(roact.installed);
        assert_eq!(
            roact.install_path.as_deref(),
            Some("Packages/_Index/roblox_roact@1.4.4")
        );
        assert!(!roact.lock_out_of_date);
        assert!(roact.note.is_none());

        let profile = declared
            .iter()
            .find(|entry| entry.name == "profileservice")
            .unwrap();
        assert_eq!(profile.realm, Realm::Server);
        assert!(
            !profile.installed,
            "nothing was written under ServerPackages"
        );
        assert!(profile.install_path.is_none());
        assert!(
            profile
                .note
                .as_deref()
                .is_some_and(|note| note.contains("never been installed"))
        );
    }

    #[test]
    fn an_index_directory_with_nothing_in_it_is_not_reported_as_installed() {
        let (_directory, project, manifest) = open(MANIFEST);
        let lock = "\
registry = \"x\"

[[package]]
name = \"me/game\"
version = \"0.1.0\"
dependencies = [
\t[\"Roact\", \"roblox/roact@1.4.4\"],
]
";
        let lockfile = Lockfile::parse(lock, Some("me/game")).unwrap();
        std::fs::create_dir_all(project.root().join("Packages/_Index/roblox_roact@1.4.4")).unwrap();

        let declared = manifest.declared(&project, Some(&lockfile));
        let roact = declared.iter().find(|entry| entry.name == "roact").unwrap();

        assert!(
            !roact.installed,
            "a failed install leaves the index directory behind empty, and every require into it \
             still fails"
        );
        assert_eq!(
            roact.install_path.as_deref(),
            Some("Packages/_Index/roblox_roact@1.4.4"),
            "the path is worth naming so the half written directory can be found"
        );
        assert!(
            roact
                .note
                .as_deref()
                .is_some_and(|note| note.contains("half written")),
            "got: {:?}",
            roact.note
        );
    }

    #[test]
    fn a_package_declared_in_another_realm_says_which_one_and_what_would_move_it() {
        let (_directory, _project, mut manifest) = open(MANIFEST);
        let name = PackageName::parse("roblox/roact").unwrap();

        let error = manifest
            .insert("Roact", &name, "^1.4.2", Realm::Dev, false)
            .unwrap_err();
        let rendered = crate::errors::render("add_wally_package", &error);

        assert!(rendered.contains("under [dependencies]"));
        assert!(rendered.contains("[dev-dependencies]"));
        assert!(rendered.contains("hint: a package is declared in one realm only"));
    }

    #[test]
    fn removing_from_the_wrong_realm_says_where_the_package_actually_is() {
        let (_directory, _project, mut manifest) = open(MANIFEST);

        let error = manifest
            .remove("roblox/roact", Some(Realm::Server))
            .unwrap_err();
        let rendered = crate::errors::render("remove_wally_package", &error);

        assert!(
            rendered.contains("Roact under [dependencies]"),
            "the miss must name the section it was found in, not deny the declaration: {rendered}"
        );
        assert!(rendered.contains("not under [server-dependencies]"));
        assert!(rendered.contains("hint: omit realm"));
    }

    #[test]
    fn an_edit_can_be_put_back_byte_for_byte() {
        let (_directory, project, mut manifest) = open(MANIFEST);
        let original = manifest.original().to_string();

        let name = PackageName::parse("evaera/promise").unwrap();
        manifest
            .insert("Promise", &name, "^4.0.0", Realm::Shared, false)
            .unwrap();
        manifest.save().unwrap();

        restore(&project, &original).unwrap();
        let written = std::fs::read_to_string(project.root().join(MANIFEST_FILE)).unwrap();
        assert_eq!(written, MANIFEST, "a rolled back edit leaves no trace");
    }

    #[test]
    fn a_manifest_is_asked_whether_it_says_where_shared_packages_live() {
        let (_directory, _project, manifest) = open(MANIFEST);
        assert!(
            !manifest.declares_shared_place(),
            "this manifest has no [place] table"
        );

        let placed =
            format!("{MANIFEST}\n[place]\nshared-packages = \"game.ReplicatedStorage.Packages\"\n");
        let (_directory, _project, manifest) = open(&placed);
        assert!(manifest.declares_shared_place());

        let blank = format!("{MANIFEST}\n[place]\nshared-packages = \"\"\n");
        let (_directory, _project, manifest) = open(&blank);
        assert!(
            !manifest.declares_shared_place(),
            "an empty value tells Wally nothing"
        );
    }

    #[test]
    fn a_lockfile_version_the_requirement_no_longer_allows_is_called_out() {
        let (_directory, project, manifest) = open(MANIFEST);
        let lock = "\
registry = \"x\"

[[package]]
name = \"me/game\"
version = \"0.1.0\"
dependencies = [
\t[\"Roact\", \"roblox/roact@0.9.0\"],
]
";
        let lockfile = Lockfile::parse(lock, Some("me/game")).unwrap();
        let declared = manifest.declared(&project, Some(&lockfile));
        let roact = declared.iter().find(|entry| entry.name == "roact").unwrap();

        assert_eq!(roact.resolved_version.as_deref(), Some("0.9.0"));
        assert!(roact.lock_out_of_date, "0.9.0 does not satisfy ^1.4.2");
        assert!(
            roact
                .note
                .as_deref()
                .is_some_and(|note| note.contains("re-resolve"))
        );
    }

    #[test]
    fn a_lockfile_with_no_root_entry_still_resolves_an_unambiguous_name() {
        let lock = "\
registry = \"x\"

[[package]]
name = \"roblox/roact\"
version = \"1.4.4\"
dependencies = []
";
        let lockfile = Lockfile::parse(lock, Some("me/game")).unwrap();
        let name = PackageName::parse("roblox/roact").unwrap();
        assert_eq!(lockfile.resolved("Roact", &name).as_deref(), Some("1.4.4"));
    }

    #[test]
    fn two_resolved_versions_of_one_name_are_not_guessed_between() {
        let lock = "\
registry = \"x\"

[[package]]
name = \"roblox/roact\"
version = \"1.4.4\"
dependencies = []

[[package]]
name = \"roblox/roact\"
version = \"1.3.0\"
dependencies = []
";
        let lockfile = Lockfile::parse(lock, None).unwrap();
        let name = PackageName::parse("roblox/roact").unwrap();
        assert_eq!(lockfile.resolved("Roact", &name), None);
    }

    #[test]
    fn an_entry_that_is_not_a_requirement_is_reported_rather_than_dropped() {
        let broken = "[package]\nname = \"me/game\"\n\n[dependencies]\nRoact = \"roact\"\n";
        let (_directory, project, manifest) = open(broken);

        let declared = manifest.declared(&project, None);
        assert_eq!(declared.len(), 1);
        assert_eq!(declared[0].alias, "Roact");
        assert!(
            declared[0]
                .note
                .as_deref()
                .is_some_and(|note| note.contains("SCOPE/NAME@VERSION"))
        );
    }

    #[test]
    fn package_names_are_held_to_wallys_own_rules() {
        assert_eq!(
            PackageName::parse("roblox/roact").unwrap().to_string(),
            "roblox/roact"
        );
        assert!(PackageName::parse("roact").is_err(), "a scope is required");
        assert!(PackageName::parse("a/b/c").is_err());
        assert!(PackageName::parse("roblox/ro act").is_err());
        assert!(PackageName::parse("/roact").is_err());

        let uppercase = PackageName::parse("Roblox/Roact").unwrap_err();
        let rendered = crate::errors::render("add_wally_package", &uppercase);
        assert!(
            rendered.contains("hint: Wally names are lower case: try \"roblox\""),
            "the hint should name the lower-case form, got: {rendered}"
        );
    }

    #[test]
    fn a_version_written_inline_is_split_off_the_name() {
        let (name, version) = split_requirement("roblox/roact@^1.4.2").unwrap();
        assert_eq!(name.to_string(), "roblox/roact");
        assert_eq!(version, Some("^1.4.2"));

        let (name, version) = split_requirement("roblox/roact").unwrap();
        assert_eq!(name.to_string(), "roblox/roact");
        assert_eq!(version, None);
    }

    #[test]
    fn a_requirement_wally_would_reject_is_refused_before_the_manifest_is_touched() {
        assert!(check_version_req("^1.4.2").is_ok());
        assert!(check_version_req("1.4").is_ok());
        assert!(check_version_req("=1.4.2").is_ok());
        assert!(check_version_req("").is_err());
        assert!(check_version_req("latest").is_err());
    }

    #[test]
    fn the_default_alias_is_the_name_a_require_site_would_write() {
        let alias = |value: &str| default_alias(&PackageName::parse(value).unwrap());
        assert_eq!(alias("roblox/roact"), "Roact");
        assert_eq!(alias("evaera/promise"), "Promise");
        assert_eq!(alias("sleitnick/knit"), "Knit");
        assert_eq!(
            alias("madstudioroblox/profile-service"),
            "Profileservice",
            "hyphens are dropped, since an alias becomes an Instance name"
        );
    }

    #[test]
    fn an_alias_that_is_not_an_identifier_is_refused() {
        assert!(check_alias("Roact").is_ok());
        assert!(check_alias("Roact_17").is_ok());
        assert!(check_alias("17Roact").is_err());
        assert!(check_alias("Ro act").is_err());
        assert!(check_alias("Roact-Ui").is_err());
        assert!(check_alias("").is_err());
    }

    #[test]
    fn realms_map_to_the_tables_and_directories_wally_uses() {
        assert_eq!(Realm::parse(None).unwrap(), Realm::Shared);
        assert_eq!(Realm::parse(Some("SERVER")).unwrap(), Realm::Server);
        assert_eq!(Realm::parse(Some("dev-dependencies")).unwrap(), Realm::Dev);
        assert!(Realm::parse(Some("client")).is_err());

        assert_eq!(Realm::Shared.section(), "dependencies");
        assert_eq!(Realm::Shared.directory(), "Packages");
        assert_eq!(Realm::Server.directory(), "ServerPackages");
        assert_eq!(Realm::Dev.directory(), "DevPackages");
    }

    #[test]
    fn the_index_directory_matches_the_one_wally_unpacks_into() {
        let name = PackageName::parse("roblox/roact").unwrap();
        assert_eq!(name.index_directory("1.4.4"), "roblox_roact@1.4.4");
    }
}
