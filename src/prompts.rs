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

pub fn initial_instructions(memories: &[String], memory_only: bool) -> String {
    let mut rendered = String::from(instructions_manual(memory_only));
    rendered.push_str("\n\n## Memories available in this project\n\n");

    if memories.is_empty() {
        rendered.push_str(
            "None yet. Consider writing one with `create_memory` when you learn something durable \
             about this project.\n",
        );
        return rendered;
    }

    for name in memories {
        rendered.push_str("- `");
        rendered.push_str(name);
        rendered.push_str("`\n");
    }
    rendered.push_str(
        "\nThis index is names only. Before you start work, read every entry above that plausibly \
         relates to your task with `read_memory`. Seeing the list is not the same as reading it.\n",
    );
    rendered
}
