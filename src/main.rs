use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use rmcp::ServiceExt;
use rmcp::transport::stdio;
use tracing_subscriber::EnvFilter;

use biskit_mcp::config::Settings;
use biskit_mcp::memory::MemoryStore;
use biskit_mcp::project::Project;
use biskit_mcp::server::Biskit;
use biskit_mcp::setup::{Client, HooksTarget};
use biskit_mcp::{lsp, project, prompts, setup, upgrade};

const PROJECT_ENV: &str = "BISKIT_PROJECT";

/// `root_source` for a root that was accepted without any marker vouching for it.
const CWD_ROOT_SOURCE: &str = "working directory";

#[derive(Parser)]
#[command(
    name = "biskit-mcp",
    version,
    about = "Project memory and Luau code intelligence over MCP"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    Start {
        #[arg(long)]
        project: Option<PathBuf>,
        #[arg(long, conflicts_with = "project")]
        project_from_cwd: bool,
    },
    Init {
        #[arg(long)]
        project: Option<PathBuf>,
    },
    Doctor {
        #[arg(long)]
        project: Option<PathBuf>,
        #[arg(long, conflicts_with = "project")]
        project_from_cwd: bool,
    },
    Setup {
        #[arg(long)]
        project: Option<PathBuf>,
        #[arg(long = "client", value_enum)]
        clients: Vec<Client>,
        #[arg(long)]
        hooks: bool,
        #[arg(long, value_enum, default_value = "local")]
        hooks_target: HooksTarget,
        #[arg(long)]
        project_from_cwd: bool,
        #[arg(long, default_value = setup::DEFAULT_COMMAND)]
        command: String,
        #[arg(long)]
        dry_run: bool,
    },
    Upgrade {
        #[arg(long)]
        tag: Option<String>,
    },
    Cache {
        #[command(subcommand)]
        which: CacheCommand,
    },
    Hook {
        #[command(subcommand)]
        which: HookCommand,
    },
}

#[derive(Subcommand)]
enum CacheCommand {
    Clear {
        #[arg(long)]
        project: Option<PathBuf>,
        #[arg(long, conflicts_with = "project")]
        project_from_cwd: bool,
    },
}

