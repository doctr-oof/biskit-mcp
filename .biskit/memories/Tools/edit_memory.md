# edit_memory

Replaces regex-matched text inside one memory, in place.

## Parameters
`EditMemoryRequest` (`src/server/requests.rs:23-34`):
- `memory_name` — string, required.
- `pattern` — string, required. A Rust regex matched against the memory body.
- `replacement` — string, required. Capture groups expand as `$1`, `$2`, `${name}`; `$$` inserts a literal dollar sign.
- `allow_multiple_occurrences` — bool, optional, defaults to `false`. Replaces every match instead of erroring on an ambiguous pattern.

## Implementation
- Handler: `src/server/mod.rs:258-275`. Calls `MemoryStore::edit(...)` and reports `Replaced N occurrence(s) in memory <name>.`.
- `MemoryStore::edit` (`src/memory.rs:113-163`) in order: resolve path, require it to be a file, read the whole body, build the regex with `RegexBuilder::multi_line(true).dot_matches_new_line(true)`, validate the replacement, count matches, apply `regex.replace_all`, write the file back.
- **Both regex flags are always on**: `^`/`$` anchor per line, and `.` crosses newlines. A bare `.*` therefore swallows the rest of the memory — this is the opposite default from `search_for_pattern`, which leaves `dot_matches_newline` off.
- `validate_replacement` (`src/memory.rs:321-357`) scans the replacement for `$` references, skipping `$$`, handling both `$name` and `${name}` forms, and checks each against the compiled regex with `reference_resolves` (numeric index against `captures_len()`, named group against `capture_names()`). An unknown group is refused before anything is written, rather than silently expanding to empty.
- The whole edit is read-modify-write of the entire file; there is no partial or streaming write.

## Edge cases
- Invalid regex: `invalid regular expression: <pattern>: <error>` with `REGEX_HINT` (`src/memory.rs:14-15`) explaining the flags and which metacharacters to escape.
- Zero matches: `pattern did not match anything in memory <name>: <pattern>`, hinted to `read_memory <name>` and copy the target text verbatim.
- More than one match with `allow_multiple_occurrences` false: `pattern matched N times in memory <name>`, hinted to narrow the pattern or pass the flag. The file is untouched.
- Replacement naming a group the pattern does not define: refused with `replacement names capture group $X, which the pattern does not define; write $$X to insert it literally` plus `REPLACEMENT_HINT` (`src/memory.rs:16-17`). This is why a literal dollar amount must be written `costs $$5`.
- A `$` followed by nothing reference-like (for example `and $ then`) is left alone — the empty reference passes validation and `replace_all` emits the bare `$`.
- Missing memory: `memory not found: <name>` with the `list_memories` hint.
- Editing a memory's own `mem:` pointers here does not update anything else; reference maintenance belongs to `mem:Tools/rename_memory`.

## Relevant files
- `src/server/mod.rs`
- `src/server/requests.rs`
- `src/server/descriptions.rs`
- `src/memory.rs`
- `src/project.rs`
- `src/errors.rs`

Siblings: `mem:Tools/create_memory`, `mem:Tools/rename_memory`, `mem:Tools/delete_memory`, `mem:Tools/read_memory`, `mem:Tools/list_memories`, `mem:Tools/initial_instructions`.
