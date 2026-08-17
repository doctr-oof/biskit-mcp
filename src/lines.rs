use std::borrow::Cow;

/// Line boundaries of one file, computed once and reused.
///
/// Both the pattern search and the symbol renderer need to turn byte offsets and line numbers into
/// text. Each used to run `content.lines().collect::<Vec<&str>>()` at the point of use, which
/// rebuilds a vector over the whole file for every match or every rendered symbol. Building the
/// boundaries once per file and slicing the original string keeps the per-use cost proportional to
/// the snippet rather than to the file.
pub struct LineIndex<'a> {
    content: &'a str,
    /// Byte offset each line begins at. Carries one trailing entry when the file ends with a
    /// newline, which `len` accounts for rather than removing, so `end_of` can rely on it.
    starts: Vec<usize>,
}

impl<'a> LineIndex<'a> {
    pub fn new(content: &'a str) -> Self {
        let mut starts = Vec::with_capacity(content.len() / 32 + 1);
        starts.push(0);
        for offset in memchr::memchr_iter(b'\n', content.as_bytes()) {
            starts.push(offset + 1);
        }
        Self { content, starts }
    }

    /// Line count as `str::lines` counts them: a trailing newline closes the last line rather than
    /// opening an empty one.
    pub fn len(&self) -> usize {
        if self.content.is_empty() {
            return 0;
        }
        if self.content.ends_with('\n') {
            return self.starts.len() - 1;
        }
        self.starts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The 0-based line the byte at `offset` sits on.
    pub fn line_of(&self, offset: usize) -> usize {
        match self.starts.binary_search(&offset) {
            Ok(index) => index,
            Err(index) => index.saturating_sub(1),
        }
    }

    /// `line` pulled back inside the file, so a caller that widened a range by a context window
    /// can report the line number it actually got.
    pub fn clamp_line(&self, line: usize) -> usize {
        line.min(self.len().saturating_sub(1))
    }

    /// Lines `from` through `to` inclusive, without their trailing line terminator.
    ///
    /// Both ends are clamped into the file, and a `from` past the end yields the empty string,
    /// matching what slicing a collected line vector used to do.
    pub fn slice(&self, from: usize, to: usize) -> &'a str {
        if self.is_empty() || from >= self.len() {
            return "";
        }
        let to = self.clamp_line(to);
        if from > to {
            return "";
        }
        &self.content[self.starts[from]..self.end_of(to)]
    }

    /// `slice`, with CRLF terminators folded to LF.
    ///
    /// `str::lines` drops the carriage return, so a snippet joined from it never carried one;
    /// borrowing the file's own bytes would otherwise start leaking `\r` into tool output on
    /// files written on Windows.
    pub fn text(&self, from: usize, to: usize) -> Cow<'a, str> {
        let raw = self.slice(from, to);
        if raw.as_bytes().contains(&b'\r') {
            return Cow::Owned(raw.replace("\r\n", "\n"));
        }
        Cow::Borrowed(raw)
    }

    /// Byte offset one past the last character of `line`, excluding its line terminator.
    fn end_of(&self, line: usize) -> usize {
        let Some(next) = self.starts.get(line + 1) else {
            return self.content.len();
        };
        let without_newline = *next - 1;
        if self.content.as_bytes()[..without_newline].last() == Some(&b'\r') {
            return without_newline - 1;
        }
        without_newline
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The index has to agree with `str::lines` everywhere, because that is what every caller
    /// used before it existed.
    fn assert_matches_lines(content: &str) {
        let index = LineIndex::new(content);
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(index.len(), lines.len(), "line count for {content:?}");

        for from in 0..lines.len() {
            for to in from..lines.len() {
                assert_eq!(
                    index.text(from, to),
                    lines[from..=to].join("\n"),
                    "slice {from}..={to} of {content:?}"
                );
            }
        }
    }

    #[test]
    fn slices_agree_with_str_lines() {
        assert_matches_lines("");
        assert_matches_lines("one");
        assert_matches_lines("one\n");
        assert_matches_lines("one\ntwo");
        assert_matches_lines("one\ntwo\n");
        assert_matches_lines("one\r\ntwo\r\n");
        assert_matches_lines("one\r\ntwo\nthree");
        assert_matches_lines("\n\n\n");
        assert_matches_lines("local Service = {}\n\nfunction Service:run()\n    return 1\nend\n");
    }

    #[test]
    fn out_of_range_requests_are_clamped() {
        let index = LineIndex::new("one\ntwo\nthree\n");
        assert_eq!(index.slice(0, 99), "one\ntwo\nthree");
        assert_eq!(index.slice(2, 99), "three");
        assert_eq!(index.slice(3, 4), "");
        assert_eq!(index.slice(2, 1), "");

        let empty = LineIndex::new("");
        assert_eq!(empty.slice(0, 0), "");
        assert!(empty.is_empty());
    }

    #[test]
    fn offsets_resolve_to_the_line_that_holds_them() {
        let content = "alpha\nbeta\ngamma";
        let index = LineIndex::new(content);

        assert_eq!(index.line_of(0), 0);
        assert_eq!(index.line_of(4), 0);
        assert_eq!(
            index.line_of(5),
            0,
            "the newline belongs to the line it ends"
        );
        assert_eq!(index.line_of(6), 1);
        assert_eq!(index.line_of(content.find("gamma").unwrap()), 2);
        assert_eq!(index.line_of(content.len()), 2);
    }

    #[test]
    fn carriage_returns_are_dropped_only_where_they_terminate_a_line() {
        let index = LineIndex::new("a\r\nb\rc\n");
        assert_eq!(index.text(0, 1), "a\nb\rc");
        assert!(matches!(index.text(0, 0), Cow::Borrowed("a")));
    }
}
