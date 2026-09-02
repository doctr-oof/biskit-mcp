# get_require_graph

Reports which modules a module requires and which require it, resolved through the sourcemap rather than by text search; project-wide it reports cycles and unresolved requires. Not registered while `project.memory_only` is true (`src/server/mod.rs:94-145`).

## Parameters
| Param | Type | Default | Required |
| --- | --- | --- | --- |
| `relative_path` | string | none (project-wide answer) | no |
| `direction` | string | `"both"` | no |
| `depth` | u32 | `1` | no |
| `include_cycles` | bool | `true` when `relative_path` is omitted, else `false` | no |
| `include_unresolved` | bool | `true` | no |

`RequireGraphRequest`, `src/server/requests.rs:314-332`. `direction` accepts `"dependencies"`, `"dependents"`, `"both"` only; anything else errors (`Direction::parse`, `src/roblox/requires/mod.rs:23-37`). The `include_cycles` default is applied in the handler as `request.include_cycles.unwrap_or(request.relative_path.is_none())` (`src/server/mod.rs:634-637`). There is no caller-facing limit: it is fixed to `tools.max_listing_entries` (default 2000).

## Implementation
- Handler: `src/server/mod.rs:607-644`. Parses direction, loads the sourcemap, builds/reuses the require graph, calls `RequireGraph::answer` (`src/roblox/requires/graph.rs:275-361`).
- Graph build: `build_or_reuse` (`src/roblox/requires/build.rs:35-50`) reuses the cached graph while the `GraphStamp` — sourcemap (mtime, len), file count, total bytes, newest mtime — is unchanged. Cached in `RobloxIndex` (`src/roblox/index.rs:60-74`).
- File set = the project walk (honouring `project.respect_gitignore` and `project.ignored_paths`) **plus** every Luau file the sourcemap names, so vendored `Packages/` still counts even when gitignored (`luau_files`, `src/roblox/requires/build.rs:57-89`).
- Scanning: comments are blanked in place, preserving byte offsets and line breaks (`blank_comments`, `src/roblox/requires/mod.rs:40-94`), so a commented-out require is not an edge. `find_requires` matches the word `require` followed by a balanced paren group or a bare string literal (`src/roblox/requires/scan.rs:34-87`); `require` inside a longer identifier is skipped.
- `local Name = expr` bindings are collected by regex (`local_bindings`, `src/roblox/requires/scan.rs:216-230`) — last assignment wins — and used to resolve heads such as `Shared` in `require(Shared.C)`, up to `MAX_BINDING_HOPS` = 8 hops (`src/roblox/requires/resolve.rs:66-93`).
- Instance-expression requires: `parse_chain` (`src/roblox/requires/chain.rs`) handles `.Name`, `.Parent`, `["Name"]`, and `:GetService/:WaitForChild/:FindFirstChild("Name")`. Heads `script`, `game`, `workspace` are special-cased; any other head is a local binding or a direct child of the root. The resolved node must have a Luau `script_file`, otherwise the require is unresolved with "resolves to X, which is a Folder rather than a script".
- String requires (`src/roblox/requires/string_require.rs`): `@Alias/...` resolved against the nearest `.luaurc` `aliases` walking up to the project root (`//` line comments stripped before JSON parse), and `./`, `../` relative paths. Any other string form is unresolved. Candidate suffixes tried in order: `.luau`, `.lua`, `/init.luau`, `/init.lua`.
- Shared requires: when `project.shared_require` is true (default), `shared("Name")` calls count as edges (`src/roblox/shared_require.rs`, `src/roblox/requires/resolve.rs:10-35`). Scanning is skipped for a file that locally binds `shared`. `SharedIndex` is a case-insensitive index keyed by file stem (an `init` file keys on its directory name), skipping anything under a `_Index` segment (Wally realization copies). Bare stems and partial paths (`"jobs/Foo"`) both match; ties are broken by most common leading instance-path segments with the requiring module, then by shallowest module path; a remaining tie returns `Ambiguous` and becomes an unresolved entry listing the candidates. Self-requires never match.
- Walking: `RequireGraph::walk` is breadth-first with a `seen` set, so each module is reported once at its shallowest depth; `depth` is clamped to at least 1 (`src/roblox/requires/graph.rs:84-128`). Each `ReachedModule` carries `relative_path`, `instance_path`, `depth`, `via` (the require expression), `from` (only for hops past the first), `line`.
- Cycles: iterative white/grey/black DFS over dependency edges; each cycle is the loop it closes with the first module repeated at the end; duplicates suppressed (`cycles`, `src/roblox/requires/graph.rs:160-214`).
- Answer (`GraphAnswer`, `src/roblox/requires/graph.rs:242-262`): `relative_path`, `instance_path`, `modules_scanned`, `dependencies`, `dependents`, `unresolved`, `cycles`, `truncated`, `sourcemap` (same `SourcemapReference` and staleness contract as `mem:Tools/resolve_instance_path`).

## Edge cases
- An unknown `relative_path` errors with "no Luau file at ..." — the module must be in the scanned set, not merely on disk.
- Project-wide answers never include `dependencies`/`dependents`; they only carry `unresolved` and (by default) `cycles`.
- `truncated: true` means either the walk, the cycle list, or the unresolved list hit `tools.max_listing_entries`; the flag is shared, so it does not say which.
- Self-edges are dropped: a module requiring itself produces no edge and no unresolved entry (`src/roblox/requires/build.rs:200-210`).
- A require that resolves to a path outside the scanned file set becomes an unresolved entry, reason "resolves to X, which is outside the files Biskit scans".
- Dependent edges are deduped and sorted; the `via`/`line` on a dependent hop is looked up from the dependent's own edge list.
- Scanning is textual, not a parse: `require` inside a string is blanked only for comments, and dynamic requires (`require(Shared[name])`) are always unresolved by design.
- The graph rebuild is a full re-read of every Luau file, done on a blocking task; the stamp is byte-total based, so an edit that keeps total size and mtimes identical would be missed.

## Relevant files
- `src/server/mod.rs`, `src/server/requests.rs`, `src/server/descriptions.rs`
- `src/roblox/requires/mod.rs` — `Direction`, `blank_comments`
- `src/roblox/requires/build.rs` — file set, `GraphStamp`, graph construction
- `src/roblox/requires/scan.rs` — require and `shared` call scanning, local bindings
- `src/roblox/requires/chain.rs` — instance expression parsing
- `src/roblox/requires/resolve.rs` — resolution entry points
- `src/roblox/requires/string_require.rs` — `@alias` and relative string requires, `.luaurc`
- `src/roblox/requires/graph.rs` — walk, cycles, unresolved, `GraphAnswer`
- `src/roblox/shared_require.rs` — `shared("Name")` index and tie-breaking
- `src/roblox/sourcemap.rs`, `src/roblox/index.rs`, `src/config.rs`

Siblings: `mem:Tools/resolve_instance_path`, `mem:Tools/get_module_context`, `mem:Tools/query_roblox_api`.
