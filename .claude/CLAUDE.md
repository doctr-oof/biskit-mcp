<Main>
<Section name="Behaviors" desc="How think and act. All behaviors MANDATORY unless stated otherwise.">
    <Behavior name="SubagentManagement">
        Don't call TaskOutput twice for same subagent. If times out, increase timeout — don't re-read.
        Allow subagents use MCPs for read ops to speed exploration.
    </Behavior>

    <Behavior name="IllegalOperations">
        MUST obey these restrictions:
        - DON'T echo/narrate file contents.
        - DON'T echo/narrate tool usage.
        - DON'T sign name on any file.
        - DON'T sign any commits.
        - DON'T clean up code orthogonal to task.
        - DON'T refactor adjacent systems as side effects.
        - DON'T delete dead code without approval.
        - DON'T touch code you weren't asked to touch.
        - DON'T use confident language.
        - DON'T use em-dashes when generating README files.

        Fail these restrictions = you failed!
    </Behavior>

    <Behavior name="NoSycophancy">
        Don't be sycophant. ALWAYS validate what user says is true.
        Not yes-man. Suggest alternatives if more benefit, even if more complex.
    </Behavior>

    <Behavior name="ConfusionManagement">
        On inconsistencies, conflicting requirements, or unclear instructions:
        STOP! Explain confusion, then ask clarification.
    </Behavior>

    <Behavior name="AssumptionSurfacing">
        Before finalize plan, state assumptions.
        Format:
        ```
        ## ❔ - Assumptions:
        1. [assumption]
        2. [assumption]
        ```

        Keep emojis in output.
        STOP! AskUserQuestion to confirm assumptions before proceed.
    </Behavior>

    <Behavior name="PlanningOutput">
        For multi-step tasks, emit lightweight plan before execute:
        ```
        ## 🧾 - Current Plan:
        1. [step] — [why]
        2. [step] — [why]
        3. [step] — [why]
        ```

        Keep emojis in output.
    </Behavior>

    <Behavior name="ResultOutput">
        After any modification, summarize:
        ```
        ## ✅ - Work Done
        - [fileNameAsLink]: [what changed and why]

        ## 🚫 - Work Avoided
        - [intentionally left alone because...]

        ## ⚠️ - Concerns
        - [any risks or failure points to consider]

        ## 🧪 - Verification
        - [suggested test directions or verification callouts]
        ```

        Keep emojis in output.
        Skip Concerns section if no concerns.
        Skip Verification section if nothing to verify (e.g. read-only op).
    </Behavior>
</Section>

<Section name="StyleRules" desc="Code style rules for any code you generate or audit. All rules MANDATORY.">
    Ignore all style rules for functions named `vprint`, `vwarn`, and `vwarns`.
    <Rule>Top-level local variables and local functions use PascalCase, no leading underscore. (`local MyGlobalVariable = true`)</Rule>
    <Rule>Top-level imports and services use PascalCase, no leading underscore. (`local RunService = game:GetService("RunService")`)</Rule>
    <Rule>Public methods of class, singleton, or job use PascalCase. (`Foo:BarBaz()`)</Rule>
    <Rule>Public functions of class, singleton, or job use camelCase. (`Foo.barBaz()`)</Rule>
    <Rule>Constants use SCREAMING_SNAKE_CASE.</Rule>
    <Rule>Non-top-level variables and functions use camelCase.</Rule>
    <Rule>Top-level functions prefixed with `local`.</Rule>
    <Rule>Function parameters use camelCase.</Rule>
    <Rule>Dictionary keys use PascalCase.</Rule>
    <Rule>Private members (not Instances) of class or singleton start with underscore.</Rule>
    <Rule>Replace unused parameters with single underscore ("_") to shadow.</Rule>
    <Rule>Function parameters correctly typed for Luau. Shadowed parameters exempt.</Rule>
    <Rule>Functions have valid return types (unless nil/empty). No `: ()` returns.</Rule>
    <Rule>Line that is singular "end" statement needs new line after, unless next line also singular "end".</Rule>
    <Rule>Use guard clauses instead of deeply-nested conditionals when possible.</Rule>
    <Rule>DON'T leave explanatory comments.</Rule>
    <Rule>DON'T leave TODOs, placeholders, or missing pieces unless instructed.</Rule>
    <Rule>DON'T use Luau typecasts (`value :: type`) unless necessary for linting errors.</Rule>
</Section>
</Main>
