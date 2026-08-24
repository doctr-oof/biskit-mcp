macro_rules! description {
    (initial_instructions) => {
        "Returns Biskit's usage manual and the index of this project's memories. Call before any \
         other Biskit tool."
    };
    (list_memories) => {
        "Lists the names of this project's memories."
    };
    (read_memory) => {
        "Reads a memory's full markdown content."
    };
    (create_memory) => {
        "Writes a markdown memory of durable project knowledge. Use a meaningful, nestable name. \
         Errors if the name is taken unless overwrite is set."
    };
    (delete_memory) => {
        "Deletes a memory."
    };
    (edit_memory) => {
        "Replaces regex-matched content in a memory. Prefer over rewriting a memory wholesale."
    };
    (rename_memory) => {
        "Renames or moves a memory, rewriting every `mem:` reference to it."
    };
    (list_dir) => {
        "Lists files and directories under a project-relative path. Ignored paths are omitted: \
         .gitignore when project.respect_gitignore is on, plus project.ignored_paths. Naming an \
         ignored directory as relative_path lists it anyway."
    };
    (find_file) => {
        "Finds files whose name matches a glob mask. Ignored paths are omitted: .gitignore when \
         project.respect_gitignore is on, plus project.ignored_paths. Naming an ignored directory \
         as relative_path searches it anyway."
    };
    (search_for_pattern) => {
        "Searches file contents with a regular expression. Use for non-symbol text; use \
         find_symbol for definitions. mode \"files\" returns matching paths and \"counts\" returns \
         per-file match counts, both far cheaper than snippets. \".\" stops at end of line unless \
         dot_matches_newline is set. Ignored paths are omitted: .gitignore when \
         project.respect_gitignore is on, plus project.ignored_paths; naming an ignored directory \
         as relative_path searches it anyway."
    };
    (get_symbols_overview) => {
        "Lists the symbols a Luau file defines. Call before reading a file to decide what is worth \
         reading."
    };
    (find_symbol) => {
        "Finds symbols by name path across the project or within one file or directory."
    };
    (find_declaration) => {
        "Finds where a symbol is declared. name_path resolves only against symbols the named file \
         declares; to follow a symbol into another file, point line and column at a call site."
    };
    (find_referencing_symbols) => {
        "Finds every symbol that references the given symbol."
    };
    (get_file_diagnostics) => {
        "Gets diagnostics for a file, optionally limited to a line range, grouped by severity and \
         containing symbol."
    };
    (get_symbol_diagnostics) => {
        "Gets diagnostics for one symbol and, optionally, for every file that references it. Use \
         after editing a symbol."
    };
    (explain_symbol) => {
        "Reports the type the language server infers for a symbol, not the type written in source. \
         Point at it with name_path or with line and column."
    };
    (get_type_definition) => {
        "Finds where a type is declared, usually an `export type` in another module. Point line and \
         column at the type's name in an annotation. For values, use find_declaration."
    };
    (get_inlay_hints) => {
        "Lists the inferred types the language server would draw inline over a line range. Cheapest \
         way to see what a function's variables, arguments, and returns resolve to without reading \
         its body."
    };
    (get_signature_help) => {
        "Reports a call's parameters without reading the callee. Aim line and column inside the \
         call's parentheses; the result names which argument that position is."
    };
    (resolve_instance_path) => {
        "Maps between the Roblox DataModel and files on disk, either direction. Pass instance_path \
         (for example game.ReplicatedStorage.Shared.Combat) to get its file, or relative_path to \
         get where that file lands in the game. Use before assuming a file's instance path."
    };
    (get_require_graph) => {
        "Reports which modules a module requires and which require it, resolved through the \
         sourcemap rather than by text search. Omit relative_path for a project-wide answer naming \
         every require cycle and every unresolved require. Call before editing a module: \
         find_referencing_symbols sees symbol uses, not module-level coupling."
    };
    (get_module_context) => {
        "Summarizes one module in a single call: instance path, run context (server, client, or \
         both), what it requires, what requires it, what its returned table exposes, and its \
         diagnostic count. Call on an unfamiliar module instead of four or five separate calls."
    };
    (query_roblox_api) => {
        "Looks up the real Roblox API from Biskit's cached type definitions: a class's members, one \
         member's signature and docs, deprecation and its replacement, or an enum's items. Use \
         instead of recalling the Roblox API from memory."
    };
    (get_status) => {
        "Reports Biskit's state: project root, language server state, sourcemap freshness, memory \
         count, and non-default settings. Call when a tool returns nothing and you cannot tell \
         whether that means no matches."
    };
    (restart_language_server) => {
        "Restarts the Luau language server. Use when symbol results are stale or empty for a file \
         you know has symbols."
    };
}

pub(super) use description;
