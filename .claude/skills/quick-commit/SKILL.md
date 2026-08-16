---
name: quick-commit
description: Scans all pending git changes and commits them (no push) using the conventional commits format.
---

# Quick Commit
## Instructions
Thoroughly analyze all pending git changes, then commit them using the conventional commits format.
Do not, under any circumstances, sign these commits or co-author them (including signing them in the commit message).
Do not push your changes unless requested - commit only.
Do not undo previous commits (no `git reset`) unless explicitly requested by the user.
Do not use patch files to make commits over-granular.

## Workflow
This is the step-by-step workflow process to follow. Do not deviate.

### Step 1: Verify Branch
Verify that the current branch name is not "main", "master", or "dev". If it is, STOP! AskUserQuestion verbatim "Main branch detected! How do you want to proceed?" with the following options verbatim:
- Commit to main
- Create new branch

Also AskUserQuestion verbatim "Push changes after I commit?" with the following options verbatim:
- Yes, push
- No, don't push

The user can cancel the operation entirely via pressing ESC. You don't need to tell them this.

If the current branch is safe, proceed to Step 2.

### Step 2: Analyze Changes
Analyze all pending git changes, and determine how to classify and group each commit.

If the user chose "Create new branch" in Step 1: use the context you've gathered to determine a branch name, then create and check out to that branch before proceeding.

### Step 3: Generate Commits
Generate commit messages for each commit, then commit them. Use the conventional commit format 1:1 with one exception: the "scope" of each commit (if supplied) must be in PascalCase (good: "feat(SubSystem)", bad: "feat(sub-system)").

Note: if possible/available, use caveman plugin to simplify commit messages.

### Step 4: Push Changes (if requested)
If the user asked for you to push the committed changes, push them.

### Step 5: Report
Once done, echo back to the user exactly what commits you generated (including the full commit message for each commit). Reference the template output format below for examples:
```
`0sf367s` - **feat:** added this feature
`874679f` - **fix(Scope):** fixed bug that crashed everything
```

Ignore the `ResultOutput` CLAUDE.md Behavior ONLY for this final step.

!nomem
