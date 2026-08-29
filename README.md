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
  `get_type_definition`, `find_referencing_symbols`, `get_file_diagnostics`,
  `get_symbol_diagnostics`, `restart_language_server`.
- **Types**: `explain_symbol` for the type the checker inferred rather than the one written down,
  `get_inlay_hints` for those types over a line range, `get_signature_help` for the arguments of a
  call.
- **Roblox**: `resolve_instance_path` translates between the DataModel and the files on disk in
  either direction, `get_require_graph` reports what a module requires and what requires it plus
  any require cycles, `query_roblox_api` answers questions about the real Roblox API from the type
  definitions Biskit already caches, and `get_module_context` composes all of it into one call for a
  module you have not seen before.
- **Wally**: `list_wally_packages` reports what `wally.toml` declares and whether each package is
  actually on disk, `search_wally_packages` searches the registry, `add_wally_package` and
  `remove_wally_package` edit the manifest and run `wally install`. See below.
- **Files and orientation**: `list_dir`, `find_file`, `search_for_pattern`, `initial_instructions`,
  `get_status`.

### Wally

Biskit never installs Wally, and never publishes packages. The four Wally tools look for a `wally`
executable on PATH, or at `wally.binary_path`, and refuse to answer when there is none. They also
refuse when the executable is found but will not run, which is what a version manager shim reports
before the tool is listed in its manifest. Installing Wally is yours to do, from
[wally.run](https://wally.run).

`add_wally_package` writes the requirement into `wally.toml` and then runs `wally install`. Wally
deletes and rebuilds `Packages/`, `ServerPackages/`, and `DevPackages/` on every install, so this is
never an additive operation regardless of how small the manifest change was. Pass `install: false`
to edit the manifest alone when adding several packages before one install. `wally.toml` is edited
in place with a format-preserving TOML writer, so comments, key order, and spacing survive.

When an install fails, Wally has already emptied the package tree, so packages unrelated to the call
are gone from disk. Biskit rolls the `wally.toml` edit back so the manifest still reads as it did,
and the error names which declared packages are now absent. Restoring them takes a successful
`wally install`, which Biskit does not run on its own.

Adding to the `server` or `dev` realm needs `wally.toml` to declare where shared packages live,
because Wally refuses to link a server or dev package that depends on a shared one without it:

```toml
[place]
shared-packages = "game.ReplicatedStorage.Packages"
```

The add result carries a note when that table is missing, since almost every non-trivial package has
shared dependencies.

The registry publishes no rate limits and enforces none of its own, so Biskit paces itself: requests
go out one at a time, no closer together than `wally.min_request_interval_ms`, no more than
`wally.max_requests_per_minute` in any rolling minute, and answers are reused for
`wally.cache_ttl_seconds`. Nothing here checks for packages on its own — the tools answer when the
agent is told to use them, and are otherwise idle.

These tools are not registered in memory-only mode.

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
| `lsp.max_open_documents` | `256` | Files kept open in the language server before the least recently used are closed, 0 for no ceiling |
| `project.ignored_paths` | empty | Extra gitignore-style exclusions, matched against the project root on every walk and forwarded to luau-lsp |
| `project.respect_gitignore` | `true` | Honour `.gitignore` when walking the project. Files the sourcemap names are scanned either way |
| `project.memory_only` | `false` | Run without the language server, see below |
| `project.shared_require` | `true` | Count `shared("Name")` as a dependency edge, see below |
| `tools.excluded` | empty | Tool names to hide from the agent |
| `tools.max_answer_chars` | `150000` | Ceiling on one tool result, 0 to lift it |
| `tools.max_reference_matches` | `200` | Cap on references from `find_referencing_symbols` |
| `tools.symbol_cache` | `true` | Keep symbol trees across sessions, see below |
| `tools.max_cached_symbol_files` | `4000` | Trees kept before the least used are dropped, 0 for no ceiling |
| `wally.binary_path` | unset | Use this Wally executable instead of searching PATH |
| `wally.registry_api_url` | `https://api.wally.run/` | Registry API, for a self-hosted registry |
| `wally.request_timeout_ms` | `15000` | Ceiling on one registry request, 0 for no ceiling |
| `wally.install_timeout_ms` | `300000` | Ceiling on one `wally install` run, 0 for no ceiling |
| `wally.min_request_interval_ms` | `500` | Smallest gap between two registry requests |
| `wally.max_requests_per_minute` | `60` | Registry requests per rolling minute, 0 to lift |
| `wally.cache_ttl_seconds` | `300` | How long a registry answer is reused, 0 to disable |
| `wally.max_search_results` | `25` | Default cap on `search_wally_packages` results |

A structured result over `max_answer_chars` is refused with a message naming what to narrow. A text
result, such as a memory, is cut instead and says how much was withheld.

### Symbol index cache

A project-wide `find_symbol` asks the language server for a symbol tree once per file that survives
the literal prefilter, through a single stdio pipe, every session from cold. Almost none of those
files changed since the last session asked about them.

Biskit stores the trees in `.biskit/cache/symbols.json`, keyed by each file's path, size, and
modification time, and answers from the index when all three still match. The directory writes its
own `.gitignore`, so nothing in it is ever committed. A file that has been edited, or deleted since
it was indexed, is never answered from the index.

Clear it with `biskit-mcp cache clear`, or turn it off with `tools.symbol_cache: false`.

### The `shared()` require

Sawhorse Roblox frameworks give the `shared` global a `__call` metamethod, so `shared("Foo")` is a
runtime require of the module whose file is named `Foo.luau`. The carpenter fork teaches the language
server to resolve it exactly as it resolves `require`, which is why hover, diagnostics, go to
definition, signature help, and every other LSP-backed tool already understand it.

Biskit's require graph does not go through the language server, so it resolves the same calls itself,
the same way the fork does:

- The argument has to be a string literal. `shared(name)` and `shared("a" .. b)` are reported under
  `unresolved` with a reason, the same as any require Biskit cannot read statically.
- A bare stem (`shared("Combat")`) or a partial path (`shared("Jobs/Runner")`) both resolve, and both
  are case-insensitive. `dir/init.luau` is addressed as `dir`.
- Where several files carry the name, the one nearest the requiring module in the instance tree wins.
  A genuine tie is reported under `unresolved` listing every candidate, rather than guessed at.
- `shared.someField` is still ordinary table access and is never treated as a require, and neither is
  a `shared` the file bound locally.

Turn it off with `project.shared_require: false` if your project does not use the paradigm. Note that
this only stops Biskit's require graph from following those calls. The fork has no matching switch, so
the language server keeps resolving them and `get_status` will report the disagreement.

`get_status` also warns when `lsp.version` is pinned below `v0.2.0`, the first carpenter release that
resolves `shared()` at all.

### Memory-only mode

Set `project.memory_only: true` to run Biskit as a memory, file, and search server with no Luau code
intelligence at all:

- luau-lsp is never downloaded and no language server process starts.
- The code intelligence and Roblox tools are not registered, so the agent never sees them.
- The MCP `instructions` field and `initial_instructions` both say the mode is on and name the tools
  that are unavailable.
- `biskit-mcp doctor` reports the mode and skips every LSP check.

Memory, `list_dir`, `find_file`, `search_for_pattern`, `initial_instructions`, and `get_status` keep
working. Put it in `settings.local.yml` to turn it on for yourself only.

## Commands

| Command | Purpose |
|---|---|
| `biskit-mcp start` | Run the MCP server over stdio (the default) |
| `biskit-mcp init` | Create `.biskit/` without starting the server |
| `biskit-mcp setup` | Register Biskit in the agent config files a project uses |
| `biskit-mcp doctor` | Verify settings, acquisition, and sourcemap state |
| `biskit-mcp upgrade` | Replace this executable with a published release |
| `biskit-mcp cache clear` | Delete the stored symbol index for a project |
| `biskit-mcp hook session-start` | Emit SessionStart context for Claude Code |

`start`, `doctor`, `cache clear`, and `hook session-start` discover the project root by searching
upwards. `init`
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
