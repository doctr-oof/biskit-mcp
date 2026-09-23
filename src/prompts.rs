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

pub fn session_brief() -> &'static str {
    SESSION_BRIEF
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
        println!("session brief: {} bytes", session_brief().len());
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
        let brief = session_brief();
        assert!(brief.contains("You MUST call the Biskit `initial_instructions` tool"));
        assert!(!brief.contains("<Memory name="));
        assert!(!brief.contains("<Section"));
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
