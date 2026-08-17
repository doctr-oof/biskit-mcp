<p align="center">
  <img src="https://raw.githubusercontent.com/doctr-oof/biskit-mcp/main/.github/logo.png" alt="Biskit MCP" width="200">
</p>
<h3 align="center">
    Biskit MCP
</h3>
<br/>

## What is Biskit?

Warm, airy, and vibe-coded to perfection: Biskit is a project memory management and Roblox Luau code intelligence MCP server made in Rust.

He (yes, it's a boy!) gives your agents the tools to:
- Index, read, write, and modify project-level memories.
- Access symbolic information through [Sawhorse's Luau LSP fork](https://github.com/Sawhorse-Interactive/luau-lsp-carpenter).
- Perform directory, file, and pattern searches without needing to use token-expensive Grep/Glob/Bash tools.
- Resolve Roblox datamodel types in projects that generate a `sourcemap.json`.

He'll never edit or corrupt your source code. He'll never tell your agent how it should use its native tools. He's just a chill lil guy that wants to help your agent get the accurate information it needs.

> [!IMPORTANT]
> Biskit is intended for Roblox Luau projects!
> NEVER install Biskit globally if you work in standard Luau repositories.

## Quick Start

### First-Time Install

To install Biskit for the first time, open a terminal/PowerShell in your project's root directory and paste one of the following:

#### Windows PowerShell (no Git Bash!):

```powershell
irm https://raw.githubusercontent.com/doctr-oof/biskit-mcp/main/install.ps1 | iex
```

#### macOS and Linux (also no Git Bash, use an actual terminal!):

```sh
curl -fsSL https://raw.githubusercontent.com/doctr-oof/biskit-mcp/main/install.sh | sh
```
Biskit will then offer to register Biskit in a project for you. Nothing is written until you confirm, and
the step is skipped when no terminal is attached. See [Per-project setup](#per-project-setup) for
what it writes and how to run the same thing later.

For Codex, add it to your MCP server configuration with the command `biskit-mcp` and the argument
`start`.

> [!TIP]
> I strongly recommend you choose YES when asked about scoping Biskit to the current project!

> [!IMPORTANT]
> Both installers verify the download against the published SHA256SUMS and refuse to install on a mismatch.

### Upgrading

If you've already installed Biskit and want to upgrade, simply open a terminal and run the following:

```sh
biskit-mcp upgrade
```

That replaces the running executable with the latest release and nothing else. It never touches
`.mcp.json`, `.claude/`, or `.biskit/`, so your existing project registrations keep working. Pass
`--tag v0.1.4` to install a specific release, including an older one. The download is verified
against SHA256SUMS and a mismatch aborts before anything is replaced.

## Per-project setup

`biskit-mcp start` takes no project argument. It walks up from the working directory it was launched
in, looking for `.biskit/`, `.git/`, or `default.project.json`, so one registration follows you from
project to project.

If you want that registration in the repository rather than in your personal agent config, add
`.mcp.json` at the project root. Claude Code reads it automatically:

```json
{
  "mcpServers": {
    "biskit": {
      "type": "stdio",
      "command": "biskit-mcp",
      "args": ["start"]
    }
  }
}
```

Cursor uses the same shape at `.cursor/mcp.json`. VS Code uses `.vscode/mcp.json` with a top-level
`servers` key instead of `mcpServers`.

`biskit-mcp setup` writes those files for you:

```sh
biskit-mcp setup --client claude --client cursor --client vscode --hooks
```

With no `--client` it configures whichever agents the project already uses, judging by `.claude/`,
`.cursor/`, and `.vscode/`. `--hooks` additionally installs the [session start
hook](#session-start-hook). Add `--project-from-cwd` to pin the registration to that project instead
of letting the server search upwards, `--dry-run` to see the plan without touching anything, and
`--project <path>` to configure another directory.

Merging is idempotent. An existing `biskit` entry, unrelated keys, and key order are all preserved,
and a file that does not parse as JSON is left alone with an error rather than overwritten.

On Windows, `command` must resolve on `PATH`. The installer adds `%LOCALAPPDATA%\biskit\bin` to your
user `PATH`, so restart the terminal, or the editor, after installing.

If discovery is wrong for your layout, override it:

| Override | Effect |
|---|---|
| `--project <path>` | Use this root, no searching |
| `BISKIT_PROJECT` | Same, by environment variable |
| `--project-from-cwd` | Use the working directory as-is, no searching |

Precedence is `--project`, then `BISKIT_PROJECT`, then discovery. When nothing matches, Biskit exits
with an error rather than guessing.

A `.biskit/` folder wins over `.git/` and `default.project.json` no matter how far up the tree it
sits. When no ancestor has one, the nearest `.git/` or `default.project.json` wins instead. Run
`biskit-mcp doctor` to see which root was chosen and how.

## Session start hook

Biskit sets the MCP `instructions` field, which every compliant client surfaces. For Claude Code you
can additionally inject the manual and memory index at session start.

`biskit-mcp setup --hooks` writes this to `.claude/settings.local.json`, which is personal and
normally gitignored. Pass `--hooks-target shared` to put it in `.claude/settings.json` instead,
where everyone who clones the repository picks it up. Either way the entry looks like this:

```json
{
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          { "type": "command", "command": "biskit-mcp hook session-start" }
        ]
      }
    ]
  }
}
```

## First run

`biskit-mcp start` does not create a `.biskit/` folder on its own. It runs on built-in defaults
until something asks for the folder: either you run `biskit-mcp init`, or the agent saves its first
memory.

Either way, you get:

```
your-project/
  .biskit/
    .gitignore
    settings.yml
    settings.local.yml
    memories/
```

`settings.yml` is shared with your team and belongs in version control. `settings.local.yml` holds
personal overrides, takes priority, and is gitignored.

Biskit downloads the pinned [luau-lsp-carpenter](https://github.com/Sawhorse-Interactive/luau-lsp-carpenter)
release on first run, verifies its SHA-256 digest, and caches it per version in your user cache
directory. The carpenter fork does not publish checksums, so Biskit ships pinned digests for the
default version. If you pin a different version, supply digests under `lsp.checksums` or explicitly
set `lsp.require_checksum: false`.

Check everything resolved correctly:

```sh
biskit-mcp doctor
```

## Sourcemaps

For Roblox projects, generate a sourcemap so `script.Parent.Thing` and DataModel instance types
resolve:

```sh
rojo sourcemap --include-non-scripts --watch default.project.json --output sourcemap.json
```

Biskit watches that file and tells the language server when it changes, so a regenerated sourcemap
takes effect without a restart. Set `lsp.watch_sourcemap: false` to disable the watcher, or
`lsp.sourcemap: null` to turn sourcemap loading off entirely.

## Tools

These are all of the tools Biskit provides your agent. You can exclude them via the `tools.excluded` configuration.

- **Memory**: `list_memories`, `read_memory`, `create_memory`, `edit_memory`, `rename_memory`,
  `delete_memory`.
- **Code intelligence**: `get_symbols_overview`, `find_symbol`, `find_declaration`,
  `find_referencing_symbols`, `get_file_diagnostics`, `get_symbol_diagnostics`,
  `restart_language_server`.
- **Files and orientation**: `list_dir`, `find_file`, `search_for_pattern`, `initial_instructions`.

### Memories

Memories are plain markdown under `.biskit/memories/`, nestable to any depth. Reference one from
another with a `mem:` pointer in backticks, such as `` `mem:combat/hit-detection` ``.
`rename_memory` rewrites those pointers for you.

## Configuration

Every option is documented inline in the generated `.biskit/settings.yml`. The ones worth knowing:

| Key | Default | Purpose |
|---|---|---|
| `lsp.version` | `v0.2.0` | luau-lsp release tag |
| `lsp.repository` | `Sawhorse-Interactive/luau-lsp-carpenter` | Where the release comes from |
| `lsp.binary_path` | unset | Use an existing binary and skip downloading |
| `lsp.checksums` | built-in pins | SHA-256 digests by asset filename |
| `lsp.platform` | `roblox` | `roblox` or `standard` |
| `lsp.roblox_security_level` | `PluginSecurity` | Which Roblox API dump to load |
| `lsp.sourcemap` | `sourcemap.json` | Rojo sourcemap path, or null to disable |
| `lsp.server_settings` | empty | Raw luau-lsp settings in VS Code dotted-key form |
| `project.ignored_paths` | empty | Extra gitignore-style exclusions, applied to every project walk |
| `project.memory_only` | `false` | Run without the language server, see below |
| `tools.excluded` | empty | Tool names to hide from the agent |
| `tools.max_answer_chars` | `150000` | Ceiling on one tool result, 0 to lift it |
| `tools.max_reference_matches` | `200` | Cap on references from `find_referencing_symbols` |

A structured result over `max_answer_chars` is refused with a message naming what to narrow. A text
result, such as a memory, is cut instead and says how much was withheld.

### Memory-only mode

Set `project.memory_only: true` to run Biskit as a memory, file, and search server with no Luau code
intelligence at all:

- luau-lsp is never downloaded and no language server process starts.
- The seven code-intelligence tools are not registered, so the agent never sees them.
- The MCP `instructions` field and `initial_instructions` both say the mode is on and name the tools
  that are unavailable.
- `biskit-mcp doctor` reports the mode and skips every LSP check.

Memory, `list_dir`, `find_file`, and `search_for_pattern` keep working. Put it in
`settings.local.yml` to turn it on for yourself only.

## Commands

| Command | Purpose |
|---|---|
| `biskit-mcp start` | Run the MCP server over stdio (the default) |
| `biskit-mcp init` | Create `.biskit/` without starting the server |
| `biskit-mcp setup` | Register Biskit in the agent config files a project uses |
| `biskit-mcp doctor` | Verify settings, acquisition, and sourcemap state |
| `biskit-mcp upgrade` | Replace this executable with a published release |
| `biskit-mcp hook session-start` | Emit SessionStart context for Claude Code |

`start`, `doctor`, and `hook session-start` discover the project root by searching upwards. `init`
and `setup` always use the working directory unless you pass `--project`. On `setup`,
`--project-from-cwd` means something different: it does not choose the directory being configured,
it writes that flag into the registration the command generates. `upgrade` has no project at all.

Set `BISKIT_LOG` to control logging, for example `BISKIT_LOG=biskit=debug`. Logs always go to
stderr, because stdout carries the JSON-RPC stream.

## Building from source

Requires Rust 1.88 or newer.

```sh
cargo build --release
cargo test
```

## Security

Downloads are restricted to an explicit host allowlist, every redirect hop is re-checked, plain HTTP
is refused, and archive entries are rejected if they attempt path traversal. The language server
binary is verified against a pinned SHA-256 digest before it is ever executed. Dependency advisories
are checked in CI with `cargo audit`.

## License

MIT
