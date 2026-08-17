use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const DEFAULT_LSP_VERSION: &str = "v0.2.0";
pub const DEFAULT_LSP_REPOSITORY: &str = "Sawhorse-Interactive/luau-lsp-carpenter";
pub const DEFAULT_TYPE_DEFINITIONS_URL: &str =
    "https://luau-lsp.pages.dev/type-definitions/globalTypes.{security_level}.d.luau";
pub const DEFAULT_ROBLOX_DOCS_URL: &str = "https://luau-lsp.pages.dev/api-docs/en-us.json";
pub const DEFAULT_STANDARD_DOCS_URL: &str = "https://luau-lsp.pages.dev/api-docs/luau-en-us.json";

/// The carpenter fork publishes no checksums; these are the digests pinned for `v0.2.0`.
const PINNED_CHECKSUMS: [(&str, &str); 4] = [
    (
        "luau-lsp-win64.zip",
        "28c0a72f282c26d34b376664786857ce60aa4eecbec40e9daf7ea3ef3a193936",
    ),
    (
        "luau-lsp-macos.zip",
        "64c461c215a8965da16e3300e1470c1d5b4e0c2d03eb9d3a539efab4969b8d91",
    ),
    (
        "luau-lsp-linux-x86_64.zip",
        "a468876d6559a77e718dd9eec37a555ce212d7ea04fc3124e92ca0f33f10912c",
    ),
    (
        "luau-lsp-linux-arm64.zip",
        "b919879d703f5ae9cf92e908ae3556188930eac9b07728de8561d74173c1d95c",
    ),
];

pub fn pinned_checksum(version: &str, asset: &str) -> Option<&'static str> {
    if version != DEFAULT_LSP_VERSION {
        return None;
    }
    PINNED_CHECKSUMS
        .iter()
        .find(|(name, _)| *name == asset)
        .map(|(_, digest)| *digest)
}

pub const DEFAULT_SETTINGS_YML: &str = include_str!("../assets/settings.default.yml");
pub const DEFAULT_LOCAL_SETTINGS_YML: &str = include_str!("../assets/settings.local.default.yml");

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    #[serde(deserialize_with = "null_as_default")]
    pub lsp: LspSettings,
    #[serde(deserialize_with = "null_as_default")]
    pub project: ProjectSettings,
    #[serde(deserialize_with = "null_as_default")]
    pub tools: ToolSettings,
}

