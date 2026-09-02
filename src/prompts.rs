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

const DELIVERED_CONNECTION_INSTRUCTIONS: &str = concat!(
    "Biskit provides symbolic code intelligence for Luau and a persistent project memory store. ",
    "This project's SessionStart hook has already delivered the usage manual and the index of ",
    "memories to you, so do NOT call `initial_instructions`; the manual is in your context ",
    "already. Read the memories relevant to your task with `read_memory` — that stored context ",
    "does not survive between sessions, and you do not have it until you read it. Biskit never ",
    "writes source files — use your own editing tools for that."
);

const DELIVERED_MEMORY_ONLY_CONNECTION_INSTRUCTIONS: &str = concat!(
    "Biskit provides a persistent project memory store plus file listing and text search. It is ",
    "configured for memory-only mode in this project, so the Luau language server is not running ",
    "and its symbol and diagnostic tools are not registered. This project's SessionStart hook has ",
    "already delivered the usage manual and the index of memories to you, so do NOT call ",
    "`initial_instructions`; the manual is in your context already. Read the memories relevant to ",
    "your task with `read_memory` — in this mode stored memory is the only durable project ",
    "knowledge Biskit has, and you do not have it until you read it. Biskit never writes source ",
    "files — use your own editing tools for that."
);

/// The stub the `initial_instructions` tool answers with once the SessionStart hook has already
/// put the manual into the agent's context. Sending the manual a second time is the single largest
/// avoidable cost at session start.
pub const ALREADY_DELIVERED: &str = concat!(
    "Already delivered. This project's SessionStart hook put Biskit's usage manual and memory ",
    "index into your context before this session began, and nothing has changed since. There is ",
    "nothing further to load here.\n\n",
    "Read the memories relevant to your task with `read_memory`.\n\n",
    "If the manual is genuinely absent from your context — a compaction dropped it, for instance ",
    "— call `initial_instructions` again with `force` set to true to have it sent in full."
);

pub fn connection_instructions(memory_only: bool, already_delivered: bool) -> &'static str {
    match (memory_only, already_delivered) {
        (true, true) => DELIVERED_MEMORY_ONLY_CONNECTION_INSTRUCTIONS,
        (true, false) => MEMORY_ONLY_CONNECTION_INSTRUCTIONS,
        (false, true) => DELIVERED_CONNECTION_INSTRUCTIONS,
        (false, false) => CONNECTION_INSTRUCTIONS,
    }
}

pub fn instructions_manual(memory_only: bool) -> &'static str {
    if memory_only {
        return MEMORY_ONLY_INSTRUCTIONS_MANUAL;
    }
    INSTRUCTIONS_MANUAL
}

const MANUAL_ROOT_CLOSE: &str = "</Main>";

pub fn initial_instructions(memories: &[String], memory_only: bool) -> String {
    let manual = instructions_manual(memory_only).trim_end();
    let body = manual
        .strip_suffix(MANUAL_ROOT_CLOSE)
        .unwrap_or(manual)
        .trim_end();

    let mut rendered = String::from(body);
    rendered.push_str(
        "\n\n<Section name=\"AvailableMemories\" desc=\"Memory index for this project. Names \
         only.\">\n",
    );

    if memories.is_empty() {
        rendered.push_str(
            "    <Rule>\n        None yet. Consider writing one with `create_memory` when you \
             learn something durable\n        about this project.\n    </Rule>\n",
        );
    } else {
        for name in memories {
            rendered.push_str("    <Memory name=\"");
            rendered.push_str(name);
            rendered.push_str("\" />\n");
        }
        rendered.push_str(
            "\n    <Rule>\n        This index is names only, and an index you never read is an \
             index you never used.\n        Before you start work, `read_memory` every entry above \
             that plausibly relates to your\n        task.\n    </Rule>\n",
        );
    }

    rendered.push_str("</Section>\n");
    rendered.push_str(MANUAL_ROOT_CLOSE);
    rendered.push('\n');
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undelivered_connection_instructions_demand_the_tool_call() {
        for memory_only in [false, true] {
            let text = connection_instructions(memory_only, false);
            assert!(
                text.contains("You MUST call the `initial_instructions` tool"),
                "memory_only={memory_only}"
            );
        }
    }

    #[test]
    fn delivered_connection_instructions_wave_the_tool_call_off() {
        for memory_only in [false, true] {
            let text = connection_instructions(memory_only, true);
            assert!(
                !text.contains("MUST call"),
                "still demands the call, memory_only={memory_only}"
            );
            assert!(
                text.contains("do NOT call `initial_instructions`"),
                "memory_only={memory_only}"
            );
        }
    }

    #[test]
    fn every_connection_variant_keeps_the_memory_only_marker_honest() {
        for already_delivered in [false, true] {
            assert!(
                connection_instructions(true, already_delivered).contains("memory-only mode"),
                "already_delivered={already_delivered}"
            );
            assert!(
                !connection_instructions(false, already_delivered).contains("memory-only mode"),
                "already_delivered={already_delivered}"
            );
        }
    }

    #[test]
    fn the_stub_names_the_escape_hatch_and_stays_small() {
        assert!(ALREADY_DELIVERED.contains("`force`"));
        assert!(ALREADY_DELIVERED.contains("read_memory"));
        assert!(
            ALREADY_DELIVERED.len() < MEMORY_ONLY_INSTRUCTIONS_MANUAL.len() / 4,
            "the stub has grown into a second manual"
        );
    }

    #[test]
    fn the_rendered_manual_carries_the_memory_index() {
        let rendered = initial_instructions(&["Combat/HitDetection".to_string()], true);
        assert!(rendered.contains("<Memory name=\"Combat/HitDetection\" />"));
        assert!(rendered.trim_end().ends_with(MANUAL_ROOT_CLOSE));
    }
}
