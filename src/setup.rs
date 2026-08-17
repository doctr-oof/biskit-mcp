use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use serde_json::{Map, Value, json};

pub const SERVER_NAME: &str = "biskit";
pub const DEFAULT_COMMAND: &str = "biskit-mcp";

const CLAUDE_DIR: &str = ".claude";
const SETTINGS_FILE: &str = "settings.json";
const LOCAL_SETTINGS_FILE: &str = "settings.local.json";
const ENABLED_SERVERS_KEY: &str = "enabledMcpjsonServers";
const SESSION_START_EVENT: &str = "SessionStart";

/// Agent configuration file that can hold an MCP server registration.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum Client {
    /// `.mcp.json` at the project root.
    Claude,
    /// `.cursor/mcp.json`.
    Cursor,
    /// `.vscode/mcp.json`.
    #[value(name = "vscode", alias = "vs-code")]
    VsCode,
}

impl Client {
    pub const ALL: [Client; 3] = [Client::Claude, Client::Cursor, Client::VsCode];

    pub fn config_path(self, root: &Path) -> PathBuf {
        match self {
            Client::Claude => root.join(".mcp.json"),
            Client::Cursor => root.join(".cursor").join("mcp.json"),
            Client::VsCode => root.join(".vscode").join("mcp.json"),
        }
    }

    /// VS Code names the map `servers`; everything else uses `mcpServers`.
    pub fn servers_key(self) -> &'static str {
        match self {
            Client::Claude | Client::Cursor => "mcpServers",
            Client::VsCode => "servers",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Client::Claude => "Claude Code",
            Client::Cursor => "Cursor",
            Client::VsCode => "VS Code",
        }
    }

    fn is_present(self, root: &Path) -> bool {
        match self {
            Client::Claude => root.join(CLAUDE_DIR).exists() || root.join(".mcp.json").exists(),
            Client::Cursor => root.join(".cursor").exists(),
            Client::VsCode => root.join(".vscode").exists(),
        }
    }
}

/// Which Claude Code settings file the SessionStart hook is written to.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum HooksTarget {
    /// `.claude/settings.local.json`, personal and normally gitignored.
    Local,
    /// `.claude/settings.json`, shared with everyone who clones the repository.
    Shared,
}

impl HooksTarget {
    fn path(self, root: &Path) -> PathBuf {
        let dir = root.join(CLAUDE_DIR);
        match self {
            HooksTarget::Local => dir.join(LOCAL_SETTINGS_FILE),
            HooksTarget::Shared => dir.join(SETTINGS_FILE),
        }
    }
}

pub struct Plan {
    pub clients: Vec<Client>,
    pub hooks: Option<HooksTarget>,
    pub command: String,
    pub project_from_cwd: bool,
    pub dry_run: bool,
}

#[derive(Debug)]
pub struct Step {
    pub path: PathBuf,
    pub created: bool,
    pub changed: bool,
    pub notes: Vec<String>,
}

/// Clients whose configuration directory already exists in `root`.
pub fn detect(root: &Path) -> Vec<Client> {
    Client::ALL
        .into_iter()
        .filter(|client| client.is_present(root))
        .collect()
}

pub fn run(root: &Path, plan: &Plan) -> Result<Vec<Step>> {
    let mut steps = Vec::new();
    for client in &plan.clients {
        steps.push(configure_client(root, *client, plan)?);
    }
    if let Some(target) = plan.hooks {
        steps.push(configure_hooks(root, target, plan)?);
    }
    Ok(steps)
}

fn configure_client(root: &Path, client: Client, plan: &Plan) -> Result<Step> {
    let path = client.config_path(root);
    let existing = load_object(&path)?;
    let created = existing.is_none();
    let mut document = existing.unwrap_or_default();

    let key = client.servers_key();
    if !document.contains_key(key) {
        document.insert(key.to_string(), Value::Object(Map::new()));
    }
    let Some(Value::Object(servers)) = document.get_mut(key) else {
        bail!(
            "{} holds a non-object \"{key}\" entry; refusing to modify it",
            path.display()
        );
    };

    let mut notes = Vec::new();
    let changed = if servers.contains_key(SERVER_NAME) {
        notes.push(format!(
            "\"{SERVER_NAME}\" already registered under \"{key}\", left untouched"
        ));
        false
    } else {
        servers.insert(
            SERVER_NAME.to_string(),
            server_entry(&plan.command, plan.project_from_cwd),
        );
        notes.push(format!("registered \"{SERVER_NAME}\" under \"{key}\""));
        true
    };

    if changed && !plan.dry_run {
        write_object(&path, &document)?;
    }
    Ok(Step {
        path,
        created: created && changed,
        changed,
        notes,
    })
}

