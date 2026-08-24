use rmcp::model::CallToolResult;

use crate::errors;

pub(super) type ToolResult = Result<CallToolResult, String>;

pub(super) fn fail(tool: &'static str) -> impl Fn(anyhow::Error) -> String {
    move |error| errors::render(tool, &error)
}

pub(super) const OVERRUN_HINT: &str = "ask for less: narrow relative_path, lower max_matches, drop \
                            include_body, or raise tools.max_answer_chars in .biskit/settings.yml";

pub(super) fn truncate_at_char_boundary(value: &str, limit: usize) -> &str {
    if value.len() <= limit {
        return value;
    }
    let mut end = limit;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}
