pub const INSTRUCTIONS_MANUAL: &str = include_str!("../assets/instructions.md");
pub const MEMORY_ONLY_INSTRUCTIONS_MANUAL: &str =
    include_str!("../assets/instructions.memory-only.md");

const CONNECTION_INSTRUCTIONS: &str = concat!(
    "Biskit provides symbolic code intelligence for Luau and a persistent project memory store. ",
    "You MUST call the `initial_instructions` tool at the start of every session, before any other ",
    "Biskit tool and before starting work of any kind; it returns the usage manual and the index of ",
    "memories available for this project. Then read the memories relevant to your task with ",
    "`read_memory` — that stored context does not survive between sessions, and you do not have it ",
    "until you read it. Biskit never writes source files — use your own editing tools for that."
);

const MEMORY_ONLY_CONNECTION_INSTRUCTIONS: &str = concat!(
    "Biskit provides a persistent project memory store plus file listing and text search. It is ",
    "configured for memory-only mode in this project, so the Luau language server is not running ",
    "and its symbol and diagnostic tools are not registered. You MUST call the ",
    "`initial_instructions` tool at the start of every session, before any other Biskit tool and ",
    "before starting work of any kind; it returns the usage manual and the index of memories ",
    "available for this project. Then read the memories relevant to your task with `read_memory` — ",
    "in this mode stored memory is the only durable project knowledge Biskit has, and you do not ",
    "have it until you read it. Biskit never writes source files — use your own editing tools for ",
    "that."
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

const MANUAL_ROOT_CLOSE: &str = "</Main>";

const SESSION_BRIEF_BUDGET_CHARS: usize = 8_000;

const BRIEF_SECTIONS: &[&str] = &[
    "WhatBiskitIs",
    "MemoryOnlyMode",
    "SessionStart",
    "WritingMemories",
];

const BRIEF_HEADER: &str = "# Biskit MCP Session Brief\n\n<Main name=\"BiskitSessionBrief\">";

const BRIEF_FULL_MANUAL: &str = concat!(
    "<Section name=\"FullManual\" desc=\"Where the rest of the manual lives. MANDATORY.\">\n",
    "    <Rule>\n",
    "        This brief carries only the session-start rules, not the tool manual. You MUST call\n",
    "        `initial_instructions` for the full manual before calling any Biskit tool other than\n",
    "        `list_memories` and `read_memory`. No exceptions.\n",
    "    </Rule>\n",
    "</Section>"
);

fn section<'a>(manual: &'a str, name: &str) -> Option<&'a str> {
    let start = manual.find(&format!("<Section name=\"{name}\""))?;
    let length = manual[start..].find("</Section>")? + "</Section>".len();
    Some(&manual[start..start + length])
}

fn brief_body(memory_only: bool) -> String {
    let manual = instructions_manual(memory_only);
    let mut body = String::from(BRIEF_HEADER);
    for text in BRIEF_SECTIONS
        .iter()
        .filter_map(|name| section(manual, name))
    {
        body.push('\n');
        body.push_str(text);
        body.push('\n');
    }
    body.push('\n');
    body.push_str(BRIEF_FULL_MANUAL);
    body
}

fn manual_body(memory_only: bool) -> &'static str {
    let manual = instructions_manual(memory_only).trim_end();
    manual
        .strip_suffix(MANUAL_ROOT_CLOSE)
        .unwrap_or(manual)
        .trim_end()
}

fn assemble(body: &str, memories: &str) -> String {
    let mut rendered = String::from(body);
    rendered.push_str(
        "\n\n<Section name=\"AvailableMemories\" desc=\"Memory index for this project. Names \
         only.\">\n",
    );
    rendered.push_str(memories);
    rendered.push_str("</Section>\n");
    rendered.push_str(MANUAL_ROOT_CLOSE);
    rendered.push('\n');
    rendered
}

fn memory_index(memories: &[String]) -> String {
    if memories.is_empty() {
        return String::from(
            "    <Rule>\n        None yet. Consider writing one with `create_memory` when you \
             learn something durable\n        about this project.\n    </Rule>\n",
        );
    }

    let mut index = String::new();
    for name in memories {
        index.push_str("    <Memory name=\"");
        index.push_str(name);
        index.push_str("\" />\n");
    }
    index.push_str(
        "\n    <Rule>\n        This index is names only, and an index you never read is an \
         index you never used.\n        Before you start work, `read_memory` every entry above \
         that plausibly relates to your\n        task.\n    </Rule>\n",
    );
    index
}

