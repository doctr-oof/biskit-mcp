# Biskit MCP

Project memory and Luau code intelligence over the Model Context Protocol.

Biskit gives AI coding agents two things for a Roblox Luau project: a durable, curated memory store
that survives between sessions, and symbol-level code intelligence backed by a real Luau language
server instead of text search.

Biskit never edits, creates, or deletes a source file. Agents keep using their own native editing
tools. The only files Biskit writes are project memories and its own configuration.

## Why this exists

Biskit is inspired by [Serena](https://github.com/oraios/serena), but scoped to a single language.
Where Serena supports around ninety language servers and ships a dashboard, a tray manager, and a
JetBrains backend, Biskit is a single native binary with no runtime dependency, seventeen tools, and
Rojo sourcemap support, so DataModel instance types actually resolve.

## Install

Windows, PowerShell:

```powershell
irm https://raw.githubusercontent.com/doctr-oof/biskit-mcp/main/install.ps1 | iex
```

macOS and Linux:

```sh
curl -fsSL https://raw.githubusercontent.com/doctr-oof/biskit-mcp/main/install.sh | sh
```

On Windows use the PowerShell installer even if you normally work in Git Bash or the VS Code
terminal. The Windows build ships as a .zip and the install registers your user PATH, neither of
which the shell installer does. WSL is the exception, since that is a real Linux target.

Both installers verify the download against the published SHA256SUMS and refuse to install on a
mismatch.

Both then offer to register Biskit in a project for you. Nothing is written until you confirm, and
the step is skipped when no terminal is attached. See [Per-project setup](#per-project-setup) for
what it writes and how to run the same thing later.

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

## Updating

```sh
biskit-mcp upgrade
```

That replaces the running executable with the latest release and nothing else. It never touches
`.mcp.json`, `.claude/`, or `.biskit/`, so your existing project registrations keep working. Pass
`--tag v0.1.4` to install a specific release, including an older one. The download is verified
against SHA256SUMS and a mismatch aborts before anything is replaced.

Windows cannot overwrite a running executable, so the current binary is renamed to
`biskit-mcp.exe.old` before the new one takes its place. If an agent still has the server open, that
file is cleaned up on the next upgrade instead. Restart any session that already had Biskit running
to pick up the new build.

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

Memory:

| Tool | Purpose |
|---|---|
| `list_memories` | Names of every stored memory |
| `read_memory` | Read one memory |
| `create_memory` | Write a new memory, refusing an existing name unless `overwrite` is set |
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

There is no `find_implementations`, since Luau has no interface or implementation relation. Use
`find_referencing_symbols` instead.

### Name paths

Symbols are addressed by name path. Slashes, dots, and colons are interchangeable, so an agent does
not have to guess:

| Pattern | Matches |
|---|---|
| `update` | any symbol named `update`, at any depth |
| `PlayerService/update` | `update` directly inside `PlayerService` |
| `PlayerService.update` | the same thing |
| `PlayerService:update` | the same thing |
| `/PlayerService` | only a top-level `PlayerService` |
| `UserInfo[1]` | the second of two same-named `UserInfo` symbols |

luau-lsp reports members as flat names, dot-separated for fields such as `PlayerService.addScore`
and colon-separated for methods such as `PlayerService:addScore`. Biskit splits both into name path
segments, so a method is reachable by its own name without naming the table that owns it, and LSP
requests anchor on the method rather than on that table.

Symbols that share a name within one file are listed as `UserInfo[0]` and `UserInfo[1]`. Those
labels can be passed straight back in, which is the only way to address one of them with
`find_declaration`, `find_referencing_symbols`, or `get_symbol_diagnostics`.

Response shapes:

- `find_symbol` answers with `{ symbols, truncated }`, where `truncated` reports whether
  `max_matches` cut the result set short and is omitted when nothing was cut. `list_dir` and
  `search_for_pattern` report truncation the same way.
- `find_symbol` and `find_declaration` key results by file path, and the symbols under a key carry
  no path of their own. `get_symbols_overview` answers with a bare list.
- `list_dir` answers with `{ base, directories, files }`, with every entry named relative to `base`.
  Join the two with `/` for a project-relative path. `find_file` and `search_for_pattern` answer
  with full project-relative paths.
- A symbol at the top of a result carries its full name path. A symbol nested under `children`
  carries only its own leaf name. Join them with `/` to address one: `update` under `PlayerService`
  is `PlayerService/update`.

`find_referencing_symbols` answers with `{ references, truncated }`, keyed by file the same way, and
is capped by `tools.max_reference_matches` rather than by the listing cap. It reports the reference
line on its own; pass `context_lines` to widen the snippet either side of it, at the cost of one
extra line per reference.

The language server's type signature for a symbol is omitted unless asked for. Pass
`include_detail: true` to `get_symbols_overview`, `find_symbol`, or `find_declaration` when the
signature is what you are after rather than the location.

### Memories

Memories are plain markdown under `.biskit/memories/`, nestable to any depth. Reference one from
another with a `mem:` pointer in backticks, such as `` `mem:combat/hit-detection` ``.
`rename_memory` rewrites those pointers for you.

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