fn configure_hooks(root: &Path, target: HooksTarget, plan: &Plan) -> Result<Step> {
    let path = target.path(root);
    let existing = load_object(&path)?;
    let created = existing.is_none();
    let mut document = existing.unwrap_or_default();
    let mut notes = Vec::new();
    let mut changed = false;

    if !document.contains_key("hooks") {
        document.insert("hooks".to_string(), Value::Object(Map::new()));
    }
    let Some(Value::Object(hooks)) = document.get_mut("hooks") else {
        bail!(
            "{} holds a non-object \"hooks\" entry; refusing to modify it",
            path.display()
        );
    };

    if !hooks.contains_key(SESSION_START_EVENT) {
        hooks.insert(SESSION_START_EVENT.to_string(), Value::Array(Vec::new()));
    }
    let Some(Value::Array(blocks)) = hooks.get_mut(SESSION_START_EVENT) else {
        bail!(
            "{} holds a non-array \"{SESSION_START_EVENT}\" entry; refusing to modify it",
            path.display()
        );
    };

    if blocks.iter().any(holds_biskit_session_start) {
        notes.push(format!(
            "a Biskit {SESSION_START_EVENT} hook is already present, left untouched"
        ));
    } else {
        let command = hook_command(&plan.command, plan.project_from_cwd);
        blocks.push(json!({
            "hooks": [ { "type": "command", "command": command } ]
        }));
        notes.push(format!("added the {SESSION_START_EVENT} hook"));
        changed = true;
    }

    if plan.clients.contains(&Client::Claude) {
        if !document.contains_key(ENABLED_SERVERS_KEY) {
            document.insert(ENABLED_SERVERS_KEY.to_string(), Value::Array(Vec::new()));
        }
        let Some(Value::Array(enabled)) = document.get_mut(ENABLED_SERVERS_KEY) else {
            bail!(
                "{} holds a non-array \"{ENABLED_SERVERS_KEY}\" entry; refusing to modify it",
                path.display()
            );
        };
        if enabled.iter().any(|value| value == SERVER_NAME) {
            notes.push(format!(
                "\"{SERVER_NAME}\" already in {ENABLED_SERVERS_KEY}"
            ));
        } else {
            enabled.push(Value::from(SERVER_NAME));
            notes.push(format!(
                "approved \"{SERVER_NAME}\" in {ENABLED_SERVERS_KEY}"
            ));
            changed = true;
        }
    }

    if changed && !plan.dry_run {
        write_object(&path, &document)?;
    }
    Ok(Step {
        path,
        created: created && changed,
        changed,
        notes,
    })
}

fn server_entry(command: &str, project_from_cwd: bool) -> Value {
    let mut args = vec![Value::from("start")];
    if project_from_cwd {
        args.push(Value::from("--project-from-cwd"));
    }
    json!({ "type": "stdio", "command": command, "args": args })
}

fn hook_command(command: &str, project_from_cwd: bool) -> String {
    let head = if command.contains(char::is_whitespace) {
        format!("\"{command}\"")
    } else {
        command.to_string()
    };
    let suffix = if project_from_cwd {
        " --project-from-cwd"
    } else {
        ""
    };
    format!("{head} hook session-start{suffix}")
}

/// Matches any Biskit session-start hook, so re-running with different flags never duplicates it.
fn holds_biskit_session_start(block: &Value) -> bool {
    let Some(entries) = block.get("hooks").and_then(Value::as_array) else {
        return false;
    };
    entries.iter().any(|entry| {
        entry
            .get("command")
            .and_then(Value::as_str)
            .is_some_and(|command| {
                command.contains(DEFAULT_COMMAND) && command.contains("hook session-start")
            })
    })
}

fn load_object(path: &Path) -> Result<Option<Map<String, Value>>> {
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("could not read {}", path.display()))?;
    if text.trim().is_empty() {
        return Ok(Some(Map::new()));
    }
    let value: Value = serde_json::from_str(&text).with_context(|| {
        format!(
            "{} is not valid JSON; refusing to overwrite it",
            path.display()
        )
    })?;
    match value {
        Value::Object(map) => Ok(Some(map)),
        _ => bail!(
            "{} does not hold a JSON object; refusing to overwrite it",
            path.display()
        ),
    }
}

