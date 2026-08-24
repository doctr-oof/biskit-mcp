use std::borrow::Cow;

/// Line boundaries of one file, computed once and reused.
pub struct LineIndex<'a> {
    content: &'a str,
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

    /// Line count as `str::lines` counts them: a trailing newline closes the last line rather than opening an empty one.
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

    /// The file the index was built over, for callers that need the text behind a range.
    pub fn content(&self) -> &'a str {
        self.content
    }

    /// The 0-based line and column the byte at `offset` sits on.
    pub fn position_of(&self, offset: usize) -> (usize, usize) {
        let line = self.line_of(offset);
        let column = offset.saturating_sub(self.starts.get(line).copied().unwrap_or(0));
        (line, column)
    }

    /// The 0-based line the byte at `offset` sits on.
    pub fn line_of(&self, offset: usize) -> usize {
        match self.starts.binary_search(&offset) {
            Ok(index) => index,
            Err(index) => index.saturating_sub(1),
        }
    }

    /// `line` pulled back inside the file, so a caller that widened a range by a context window can report the line number it actually got.
    pub fn clamp_line(&self, line: usize) -> usize {
        line.min(self.len().saturating_sub(1))
    }

    /// Lines `from` through `to` inclusive, without their trailing line terminator.
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
    pub fn text(&self, from: usize, to: usize) -> Cow<'a, str> {
        let raw = self.slice(from, to);
        if raw.as_bytes().contains(&b'\r') {
            return Cow::Owned(raw.replace("\r\n", "\n"));
        }
        Cow::Borrowed(raw)
    }

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

/// The byte offset in `line` that an LSP `character` column points at.
///
/// LSP columns are UTF-16 code units unless `positionEncoding` was negotiated, which this crate
/// never does. The result is always a char boundary, and a column past the end of the line clamps
/// to its length, so the return value is always safe to slice with.
pub fn utf16_column_to_byte(line: &str, column: usize) -> usize {
    if line.is_ascii() {
        return column.min(line.len());
    }

    let mut units = 0usize;
    for (offset, character) in line.char_indices() {
        if units >= column {
            return offset;
        }
        units += character.len_utf16();
    }
    line.len()
}

/// The LSP `character` column of the byte at `offset` in `line`, the inverse of `utf16_column_to_byte`.
pub fn byte_to_utf16_column(line: &str, offset: usize) -> usize {
    if line.is_ascii() {
        return offset.min(line.len());
    }

    let mut units = 0usize;
    for (at, character) in line.char_indices() {
        if at >= offset {
            return units;
        }
        units += character.len_utf16();
    }
    units
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn utf16_columns_round_trip_through_byte_offsets() {
        for line in [
            "local Combat = {}",
            "local naïve = 1",
            "-- 日本語 comment\u{0}",
            "local emoji = \"😀\" -- tail",
            "",
        ] {
            let mut units = 0usize;
            for (offset, character) in line.char_indices() {
                assert_eq!(utf16_column_to_byte(line, units), offset, "{line:?}");
                assert_eq!(byte_to_utf16_column(line, offset), units, "{line:?}");
                units += character.len_utf16();
            }
            assert_eq!(utf16_column_to_byte(line, units), line.len(), "{line:?}");
            assert_eq!(byte_to_utf16_column(line, line.len()), units, "{line:?}");
        }
    }

    #[test]
    fn a_column_past_the_end_of_the_line_clamps_rather_than_panicking() {
        let line = "local naïve = 1";
        assert_eq!(utf16_column_to_byte(line, 9_999), line.len());
        assert_eq!(&line[utf16_column_to_byte(line, 9_999)..], "");
        assert_eq!(byte_to_utf16_column(line, 9_999), line.chars().count());
    }

    #[test]
    fn a_column_inside_a_surrogate_pair_lands_on_a_char_boundary() {
        let line = "😀ab";
        let at = utf16_column_to_byte(line, 1);
        assert!(line.is_char_boundary(at));
        assert_eq!(&line[at..], "ab");
    }
}
