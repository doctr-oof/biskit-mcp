---
name: code-audit
description: Audits code according to user preferences, checking for things like style violations, bugs, memory leaks, and more.
---

!nomem

# Code Audit
## Summary
Thoroughly analyze .lua(u) files for style violations, naming issues, missing types, duplicate code, dead code, memory leaks, security vulnerabilities, etc.

## Scanning Process
This is the step-by-step process to follow. Do not deviate.

### Step 1: Gather Target Files
AskUserQuestion verbatim "What file(s) do you want audited?" with the following choices:
- ALL Git Modifications
- Unstaged Git Modifications

The user can use the default "Other" field to specify certain files or systems.

### Step 2: Confirm Style Audit
AskUserQuestion verbatim "What should this audit cover?" with the following choices verbatim (multiple choice):
- Bugs and Stability
- Code Styling

### Step 3: Core Audit
You will now perform the core audit on all target files in a specific order of audit categories, using fresh subagents for each review category to prevent context drift.

Subagents should only report back their discoveries, and should not provide suggestions on how to address/fix them. Append "!nomem" to each subagent's prompt.

Subagents should report their findings in the following table format:
```
| File Path | Category | Summary |
|-|-|-|
| {{FILE_PATH}} | {{CATEGORY_NAME}} | {{ISSUE_SUMMARY}} |
```

Any audit step suffixed with "[STYLE]" should be skipped if the user did NOT select "Code Styling" in Step 2.

Any audit step suffixed with "[STABILITY]" should be skipped if the user did NOT select "Bugs and Stability" in Step 2.

#### Step 3.1: Audit Security Vulnerabilities (P0) [STABILITY]
Investigate for any potential exploit possibilities in which a client-side user can maliciously influence server-side functionality due to lack of rate limiting, data verification, owner verification, etc.

#### Step 3.2: Audit Showstoppers (P0) [STABILITY]
Investigate for any potential bugs that - if encountered - could potentially render a feature or component non-functional without any way to recover (thus potentially soft or hard-locking a user).

#### Step 3.3: Audit Performance (P1) [STABILITY]
Identify any memory leaks or performance deficits that can occur due to poor data or object management or lack of cleanup.

#### Step 3.4: Audit LSP (P1) [STABILITY]
If you have access to LSP diagnostics through an MCP, call them out accordingly.

#### Step 3.5: Audit General Bugs (P1) [STABILITY]
Thoroughly analyze and identify uncaught logic errors, unreachable code, or anything that can lead to bugs at runtime.

#### Step 3.6: Audit Unfinished Work (P2) [STABILITY]
Identify any code or comments that are marked with "TODO", "TEMP" or similar. Also look for any code that appears to be incomplete/unfinished.

#### Step 3.7: Audit Dead Code (P3) [STABILITY]
Call out any code that is not referenced anywhere else in the codebase. This includes entire modules, public and private members, constants, variables, imports, and Roblox service declarations.

#### Step 3.8: Audit Code Style (P3) [STYLE]
Search for any violation(s) of this project's "StyleRules" project rule in CLAUDE.md. All rules are mandatory and all violations must be noted, no exceptions (unless listed in CLAUDE.md).

NEVER edit or consolidate the style rules when giving them to subagents. Also give the subagents the full rule list as it is in CLAUDE.md.

### Step 4: Sanity Check
You yourself must now perform a sanity check on all audit results from the subagents. Go issue-by-issue and verify its validity, discarding any false-positives before generating a report. Do not use subagents for the sanity check.

### Step 5: Generate Report
Generate a markdown report file in this project's `.claude/audits` folder (create one if it doesn't exist). Base the report off of the template file (`./report-template.md`).

Ensure the following organizational and formatting criteria are met when generating the report:
- Issues are grouped by file.
- Each issue group is sorted from highest -> lowest severity (P0 -> P1 -> P2 -> P3).
- Each issue has a unique numerical identifier. No letters, symbols, or leading zeroes.
- When mentioning filenames, use a link (relative to audit file path) (example: `../Folder/Module.luau`).
- When mentioning line numbers, use a link (relative to the audit file path) (example: `../Folder/Module.luau#L123`).
- When mentioning line ranges, use a link (relative to the audit file path) pointing to the START of the line range (example: `../Folder/Module.luau#L10`).

The name of the file should be the current date and 24-hour timestamp as follows: `{monthNameAbbreviated}_{dayNumber}_{yearNumber}_{24hourTimeWithSeconds}.md`. Example: `May_11_2026_094523.md`.

DO NOT ECHO/NARRATE YOUR AUDIT RESULTS IN REAL TIME! SAVE THAT FOR THE REPORT!

### Step 6: Wait For User
After generating the report file, STOP! Say the following to the user verbatim:
```
## Report Generated: {reportFileNameWithClickableLink}

You may now view the report. You can also have me fix them by file, category, or ID simply by asking. Examples:
- `Fix issue 3`
- `Fix issues 2, 5, and 17`
- `Fix all issues in MyModule.luau`
- `Fix all Security Vulnerabilities`
```