fn write_object(path: &Path, document: &Map<String, Value>) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    let mut text = serde_json::to_string_pretty(&Value::Object(document.clone()))?;
    text.push('\n');
    std::fs::write(path, text).with_context(|| format!("could not write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(clients: Vec<Client>, hooks: Option<HooksTarget>, project_from_cwd: bool) -> Plan {
        Plan {
            clients,
            hooks,
            command: DEFAULT_COMMAND.to_string(),
            project_from_cwd,
            dry_run: false,
        }
    }

    fn read(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn creates_mcp_json_for_a_bare_project() {
        let dir = tempfile::tempdir().unwrap();
        let steps = run(dir.path(), &plan(vec![Client::Claude], None, false)).unwrap();

        assert!(steps[0].created);
        let document = read(&dir.path().join(".mcp.json"));
        assert_eq!(
            document["mcpServers"]["biskit"],
            json!({ "type": "stdio", "command": "biskit-mcp", "args": ["start"] })
        );
    }

    #[test]
    fn project_from_cwd_reaches_every_generated_registration() {
        let dir = tempfile::tempdir().unwrap();
        run(
            dir.path(),
            &plan(Client::ALL.to_vec(), Some(HooksTarget::Local), true),
        )
        .unwrap();

        for client in Client::ALL {
            let document = read(&client.config_path(dir.path()));
            assert_eq!(
                document[client.servers_key()]["biskit"]["args"],
                json!(["start", "--project-from-cwd"]),
                "{} did not receive the flag",
                client.label()
            );
        }

        let settings = read(&dir.path().join(CLAUDE_DIR).join(LOCAL_SETTINGS_FILE));
        assert_eq!(
            settings["hooks"]["SessionStart"][0]["hooks"][0]["command"],
            json!("biskit-mcp hook session-start --project-from-cwd")
        );
    }

    #[test]
    fn vscode_uses_the_servers_key() {
        let dir = tempfile::tempdir().unwrap();
        run(dir.path(), &plan(vec![Client::VsCode], None, false)).unwrap();

        let document = read(&dir.path().join(".vscode").join("mcp.json"));
        assert!(document.get("mcpServers").is_none());
        assert!(document["servers"]["biskit"].is_object());
    }

    #[test]
    fn merging_preserves_unrelated_keys_and_their_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
        std::fs::write(
            &path,
            r#"{"zebra": 1, "mcpServers": {"other": {"command": "other"}}, "alpha": 2}"#,
        )
        .unwrap();

        run(dir.path(), &plan(vec![Client::Claude], None, false)).unwrap();

        let document = read(&path);
        let keys: Vec<&String> = document.as_object().unwrap().keys().collect();
        assert_eq!(keys, vec!["zebra", "mcpServers", "alpha"]);
        assert!(document["mcpServers"]["other"].is_object());
        assert!(document["mcpServers"]["biskit"].is_object());
    }

    #[test]
    fn an_existing_registration_is_left_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
        std::fs::write(&path, r#"{"mcpServers": {"biskit": {"command": "mine"}}}"#).unwrap();

        let steps = run(dir.path(), &plan(vec![Client::Claude], None, true)).unwrap();

        assert!(!steps[0].changed);
        assert_eq!(
            read(&path)["mcpServers"]["biskit"]["command"],
            json!("mine")
        );
    }

    #[test]
    fn a_second_run_does_not_duplicate_the_hook() {
        let dir = tempfile::tempdir().unwrap();
        let target = Some(HooksTarget::Local);
        run(dir.path(), &plan(vec![Client::Claude], target, false)).unwrap();
        run(dir.path(), &plan(vec![Client::Claude], target, true)).unwrap();

        let settings = read(&dir.path().join(CLAUDE_DIR).join(LOCAL_SETTINGS_FILE));
        assert_eq!(
            settings["hooks"]["SessionStart"].as_array().unwrap().len(),
            1
        );
        assert_eq!(
            settings[ENABLED_SERVERS_KEY],
            json!(["biskit"]),
            "the server was approved twice"
        );
    }

    #[test]
    fn existing_hooks_for_other_tools_survive() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CLAUDE_DIR).join(LOCAL_SETTINGS_FILE);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": "other"}]}]}}"#,
        )
        .unwrap();

        run(
            dir.path(),
            &plan(vec![Client::Claude], Some(HooksTarget::Local), false),
        )
        .unwrap();

        let blocks = read(&path)["hooks"]["SessionStart"]
            .as_array()
            .unwrap()
            .len();
        assert_eq!(blocks, 2);
    }

    #[test]
    fn malformed_json_is_refused_rather_than_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".mcp.json");
        std::fs::write(&path, "{ not json").unwrap();

        let error = run(dir.path(), &plan(vec![Client::Claude], None, false)).unwrap_err();

        assert!(error.to_string().contains("refusing to overwrite"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
    }

    #[test]
    fn a_dry_run_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut dry = plan(Client::ALL.to_vec(), Some(HooksTarget::Local), true);
        dry.dry_run = true;

        let steps = run(dir.path(), &dry).unwrap();

        assert!(steps.iter().all(|step| step.changed));
        assert!(!dir.path().join(".mcp.json").exists());
        assert!(!dir.path().join(CLAUDE_DIR).exists());
    }

    #[test]
    fn detection_only_reports_clients_that_are_already_set_up() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".cursor")).unwrap();

        assert_eq!(detect(dir.path()), vec![Client::Cursor]);
    }
}
