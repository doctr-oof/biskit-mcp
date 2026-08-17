use std::fmt;

/// An error that carries a recovery hint alongside its message.
///
/// The hint is deliberately kept out of `Display` so that it never leaks into a wrapping
/// `anyhow` context chain; the renderer pulls it back out with `anyhow::Error::downcast_ref`,
/// which searches the whole chain rather than only the outermost layer.
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

/// `bail!` with a recovery hint attached. The hint comes first, separated by `;`.
#[macro_export]
macro_rules! bail_hint {
    ($hint:expr; $($message:tt)*) => {
        return ::core::result::Result::Err($crate::errors::hinted(format!($($message)*), $hint))
    };
}

/// Renders a tool failure as prose the caller can act on, rather than a collapsed
/// `outer: inner: root` chain.
pub fn render(tool: &str, error: &anyhow::Error) -> String {
    let mut chain = error.chain().map(ToString::to_string);
    let headline = chain.next().unwrap_or_else(|| "unknown error".to_string());

    let mut causes = Vec::new();
    for cause in chain {
        // A context layer whose text already contains the layer below it adds nothing.
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