#[derive(Subcommand)]
enum HookCommand {
    SessionStart {
        #[arg(long)]
        project: Option<PathBuf>,
        #[arg(long, conflicts_with = "project")]
        project_from_cwd: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command.unwrap_or(Command::Start {
        project: None,
        project_from_cwd: false,
    }) {
        Command::Start {
            project,
            project_from_cwd,
        } => run_server(RootRequest::new(project, project_from_cwd)),
        Command::Init { project } => run_init(RootRequest::from_cwd(project)),
        Command::Setup {
            project,
            clients,
            hooks,
            hooks_target,
            project_from_cwd,
            command,
            dry_run,
        } => run_setup(
            RootRequest::from_cwd(project),
            clients,
            hooks.then_some(hooks_target),
            project_from_cwd,
            command,
            dry_run,
        ),
        Command::Doctor {
            project,
            project_from_cwd,
        } => run_doctor(RootRequest::new(project, project_from_cwd)),
        Command::Upgrade { tag } => upgrade::run(tag),
        Command::Cache {
            which:
                CacheCommand::Clear {
                    project,
                    project_from_cwd,
                },
        } => run_cache_clear(RootRequest::new(project, project_from_cwd)),
        Command::Hook {
            which:
                HookCommand::SessionStart {
                    project,
                    project_from_cwd,
                },
        } => run_session_start_hook(RootRequest::new(project, project_from_cwd)),
    }
}

fn install_tracing() {
    let filter = EnvFilter::try_from_env("BISKIT_LOG")
        .unwrap_or_else(|_| EnvFilter::new("biskit=info,warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
}

struct RootRequest {
    explicit: Option<PathBuf>,
    discover: bool,
}

impl RootRequest {
    fn new(explicit: Option<PathBuf>, project_from_cwd: bool) -> Self {
        Self {
            explicit,
            discover: !project_from_cwd,
        }
    }

    fn from_cwd(explicit: Option<PathBuf>) -> Self {
        Self {
            explicit,
            discover: false,
        }
    }
}

struct Opened {
    project: Project,
    settings: Settings,
    root_source: &'static str,
}

fn resolve_root(request: RootRequest) -> Result<(PathBuf, &'static str)> {
    if let Some(path) = request.explicit {
        return Ok((path, "--project"));
    }
    if let Some(value) = std::env::var_os(PROJECT_ENV).filter(|value| !value.is_empty()) {
        return Ok((PathBuf::from(value), PROJECT_ENV));
    }

    let cwd = std::env::current_dir().context("could not determine the current directory")?;
    if !request.discover {
        return Ok((cwd, CWD_ROOT_SOURCE));
    }

    match project::discover_root(&cwd) {
        Some(path) => Ok((path, "search upwards from the working directory")),
        None => bail!(
            "no project root found at or above {}\n\
             looked for {}\n\
             pass --project <path>, set {PROJECT_ENV}, or pass --project-from-cwd to accept the \
             working directory as the root",
            cwd.display(),
            project::ROOT_MARKERS.join(", ")
        ),
    }
}

fn open_project(request: RootRequest) -> Result<Opened> {
    let (root, root_source) = resolve_root(request)?;
    let project = Project::open(root)?;
    let settings = Settings::load(&project.settings_path(), &project.local_settings_path())?;
    Ok(Opened {
        project,
        settings,
        root_source,
    })
}

fn run_server(request: RootRequest) -> Result<()> {
    install_tracing();
    let Opened {
        project,
        settings,
        root_source,
    } = open_project(request)?;
    tracing::info!(
        target: "biskit",
        "serving project {} (root from {root_source})",
        project.root().display()
    );
    bootstrap_on_startup(&project, root_source);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    runtime.block_on(async move {
        let biskit = Biskit::new(project, settings, root_source);
        biskit.warm_up();
        let service = biskit.clone().serve(stdio()).await?;
        let outcome = service.waiting().await;
        biskit.shutdown().await;
        outcome?;
        anyhow::Ok(())
    })
}

/// Lays `.biskit` down at startup so its settings can be edited before the first tool call.
///
/// A root that no marker vouched for is left alone: `.biskit` is the highest-priority entry in
/// [`project::ROOT_MARKERS`], so planting one in a stray working directory would outrank the real
/// project root for every later discovery. A project Biskit cannot write to still serves every
/// read-only tool, so a failure here is reported rather than fatal.
fn bootstrap_on_startup(project: &Project, root_source: &str) {
    let biskit = project.biskit_dir();
    if root_source == CWD_ROOT_SOURCE {
        tracing::debug!(
            target: "biskit",
            "root came from the {CWD_ROOT_SOURCE}, so {} was left uncreated",
            biskit.display()
        );
        return;
    }

    let report = match project.bootstrap() {
        Ok(report) => report,
        Err(error) => {
            tracing::warn!(
                target: "biskit",
                "could not initialise {}: {error:#}",
                biskit.display()
            );
            return;
        }
    };

    let created: Vec<&str> = [
        (report.created_biskit_dir, project::BISKIT_DIR),
        (report.created_gitignore, ".gitignore"),
        (report.created_settings, project::SETTINGS_FILE),
        (report.created_local_settings, project::LOCAL_SETTINGS_FILE),
    ]
    .into_iter()
    .filter_map(|(created, name)| created.then_some(name))
    .collect();

    if !created.is_empty() {
        tracing::info!(
            target: "biskit",
            "initialised {} ({})",
            biskit.display(),
            created.join(", ")
        );
    }
}

fn run_init(request: RootRequest) -> Result<()> {
    let opened = open_project(request)?;
    opened.project.bootstrap()?;
    println!(
        "Biskit initialised at {}",
        opened.project.biskit_dir().display()
    );
    Ok(())
}

fn run_setup(
    request: RootRequest,
    clients: Vec<Client>,
    hooks: Option<HooksTarget>,
    project_from_cwd: bool,
    command: String,
    dry_run: bool,
) -> Result<()> {
    let (requested, _) = resolve_root(request)?;
    let project = Project::open(&requested)?;
    let root = project.root();

    let mut selected: Vec<Client> = Vec::new();
    for client in clients {
        if !selected.contains(&client) {
            selected.push(client);
        }
    }

    let detected = selected.is_empty() && hooks.is_none();
    if detected {
        selected = setup::detect(root);
        if selected.is_empty() {
            bail!(
                "no agent configuration found in {}\n\
                 pass --client claude, --client cursor, or --client vscode to choose one, \
                 or --hooks to only add the Claude Code session hook",
                root.display()
            );
        }
    }

    let plan = setup::Plan {
        clients: selected,
        hooks,
        command,
        project_from_cwd,
        dry_run,
    };

    println!("project root      {}", root.display());
    if detected {
        let labels: Vec<&str> = plan.clients.iter().map(|client| client.label()).collect();
        println!("detected agents   {}", labels.join(", "));
    }

    let steps = setup::run(root, &plan)?;
    for step in &steps {
        let shown = step.path.strip_prefix(root).unwrap_or(&step.path);
        let state = if !step.changed {
            " (unchanged)"
        } else if step.created {
            " (created)"
        } else {
            ""
        };
        println!("\n{}{state}", project::normalize_separators(shown));
        for note in &step.notes {
            println!("  {note}");
        }
    }

    if dry_run {
        println!("\nDry run: nothing was written.");
    }
    Ok(())
}

fn run_doctor(request: RootRequest) -> Result<()> {
    install_tracing();
    let Opened {
        project,
        settings,
        root_source,
    } = open_project(request)?;

    println!("project root      {}", project.root().display());
    println!("root resolved by  {root_source}");
    if !project.settings_path().exists() {
        println!("settings          defaults (run `biskit-mcp init` to write settings.yml)");
    }

    if settings.project.memory_only {
        println!("mode              memory-only (no language server, no LSP-backed tools)");
        let memories = MemoryStore::new(project).list()?;
        println!("memories          {}", memories.len());
        return Ok(());
    }

    println!("lsp version       {}", settings.lsp.version);
    println!("lsp repository    {}", settings.lsp.repository);
    println!("platform          {}", settings.lsp.platform.as_str());
    println!(
        "security level    {}",
        settings.lsp.roblox_security_level.as_str()
    );

    let asset = lsp::acquire::asset_name()?;
    println!("release asset     {asset}");
    match settings.lsp.checksum_for(asset) {
        Some(digest) => println!("expected sha256   {digest}"),
        None => println!("expected sha256   (none known)"),
    }

    let install = lsp::acquire::ensure_installed(&settings.lsp)?;
    println!("binary            {}", install.binary.display());
    for (alias, path) in &install.definition_files {
        println!("definitions       {alias} -> {}", path.display());
    }
    for path in &install.documentation_files {
        println!("documentation     {}", path.display());
    }

    match &settings.lsp.sourcemap {
        Some(relative) => {
            let resolved = project.resolve(relative)?;
            let state = if resolved.is_file() {
                "present"
            } else {
                "missing"
            };
            println!("sourcemap         {relative} ({state})");
        }
        None => println!("sourcemap         disabled"),
    }

    let memories = MemoryStore::new(project).list()?;
    println!("memories          {}", memories.len());
    Ok(())
}

fn run_cache_clear(request: RootRequest) -> Result<()> {
    let (root, _) = resolve_root(request)?;
    let project = Project::open(root)?;
    let directory = lsp::cache::cache_dir(&project);

    match lsp::cache::SymbolCache::clear(&project)? {
        true => println!("removed {}", directory.display()),
        false => println!("nothing cached at {}", directory.display()),
    }
    Ok(())
}

fn run_session_start_hook(request: RootRequest) -> Result<()> {
    let opened = open_project(request)?;
    let memory_only = opened.settings.project.memory_only;
    let memories = MemoryStore::new(opened.project).list()?;

    let payload = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": prompts::initial_instructions(&memories, memory_only),
        }
    });
    println!("{payload}");
    Ok(())
}
