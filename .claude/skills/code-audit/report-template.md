# Audit Summary

| File | # P0 | # P1 | # P2 | # P3 |
|-|-|-|-|-|
| {{FILE_NAME_WITH_LINK}} | {{NUM_CRITICAL_RESULTS}} | {{NUM_HIGH_RESULTS}} | {{NUM_MEDIUM_RESULTS}} | {{NUM_LOW_RESULTS}} |

<br/>

# Audit Details (Grouped by File)

{{#EACH_FILE}}
## File: {{FILE_PATH}}
| ID | Category | Location | Summary |
|-|-|-|-|
| {{UNIQUE_BUG_ID}} | {{CATEGORY_NAME_WITH_SEVERITY_IN_PARENTHESES}} | {{LINE_NUMBER_OR_RANGE_WITH_LINK}} | {{ISSUE_SUMMARY}} |

<br/>
{{/EACH_FILE}}
