# Biskit MCP

Project memory and Luau code intelligence over the Model Context Protocol.

Biskit gives AI coding agents two things for a Roblox Luau project: a durable, curated memory store
that survives between sessions, and symbol-level code intelligence backed by a real Luau language
server instead of text search.

Biskit is read-only with respect to your source code. It never edits, creates, or deletes a source
file. Agents keep using their own native editing tools. The only files Biskit writes are project
memories and its own configuration.

## Why this exists

Biskit is inspired by [Serena](https://github.com/oraios/serena), but is scoped to a single
language. Serena supports around ninety language servers, ships a web dashboard, a tray manager, and
a JetBrains backend. If you only ever write Roblox Luau, nearly all of that is overhead.

Biskit is a single native binary with no runtime dependency, seventeen tools, and no dashboard. It
also supports Rojo sourcemaps, which Serena does not, so DataModel instance types actually resolve.

## Install

Windows, PowerShell:

```powershell
irm https://raw.githubusercontent.com/doctr-oof/biskit-mcp/main/install.ps1 | iex
```

macOS and Linux:

```sh
curl -fsSL https://raw.githubusercontent.com/doctr-oof/biskit-mcp/main/install.sh | sh
```

Both installers verify the download against the SHA256SUMS published with the release and refuse to
install on a mismatch.

Once the binary is in place, both installers offer to register Biskit in a project for you. Every
question defaults to the safe answer, nothing is written until you say yes, and the whole step is
skipped when no terminal is attached, so piping the installer into a provisioning script never
changes files behind your back. See [Per-project setup](#per-project-setup) for what it writes and
how to run the same thing later.

To drive that step from a script, set `BISKIT_SETUP_CLIENTS` (any of `claude`, `cursor`, `vscode`)
and optionally `BISKIT_SETUP_HOOKS=1` before running the installer, or set `BISKIT_NO_SETUP=1` to
turn the whole thing off. The PowerShell installer accepts the same options as parameters when you
run it from a file rather than through `irm`.

Or register it with your agent yourself:

```sh
claude mcp add biskit -- biskit-mcp start
```

For Codex, add it to your MCP server configuration with the command `biskit-mcp` and the argument
`start`.

## Per-project setup

`biskit-mcp start` takes no project argument by design. It walks up from the working directory the
agent launched it in, looking for `.biskit/`, `.git/`, or `default.project.json`. One registration
therefore follows you from project to project, and a registration checked into a repository gives
every contributor the same server.

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
`servers` key instead of `mcpServers`. Agents that only support a global MCP config still behave
per-project, because the root is discovered from the working directory rather than baked into the
registration.

`biskit-mcp setup` writes those files for you, from any project, at any time:

```sh
biskit-mcp setup --client claude --client cursor --client vscode --hooks
```

With no `--client` it configures whichever agents the project already uses, judging by `.claude/`,
`.cursor/`, and `.vscode/`. `--hooks` additionally installs the [session start
hook](#session-start-hook). Add `--project-from-cwd` to pin the registration to that project instead
of letting the server search upwards, `--dry-run` to see the plan without touching anything, and
`--project <path>` to configure a directory you are not standing in.

Merging is conservative. An existing `biskit` entry is never rewritten, unrelated keys and their
order are preserved, a second run changes nothing, and a file that does not parse as JSON is left
alone with an error rather than overwritten.

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
sits, because it is the only marker that says the directory is deliberately a Biskit project. When
no ancestor has one, the nearest `.git/` or `default.project.json` wins instead, so in a fresh
monorepo a package holding its own `default.project.json` resolves to that package rather than to
the repository root. Run `biskit-mcp doctor` to see which root was chosen and how.

## First run

`biskit-mcp start` does not create a `.biskit/` folder on its own. It runs on built-in defaults when
a project has no folder yet, so a registration that follows you everywhere will not leave folders
behind in repositories that do not use Biskit. The folder appears when something asks for it: either
you run `biskit-mcp init`, or the agent saves its first memory.

Either way, you get:

```
your-project/
  .biskit/
    .gitignore
    settings.yml
    settings.local.yml
    cache/
    memories/
```

`settings.yml` is shared with your team and belongs in version control. `settings.local.yml` holds
personal overrides, takes priority, and is gitignored along with `cache/`.

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

Memory:

| Tool | Purpose |
|---|---|
| `list_memories` | Names of every stored memory |
| `read_memory` | Read one memory |
| `create_memory` | Write a new memory |
| `edit_memory` | Regex replace inside a memory |
| `rename_memory` | Rename or move, rewriting `mem:` references |
| `delete_memory` | Delete a memory |

Code intelligence:

| Tool | Purpose |
|---|---|
| `get_symbols_overview` | Symbols defined in a file |
| `find_symbol` | Find symbols by name path |
| `find_declaration` | Where a symbol is declared |
| `find_referencing_symbols` | Every caller or user of a symbol |
| `get_file_diagnostics` | Type errors and warnings for a file |
| `get_symbol_diagnostics` | Diagnostics for a symbol and its callers |
| `restart_language_server` | Restart the Luau language server |

Files and orientation:

| Tool | Purpose |
|---|---|
| `list_dir` | List a directory |
| `find_file` | Find files by glob |
| `search_for_pattern` | Regex search over file contents |
| `initial_instructions` | Usage manual plus the memory index |

There is no `find_implementations`. luau-lsp reports `implementationProvider: false` and has no
handler for `textDocument/implementation`, because Luau has no interface or implementation relation.
Use `find_referencing_symbols` instead.

### Name paths

Symbols are addressed by name path. Dots and slashes are interchangeable, so an agent does not have
to guess:

| Pattern | Matches |
|---|---|
| `update` | any symbol named `update`, at any depth |
| `PlayerService/update` | `update` directly inside `PlayerService` |
| `PlayerService.update` | the same thing |
| `/PlayerService` | only a top-level `PlayerService` |

luau-lsp reports members as flat dotted names such as `PlayerService.addScore`. Biskit splits those
into name path segments, so nesting works the way an agent expects.

### Memories

Memories are plain markdown under `.biskit/memories/`, nestable to any depth. Reference one from
another with a `mem:` pointer in backticks, such as `` `mem:combat/hit-detection` ``.
`rename_memory` rewrites those pointers for you.

## Session start hook

Biskit sets the MCP `instructions` field, which every compliant client surfaces. For Claude Code you
can additionally inject the manual and memory index at session start.

`biskit-mcp setup --hooks` writes this for you. It targets `.claude/settings.local.json`, which is
personal and normally gitignored. Pass `--hooks-target shared` to put it in `.claude/settings.json`
instead, where everyone who clones the repository picks it up. Either way the entry looks like this:

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
| `project.ignored_paths` | empty | Extra gitignore-style exclusions |
| `tools.excluded` | empty | Tool names to hide from the agent |

## Commands

| Command | Purpose |
|---|---|
| `biskit-mcp start` | Run the MCP server over stdio (the default) |
| `biskit-mcp init` | Create `.biskit/` without starting the server |
| `biskit-mcp setup` | Register Biskit in the agent config files a project uses |
| `biskit-mcp doctor` | Verify settings, acquisition, and sourcemap state |
| `biskit-mcp hook session-start` | Emit SessionStart context for Claude Code |

`start`, `doctor`, and `hook session-start` discover the project root by searching upwards, and each
accepts `--project`, `--project-from-cwd`, or `BISKIT_PROJECT` to override that. `init` and `setup`
do not search, because they mean "set up a project here", so they always use the working directory
unless you pass `--project`. On `setup`, `--project-from-cwd` therefore means something different: it
does not choose the directory being configured, it writes the flag into the registration that
command generates.

No command creates a `.biskit/` folder just by running. `init` creates it because that is its job,
and `start` creates it the first time the agent saves a memory. Otherwise Biskit reads what is there
and falls back to defaults.

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
