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
