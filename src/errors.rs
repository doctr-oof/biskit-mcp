use std::fmt;

/// An error that carries a recovery hint alongside its message.
#[derive(Debug)]
pub struct HintedError {
    message: String,
    hint: String,
}

impl HintedError {
    pub fn hint(&self) -> &str {
        &self.hint
    }
}

impl fmt::Display for HintedError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for HintedError {}

pub fn hinted(message: impl Into<String>, hint: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(HintedError {
        message: message.into(),
        hint: hint.into(),
    })
}

/// Restates an error under a different hint, for when the caller knows something the raiser did
/// not: what was rolled back, or what else the failure damaged.
pub fn rehinted(error: anyhow::Error, hint: impl Into<String>) -> anyhow::Error {
    let mut message = error.to_string();
    for cause in error.chain().skip(1) {
        let cause = cause.to_string();
        if !message.contains(&cause) {
            message.push_str(&format!("\ncaused by: {cause}"));
        }
    }
    hinted(message, hint)
}

macro_rules! bail_hint {
    ($hint:expr; $($message:tt)*) => {
        return ::core::result::Result::Err($crate::errors::hinted(format!($($message)*), $hint))
    };
}

pub(crate) use bail_hint;

/// Renders a tool failure as prose the caller can act on.
pub fn render(tool: &str, error: &anyhow::Error) -> String {
    let mut chain = error.chain().map(ToString::to_string);
    let headline = chain.next().unwrap_or_else(|| "unknown error".to_string());

    let mut causes = Vec::new();
    for cause in chain {
        if causes.last().is_some_and(|last| last == &cause) || headline == cause {
            continue;
        }
        causes.push(cause);
    }

    let mut rendered = format!("{tool} failed: {headline}");
    if !causes.is_empty() {
        rendered.push_str("\n\ncaused by:");
        for cause in causes {
            rendered.push_str(&format!("\n  - {cause}"));
        }
    }
    if let Some(hint) = error.downcast_ref::<HintedError>() {
        rendered.push_str(&format!("\n\nhint: {}", hint.hint()));
    }
    rendered
}

#[cfg(test)]
mod tests {
    use anyhow::Context;

    use super::*;

    #[test]
    fn renders_a_bare_error_as_one_line() {
        let error = anyhow::anyhow!("memory not found: notes");
        assert_eq!(
            render("read_memory", &error),
            "read_memory failed: memory not found: notes"
        );
    }

    #[test]
    fn a_rehinted_error_keeps_its_message_and_takes_the_new_hint() {
        let original = hinted("`wally install` failed", "fix what the output names");
        let rehinted = rehinted(original, "the wally.toml edit was rolled back");
        let rendered = render("add_wally_package", &rehinted);

        assert!(rendered.contains("add_wally_package failed: `wally install` failed"));
        assert!(rendered.contains("hint: the wally.toml edit was rolled back"));
        assert!(
            !rendered.contains("fix what the output names"),
            "the stale hint must not survive: {rendered}"
        );
    }

    #[test]
    fn a_rehinted_error_folds_its_causes_into_the_message() {
        let error = anyhow::anyhow!("connection reset").context("request failed: https://x/y");
        let rendered = render("add_wally_package", &rehinted(error, "try again"));

        assert!(rendered.contains("request failed: https://x/y"));
        assert!(rendered.contains("connection reset"));
    }

    #[test]
    fn appends_the_hint_of_a_hinted_error() {
        let error = hinted(
            "memory already exists: notes",
            "pass overwrite to replace it",
        );
        assert_eq!(
            render("create_memory", &error),
            "create_memory failed: memory already exists: notes\n\nhint: pass overwrite to replace it"
        );
    }

    #[test]
    fn recovers_a_hint_from_underneath_a_context_layer() {
        let error = Err::<(), _>(hinted("no such directory: src", "check the path"))
            .context("failed to list src")
            .unwrap_err();
        let rendered = render("list_dir", &error);

        assert!(rendered.starts_with("list_dir failed: failed to list src"));
        assert!(rendered.contains("\n  - no such directory: src"));
        assert!(rendered.ends_with("\n\nhint: check the path"));
    }

    #[test]
    fn lists_every_layer_of_a_context_chain() {
        let error = Err::<(), _>(anyhow::anyhow!("permission denied"))
            .context("failed to read foo.luau")
            .context("failed to scan the project")
            .unwrap_err();

        assert_eq!(
            render("search_for_pattern", &error),
            "search_for_pattern failed: failed to scan the project\n\ncaused by:\n  - failed to \
             read foo.luau\n  - permission denied"
        );
    }
}