fn memory_directive(count: usize) -> String {
    format!(
        "    <Rule>\n        Index too large to show here: this project has {count} memories. \
         You MUST call\n        `list_memories` now, before any other work, then `read_memory` \
         every entry that\n        plausibly relates to your task.\n    </Rule>\n"
    )
}

fn memories_section(memories: &[String], memory_only: bool) -> String {
    let index = memory_index(memories);
    if assemble(&brief_body(memory_only), &index).len() <= SESSION_BRIEF_BUDGET_CHARS {
        return index;
    }
    memory_directive(memories.len())
}

pub fn initial_instructions(memories: &[String], memory_only: bool) -> String {
    assemble(
        manual_body(memory_only),
        &memories_section(memories, memory_only),
    )
}

pub fn session_brief(memories: &[String], memory_only: bool) -> String {
    assemble(
        &brief_body(memory_only),
        &memories_section(memories, memory_only),
    )
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

    fn names(count: usize) -> Vec<String> {
        (0..count)
            .map(|index| format!("Subsystem{index}/SomeReasonablyLongTopicName"))
            .collect()
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
        for memory_only in [false, true] {
            assert!(report("session brief", &session_brief(&[], memory_only)) > 0);
        }
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
    fn the_rendered_manual_carries_the_memory_index() {
        let rendered = initial_instructions(&["Combat/HitDetection".to_string()], true);
        assert!(rendered.contains("<Memory name=\"Combat/HitDetection\" />"));
        assert!(rendered.trim_end().ends_with(MANUAL_ROOT_CLOSE));
    }

    #[test]
    fn every_brief_section_exists_in_the_manual_it_is_cut_from() {
        for name in BRIEF_SECTIONS {
            assert!(
                section(MEMORY_ONLY_INSTRUCTIONS_MANUAL, name).is_some(),
                "{name} missing from instructions.memory-only.md"
            );
            if *name != "MemoryOnlyMode" {
                assert!(
                    section(INSTRUCTIONS_MANUAL, name).is_some(),
                    "{name} missing from instructions.md"
                );
            }
        }
    }

    #[test]
    fn the_brief_carries_the_session_rules_and_demands_the_manual() {
        for memory_only in [false, true] {
            let brief = session_brief(&["Combat/HitDetection".to_string()], memory_only);
            assert!(brief.contains("<Behavior name=\"CheckMemoryFirst\">"));
            assert!(brief.contains("<Section name=\"WritingMemories\""));
            assert!(brief.contains("You MUST call\n        `initial_instructions`"));
            assert!(brief.contains("<Memory name=\"Combat/HitDetection\" />"));
            assert!(!brief.contains("<Section name=\"ChoosingATool\""));
            assert!(brief.trim_end().ends_with(MANUAL_ROOT_CLOSE));
            assert_eq!(
                brief.contains("<Section name=\"MemoryOnlyMode\""),
                memory_only
            );
        }
    }

    #[test]
    fn the_brief_stays_under_budget_at_any_store_size() {
        for memory_only in [false, true] {
            for count in [0, 30, 100, 500, 5_000] {
                let brief = session_brief(&names(count), memory_only);
                assert!(
                    brief.len() <= SESSION_BRIEF_BUDGET_CHARS,
                    "{} chars, memory_only={memory_only}, count={count}",
                    brief.len()
                );
            }
        }
    }

    #[test]
    fn an_index_over_budget_becomes_a_list_memories_directive() {
        for memory_only in [false, true] {
            let memories = names(500);
            for rendered in [
                session_brief(&memories, memory_only),
                initial_instructions(&memories, memory_only),
            ] {
                assert!(!rendered.contains("<Memory name="));
                assert!(rendered.contains("this project has 500 memories"));
                assert!(rendered.contains("You MUST call\n        `list_memories` now"));
            }
        }
    }

    #[test]
    fn a_small_index_is_shown_in_full_by_both() {
        for memory_only in [false, true] {
            let memories = names(30);
            for rendered in [
                session_brief(&memories, memory_only),
                initial_instructions(&memories, memory_only),
            ] {
                assert!(
                    rendered
                        .contains("<Memory name=\"Subsystem29/SomeReasonablyLongTopicName\" />")
                );
                assert!(!rendered.contains("Index too large"));
            }
        }
    }
}