/// A section whose keys are all commented out parses as null; treat that as "use defaults".
fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LspSettings {
    pub version: String,
    pub repository: String,
    /// Overrides the derived GitHub release asset URL. `{version}` and `{asset}` are substituted.
    pub download_url_template: Option<String>,
    /// Skips acquisition entirely and uses this executable.
    pub binary_path: Option<PathBuf>,
    /// SHA-256 digests keyed by release asset filename; overrides the built-in pins.
    pub checksums: BTreeMap<String, String>,
    pub require_checksum: bool,
    pub platform: LuauPlatform,
    pub roblox_security_level: RobloxSecurityLevel,
    pub type_definitions_url: String,
    pub documentation_url: Option<String>,
    pub extra_args: Vec<String>,
    /// Extra `@alias=path` definition files, appended after the fetched Roblox globals.
    pub definition_files: Vec<String>,
    pub documentation_files: Vec<String>,
    pub base_luaurc: Option<String>,
    pub sourcemap: Option<String>,
    pub watch_sourcemap: bool,
    /// Dotted-key overrides merged into the workspace configuration handed to luau-lsp.
    pub server_settings: Value,
    pub startup_timeout_ms: u64,
    pub request_timeout_ms: u64,
    pub diagnostics_settle_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LuauPlatform {
    #[default]
    Roblox,
    Standard,
}

impl LuauPlatform {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Roblox => "roblox",
            Self::Standard => "standard",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum RobloxSecurityLevel {
    None,
    #[default]
    PluginSecurity,
    LocalUserSecurity,
    RobloxScriptSecurity,
}

impl RobloxSecurityLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::PluginSecurity => "PluginSecurity",
            Self::LocalUserSecurity => "LocalUserSecurity",
            Self::RobloxScriptSecurity => "RobloxScriptSecurity",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectSettings {
    pub ignored_paths: Vec<String>,
    pub respect_gitignore: bool,
    /// Runs without the Luau language server: no acquisition, no process, no LSP-backed tools.
    pub memory_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ToolSettings {
    pub excluded: Vec<String>,
    pub max_answer_chars: usize,
    pub max_listing_entries: usize,
    pub max_pattern_matches: usize,
    pub max_reference_matches: usize,
}

impl Default for LspSettings {
    fn default() -> Self {
        Self {
            version: DEFAULT_LSP_VERSION.to_string(),
            repository: DEFAULT_LSP_REPOSITORY.to_string(),
            download_url_template: None,
            binary_path: None,
            checksums: BTreeMap::new(),
            require_checksum: true,
            platform: LuauPlatform::default(),
            roblox_security_level: RobloxSecurityLevel::default(),
            type_definitions_url: DEFAULT_TYPE_DEFINITIONS_URL.to_string(),
            documentation_url: None,
            extra_args: Vec::new(),
            definition_files: Vec::new(),
            documentation_files: Vec::new(),
            base_luaurc: None,
            sourcemap: Some("sourcemap.json".to_string()),
            watch_sourcemap: true,
            server_settings: Value::Object(Map::new()),
            startup_timeout_ms: 60_000,
            request_timeout_ms: 30_000,
            diagnostics_settle_ms: 1_500,
        }
    }
}

impl Default for ProjectSettings {
    fn default() -> Self {
        Self {
            ignored_paths: Vec::new(),
            respect_gitignore: true,
            memory_only: false,
        }
    }
}

impl Default for ToolSettings {
    fn default() -> Self {
        Self {
            excluded: Vec::new(),
            max_answer_chars: 150_000,
            max_listing_entries: 2_000,
            max_pattern_matches: 200,
            max_reference_matches: 200,
        }
    }
}

impl LspSettings {
    pub fn checksum_for(&self, asset: &str) -> Option<String> {
        self.checksums
            .get(asset)
            .cloned()
            .or_else(|| pinned_checksum(&self.version, asset).map(str::to_string))
    }

    pub fn documentation_url(&self) -> &str {
        if let Some(url) = &self.documentation_url {
            return url;
        }
        match self.platform {
            LuauPlatform::Roblox => DEFAULT_ROBLOX_DOCS_URL,
            LuauPlatform::Standard => DEFAULT_STANDARD_DOCS_URL,
        }
    }

    pub fn type_definitions_url(&self) -> String {
        self.type_definitions_url
            .replace("{security_level}", self.roblox_security_level.as_str())
    }

    pub fn wants_roblox_definitions(&self) -> bool {
        self.platform == LuauPlatform::Roblox
    }

    /// luau-lsp expects VS Code style dotted keys; the first segment is discarded by its parser.
    pub fn workspace_configuration(&self) -> Value {
        let mut dotted = Map::new();
        dotted.insert(
            "luau-lsp.platform.type".to_string(),
            Value::String(self.platform.as_str().to_string()),
        );

        let sourcemap_enabled = self.platform == LuauPlatform::Roblox && self.sourcemap.is_some();
        dotted.insert(
            "luau-lsp.sourcemap.enabled".to_string(),
            Value::Bool(sourcemap_enabled),
        );
        if let Some(sourcemap) = &self.sourcemap {
            dotted.insert(
                "luau-lsp.sourcemap.sourcemapFile".to_string(),
                Value::String(sourcemap.clone()),
            );
        }
        dotted.insert(
            "luau-lsp.sourcemap.autogenerate".to_string(),
            Value::Bool(false),
        );

        if let Value::Object(overrides) = &self.server_settings {
            for (key, value) in overrides {
                dotted.insert(key.clone(), value.clone());
            }
        }

        expand_dotted_keys(&dotted)
    }
}

/// Mirrors luau-lsp's `dottedToClientConfiguration`: split on `.`, drop the first segment.
fn expand_dotted_keys(dotted: &Map<String, Value>) -> Value {
    let mut root = Map::new();
    for (key, value) in dotted {
        let mut segments = key.split('.');
        let _ = segments.next();
        let path: Vec<&str> = segments.collect();
        if path.is_empty() {
            continue;
        }
        insert_nested(&mut root, &path, value.clone());
    }
    Value::Object(root)
}

fn insert_nested(target: &mut Map<String, Value>, path: &[&str], value: Value) {
    let Some((head, rest)) = path.split_first() else {
        return;
    };
    if rest.is_empty() {
        target.insert((*head).to_string(), value);
        return;
    }
    let entry = target
        .entry((*head).to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    if let Value::Object(child) = entry {
        insert_nested(child, rest, value);
    }
}

impl Settings {
    pub fn load(project_settings: &Path, local_settings: &Path) -> Result<Self> {
        let base = read_yaml_value(project_settings)?;
        let overlay = read_yaml_value(local_settings)?;
        let merged = deep_merge(base, overlay);
        serde_json::from_value(merged).context("failed to interpret Biskit settings")
    }
}

fn read_yaml_value(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(Value::Object(Map::new()));
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    // Settings files are heavily commented; capturing comment text would hit the parser's
    // buffered-comment budget without giving Biskit anything it reads.
    let mut options = serde_saphyr::Options::default();
    options.emit_comments = false;
    let parsed: Value = serde_saphyr::from_str_with_options(&raw, options)
        .with_context(|| format!("failed to parse YAML: {}", path.display()))?;
    Ok(match parsed {
        Value::Null => Value::Object(Map::new()),
        other => other,
    })
}

fn deep_merge(base: Value, overlay: Value) -> Value {
    match (base, overlay) {
        (Value::Object(mut base_map), Value::Object(overlay_map)) => {
            for (key, overlay_value) in overlay_map {
                let merged = match base_map.remove(&key) {
                    Some(base_value) => deep_merge(base_value, overlay_value),
                    None => overlay_value,
                };
                base_map.insert(key, merged);
            }
            Value::Object(base_map)
        }
        (_, Value::Null) => Value::Null,
        (base, overlay) => {
            let _ = base;
            overlay
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_settings_override_project_settings() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("settings.yml");
        let local = dir.path().join("settings.local.yml");
        std::fs::write(
            &base,
            "lsp:\n  version: \"0.2.0\"\n  request_timeout_ms: 30000\ntools:\n  excluded: [find_symbol]\n",
        )
        .unwrap();
        std::fs::write(&local, "lsp:\n  version: \"0.3.1\"\n").unwrap();

        let settings = Settings::load(&base, &local).unwrap();
        assert_eq!(settings.lsp.version, "0.3.1");
        assert_eq!(settings.lsp.request_timeout_ms, 30_000);
        assert_eq!(settings.tools.excluded, vec!["find_symbol".to_string()]);
    }

    #[test]
    fn missing_files_yield_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings::load(
            &dir.path().join("absent.yml"),
            &dir.path().join("absent.local.yml"),
        )
        .unwrap();
        assert_eq!(settings.lsp.version, DEFAULT_LSP_VERSION);
        assert!(settings.lsp.require_checksum);
        assert!(!settings.project.memory_only);
    }

    #[test]
    fn memory_only_can_be_enabled_locally() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("settings.yml");
        let local = dir.path().join("settings.local.yml");
        std::fs::write(&base, "project:\n  respect_gitignore: false\n").unwrap();
        std::fs::write(&local, "project:\n  memory_only: true\n").unwrap();

        let settings = Settings::load(&base, &local).unwrap();
        assert!(settings.project.memory_only);
        assert!(!settings.project.respect_gitignore);
    }

    #[test]
    fn workspace_configuration_expands_dotted_keys() {
        let settings = LspSettings {
            server_settings: serde_json::json!({
                "luau-lsp.diagnostics.strictDatamodelTypes": true
            }),
            ..LspSettings::default()
        };

        let configuration = settings.workspace_configuration();
        assert_eq!(configuration["platform"]["type"], "roblox");
        assert_eq!(
            configuration["sourcemap"]["sourcemapFile"],
            "sourcemap.json"
        );
        assert_eq!(configuration["sourcemap"]["enabled"], true);
        assert_eq!(configuration["diagnostics"]["strictDatamodelTypes"], true);
    }

    #[test]
    fn checksums_fall_back_to_built_in_pins() {
        let settings = LspSettings::default();
        assert_eq!(
            settings.checksum_for("luau-lsp-win64.zip").as_deref(),
            Some("28c0a72f282c26d34b376664786857ce60aa4eecbec40e9daf7ea3ef3a193936")
        );

        let mut custom = LspSettings {
            version: "v9.9.9".to_string(),
            ..LspSettings::default()
        };
        assert!(custom.checksum_for("luau-lsp-win64.zip").is_none());
        custom
            .checksums
            .insert("luau-lsp-win64.zip".to_string(), "abc".to_string());
        assert_eq!(
            custom.checksum_for("luau-lsp-win64.zip").as_deref(),
            Some("abc")
        );
    }

    #[test]
    fn long_comment_blocks_parse() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("settings.yml");
        std::fs::write(&base, DEFAULT_SETTINGS_YML).unwrap();
        let local = dir.path().join("settings.local.yml");
        std::fs::write(&local, DEFAULT_LOCAL_SETTINGS_YML).unwrap();

        let settings = Settings::load(&base, &local).unwrap();
        assert_eq!(settings.lsp.version, DEFAULT_LSP_VERSION);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("settings.yml");
        std::fs::write(&base, "lsp:\n  verzion: \"0.2.0\"\n").unwrap();
        assert!(Settings::load(&base, &dir.path().join("absent.yml")).is_err());
    }
}
