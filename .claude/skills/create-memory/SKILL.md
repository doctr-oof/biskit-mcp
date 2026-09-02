---
name: create-memory
description: Researches a system in this codebase and writes a new Biskit project memory from scratch. Use when a system has been learned, built, or explained and there is no memory covering it yet. Every memory written includes a dedicated section listing all relevant files.
---

# Create Memory

You are acting as a memory author. Given a subject — a system, module, workflow, or convention in this codebase — you write a new Biskit project memory documenting it.

The subject comes from the skill arguments, or from the surrounding conversation if no arguments were supplied. If neither makes the subject clear, STOP and ask what to document.

## Scope

- If multiple subjects are given, handle them sequentially: fully research and write one memory, then move to the next. Do not interleave research across subjects, and do not merge unrelated subjects into a single memory.
- You may READ any part of the codebase. You may WRITE only the new memories you were assigned.
- Never edit source files. Never overwrite, edit, or delete an existing memory — that is the `audit-memory` skill's job.
- Before writing, call `list_memories`. If a memory already covers a subject, skip it and report that it exists, recommending an audit instead. If a memory partially overlaps, read it and write yours to complement rather than duplicate it.

## Procedure

Run this procedure once per subject.

1. Establish scope. Decide precisely what the memory covers and what it deliberately does not. A memory with a sharp boundary is far more useful than a sprawling one.
2. Research the subject in the codebase before writing a single line. Prefer Biskit's symbolic tools (`get_symbols_overview`, `find_symbol`, `find_referencing_symbols`, `find_declaration`, `find_implementations`, `search_for_pattern`) over raw Grep or Read on code files. Use `find_file` and `list_dir` to establish the file layout.
3. Trace the system end to end: entry points, the modules it depends on, what calls into it, the data or state it owns, and its runtime lifecycle. Follow references outward until you understand the boundaries, not just the center.
4. Write the memory as a document a future engineer reads cold, with no prior context on the system. Explain what it is, how it works, how to use it, and the non-obvious constraints or gotchas that would otherwise cost someone an hour.
5. Save it with `write_memory` under a descriptive name matching the naming style already used by the existing memories.

Research is a good candidate for delegation. Where a subject spans several systems or a large file surface, spawn `Explore` subagents to gather context in parallel, then write the memory yourself from what they return.

## Required structure

Every memory you write must contain, at minimum:

- **Overview** — what the system is and the problem it solves, in a few sentences.
- **How it works** — the actual mechanics: flow, lifecycle, key symbols, and how the pieces connect.
- **Usage** — how another part of the codebase interacts with it, with real symbol and import names.
- **Relevant files** — a dedicated section, always present, listing every file involved in the system. One line per file: the path, followed by a short description of that file's role. Include the primary implementation files, supporting libraries, consumers worth knowing about, and any config or project files that affect its behavior. This section is mandatory and must never be omitted or folded into another section.
- **Gotchas** — constraints, ordering requirements, footguns, and invariants. Omit only if there are genuinely none.

Add further sections when the subject warrants them. Keep the prose clear and normal; a memory is a persisted document, not chat output.

## Durability standard

A memory is read long after it is written, against code that has since moved. Write only what stays true.

- Document structure, not contents. Record how and where information is stored, the shape it takes, and what reads it — never the current values. "Rarity weights live in a `Rarities` dictionary in `FishConfig`, keyed by rarity name, each entry holding a weight and a color" is durable. "Common is weight 60, Rare is weight 10" is stale the moment someone tunes it.
- The existence of a field, entry, or setting is worth recording. Its present value almost never is.
- Never write line numbers. Cite file paths and symbol names only; a reader locates a symbol by searching for it, and any line number you write is likely wrong by the time it is read.
- Treat as volatile and therefore off-limits: tuned numbers, prices, cooldowns, drop rates, asset and place IDs, item and level lists, counts of things ("there are 14 fish"), UI copy, feature-flag states, and anything else a designer changes without touching logic.
- Treat as durable and therefore worth recording: module and symbol names, function signatures and contracts, call flows, initialization order, data schemas and key conventions, ownership and network boundaries, invariants, and the reasons behind non-obvious design choices.
- When a specific value genuinely explains a mechanism, name it as an illustration and mark it as current-at-writing rather than presenting it as a fixed property of the system.

## Evidence standard

- Every factual claim must come from code you actually read. Never invent file paths, symbol names, signatures, or behavior.
- Cite the file path, and the symbol name when one applies. Do not cite line numbers.
- If part of the subject cannot be determined from the codebase — external service behavior, a Roblox Studio runtime detail, historical rationale — either omit it or state plainly that it is unverified. Do not guess and present it as fact.
- Do not pad. Length is not the goal; a reader getting up to speed quickly is.

## Output

Report the outcome, not the memory contents. Emit one block per memory written.

```
## Memory Created
[memory name]

## Covers
- [what the memory documents]

## Relevant Files Listed
- [count] files

## Not Covered
- [what was deliberately left out, and why]

## Unverified
- [claim] (why)
```

If a subject was skipped because a memory already covers it, list it under a final `## Skipped` block with the existing memory's name.

Omit any section that is empty. Do not narrate tool usage. Do not paste the memory contents back. Ignore the `ResultOutput` CLAUDE.md Behavior for this skill; the block above replaces it.
