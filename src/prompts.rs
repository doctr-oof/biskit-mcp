pub const INSTRUCTIONS_MANUAL: &str = include_str!("../assets/instructions.md");
pub const MEMORY_ONLY_INSTRUCTIONS_MANUAL: &str =
    include_str!("../assets/instructions.memory-only.md");

const CONNECTION_INSTRUCTIONS: &str = concat!(
    "Biskit provides symbolic code intelligence for Luau and a persistent project memory store. ",
    "You MUST call the `initial_instructions` tool at the start of every session, before any other ",
    "Biskit tool and before starting work of any kind; it returns the usage manual. Then call ",
    "`list_memories` and read the memories relevant to your task with `read_memory` — that stored ",
    "context does not survive between sessions, and you do not have it until you read it. Biskit ",
    "never writes source files — use your own editing tools for that."
);

const MEMORY_ONLY_CONNECTION_INSTRUCTIONS: &str = concat!(
    "Biskit provides a persistent project memory store plus file listing and text search. It is ",
    "configured for memory-only mode in this project, so the Luau language server is not running ",
    "and its symbol and diagnostic tools are not registered. You MUST call the ",
    "`initial_instructions` tool at the start of every session, before any other Biskit tool and ",
    "before starting work of any kind; it returns the usage manual. Then call `list_memories` and ",
    "read the memories relevant to your task with `read_memory` — in this mode stored memory is the ",
    "only durable project knowledge Biskit has, and you do not have it until you read it. Biskit ",
    "never writes source files — use your own editing tools for that."
);

const SESSION_BRIEF: &str = concat!(
    "# Biskit MCP\n\n",
    "You MUST call the Biskit `initial_instructions` tool before doing anything else in this ",
    "session: before answering, reading a file, searching, planning, or calling any other tool. ",
    "This is non-negotiable. No exceptions for short tasks or projects you think you already ",
    "know.\n"
);

const SYMBOLIC_TOOLS_BRIEF: &str = concat!(
    "\n## Code navigation\n\n",
    "For Luau code, you MUST use Biskit's symbolic tools instead of Grep, Glob, or reading whole ",
    "files:\n",
    "- `find_symbol` / `find_declaration` to locate definitions\n",
    "- `find_referencing_symbols` to find usages\n",
    "- `get_symbols_overview` before reading a file\n",
    "- `find_file` / `list_dir` instead of Glob or ls\n",
    "- `search_for_pattern` only when no symbolic tool fits (strings, comments, non-Luau files)\n\n",
    "Native Grep/Glob are a last resort when Biskit's tools don't return what you need.\n"
);

pub fn connection_instructions(memory_only: bool) -> &'static str {
    if memory_only {
        return MEMORY_ONLY_CONNECTION_INSTRUCTIONS;
    }
    CONNECTION_INSTRUCTIONS
}

pub fn instructions_manual(memory_only: bool) -> &'static str {
    if memory_only {
        return MEMORY_ONLY_INSTRUCTIONS_MANUAL;
    }
    INSTRUCTIONS_MANUAL
}

pub fn session_brief(memory_only: bool) -> String {
    if memory_only {
        return SESSION_BRIEF.to_string();
    }
    format!("{SESSION_BRIEF}{SYMBOLIC_TOOLS_BRIEF}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section_sizes(manual: &str) -> Vec<(String, usize)> {
        let mut sizes = Vec::new();
        let mut rest = manual;
        while let Some(start) = rest.find("<Section name=\"") {
            let after = &rest[start + "<Section name=\"".len()..];
            let name_end = after.find('"').unwrap_or(0);
            let name = after[..name_end].to_string();
            let end = rest[start..]
                .find("</Section>")
                .map(|offset| start + offset + "</Section>".len())
                .unwrap_or(rest.len());
            sizes.push((name, end - start));
            rest = &rest[end..];
        }
        sizes
    }

    fn report(label: &str, manual: &str) -> usize {
        let sections = section_sizes(manual);
        let total = manual.len();
        println!("{label}: {total} bytes, ~{} tokens", total / 4);
        for (name, bytes) in &sections {
            println!(
                "  {name:<32} {bytes:>6} bytes  ~{:>5} tokens  {:>3}%",
                bytes / 4,
                bytes * 100 / total
            );
        }
        sections.len()
    }

    #[test]
    fn manual_size_report() {
        assert!(report("instructions.md", INSTRUCTIONS_MANUAL) > 0);
        assert!(
            report(
                "instructions.memory-only.md",
                MEMORY_ONLY_INSTRUCTIONS_MANUAL
            ) > 0
        );
        println!("session brief: {} bytes", session_brief(false).len());
        println!("session brief (memory-only): {} bytes", session_brief(true).len());
    }

    #[test]
    fn connection_instructions_demand_the_tool_call() {
        for memory_only in [false, true] {
            assert!(
                connection_instructions(memory_only)
                    .contains("You MUST call the `initial_instructions` tool"),
                "memory_only={memory_only}"
            );
        }
    }

    #[test]
    fn connection_instructions_keep_the_memory_only_marker_honest() {
        assert!(connection_instructions(true).contains("memory-only mode"));
        assert!(!connection_instructions(false).contains("memory-only mode"));
    }

    #[test]
    fn the_brief_only_demands_initial_instructions() {
        for memory_only in [false, true] {
            let brief = session_brief(memory_only);
            assert!(
                brief.contains("You MUST call the Biskit `initial_instructions` tool"),
                "memory_only={memory_only}"
            );
            assert!(!brief.contains("<Memory name="), "memory_only={memory_only}");
            assert!(!brief.contains("<Section"), "memory_only={memory_only}");
        }
    }

    #[test]
    fn only_the_full_brief_steers_toward_symbolic_tools() {
        assert!(session_brief(false).contains("you MUST use Biskit's symbolic tools"));
        assert!(!session_brief(true).contains("symbolic tools"));
        assert_eq!(session_brief(true), SESSION_BRIEF);
    }

    #[test]
    fn the_manuals_send_the_agent_to_list_memories() {
        for memory_only in [false, true] {
            let manual = instructions_manual(memory_only);
            assert!(
                !manual.contains("AvailableMemories"),
                "memory_only={memory_only}"
            );
            assert!(
                manual.contains("Read the memory index via the `list_memories` tool"),
                "memory_only={memory_only}"
            );
        }
    }
}
