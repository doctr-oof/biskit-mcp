---
name: audit-memory
description: Audits existing Biskit project memories against the real codebase and rewrites them to be accurate. Corrects misinformation, deletes defunct content, and adds missing information a memory should have covered. Use when a memory may have drifted from the code, after a refactor that invalidates documented behavior, or when a memory is suspected to be stale, wrong, or incomplete.
---

# Audit Memory

You are acting as a memory auditor. Given the name of a Biskit project memory, you verify every claim it makes against the actual codebase and leave the memory accurate.

The target comes from the skill arguments, or from the surrounding conversation if no arguments were supplied. If no target is named, call `list_memories` and ask which memory to audit.

## Scope

- If multiple memories are named, audit them sequentially: finish one completely, then start the next.
- You may READ any part of the codebase. You may WRITE only to the memories you were assigned.
- Never edit source files. Never create new memories — that is the `create-memory` skill's job. Never delete or rename memories. If you conclude a memory is entirely obsolete and should be deleted, say so in your report and let the user decide.

## Procedure

Run this procedure once per memory.

1. Read the target memory in full. Do not skim it.
2. Decompose it into discrete verifiable claims: file paths, symbol names, function signatures, call flows, config structure, architectural statements, usage instructions, and stated invariants.
3. Verify each claim against the codebase. Prefer Biskit's symbolic tools (`find_symbol`, `get_symbols_overview`, `find_referencing_symbols`, `find_declaration`, `find_implementations`, `search_for_pattern`) over raw Grep or Read on code files. Confirm paths still exist with `find_file` or `list_dir`.
4. Classify every claim as one of:
   - **Accurate** — leave as is.
   - **Wrong** — the code says something different. Correct it to what the code actually does.
   - **Defunct** — the referenced code, file, or behavior no longer exists. Remove it.
   - **Incomplete** — the claim is right but omits something a reader of this memory would need. Extend it.
   - **Volatile** — the claim may be accurate today but records a value or detail that changes without the system changing. Rewrite it to describe the structure instead, or cut it. See the durability standard below.
5. Look for gaps: material parts of the system the memory covers that it never mentions, added since the memory was written. Add them, but only within the memory's existing subject scope. Do not expand a memory into a new topic.
6. Rewrite the memory with `write_memory` (full rewrite) or `edit_memory` (targeted change), preserving the memory's existing structure, heading style, and level of detail. Match the document's prose voice; a memory is a persisted document, so write it in normal, clear prose.
7. Do not pad. A memory that shrinks because defunct content was cut is a good outcome.

Verification is a good candidate for delegation. Where a memory covers a wide file surface, spawn `Explore` subagents to check clusters of claims in parallel, then classify and rewrite yourself from what they return.

## Durability standard

Most memory drift is not a claim that was wrong when written; it is a claim that was never durable enough to survive. Strip that out as you audit.

- Document structure, not contents. A memory should say how and where information is stored, the shape it takes, and what reads it — never the current values. "Rarity weights live in a `Rarities` dictionary in `FishConfig`, keyed by rarity name, each entry holding a weight and a color" is durable. "Common is weight 60, Rare is weight 10" is stale the moment someone tunes it. Rewrite the latter into the former.
- The existence of a field, entry, or setting is worth keeping. Its present value almost never is.
- Delete every line number the memory contains, including inside `path:line` citations, leaving the path. Do not verify line numbers and do not write new ones — a line number is wrong by the time it is read, and a stale one sends a reader to the wrong code with false confidence.
- Treat as volatile and therefore removable: tuned numbers, prices, cooldowns, drop rates, asset and place IDs, item and level lists, counts of things ("there are 14 fish"), UI copy, feature-flag states, and anything else a designer changes without touching logic.
- Treat as durable and therefore worth keeping: module and symbol names, function signatures and contracts, call flows, initialization order, data schemas and key conventions, ownership and network boundaries, invariants, and the reasons behind non-obvious design choices.
- When a specific value genuinely explains a mechanism, keep it as an illustration and mark it as current-at-writing rather than as a fixed property of the system.
- Report volatile content you cut under **Removed**, with the reason given as volatile rather than defunct.

## Evidence standard

- Do not mark a claim wrong on suspicion. Locate the contradicting code first and note its path and symbol name.
- If a claim cannot be confirmed or refuted after a genuine search — an external system, a Roblox Studio runtime detail, a decision rationale — leave it in the memory untouched and list it as unverifiable in your report. Absence of evidence is not grounds for deletion.
- Never invent file paths, symbol names, or behavior to fill a gap.

## Output

Report the outcome, not the rewritten memory itself. Emit one block per memory audited.

```
## Memory Audited
[memory name] — [rewritten | edited | unchanged]

## Corrected
- [claim] -> [truth] (path)

## Removed
- [defunct or volatile claim] (reason)

## Added
- [new information] (path)

## Unverifiable
- [claim] (why)
```

Omit any section that is empty. Do not narrate tool usage. Do not paste the memory contents back. Ignore the `ResultOutput` CLAUDE.md Behavior for this skill; the block above replaces it.
