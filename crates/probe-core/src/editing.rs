//! Pure file-editing policy, salvaged from the archived TS
//! `file-mutation.ts`. BOM and line endings are preserved through an edit;
//! matching is exact with ambiguity rejection; a detected concurrent
//! modification is a typed failure the caller MUST handle — the archived
//! integration swallowed it and reported success, which is the defect this
//! module exists to make unrepresentable.

const BOM: char = '\u{feff}';

pub fn has_utf8_bom(bytes: &[u8]) -> bool {
    bytes.len() >= 3 && bytes[0] == 0xef && bytes[1] == 0xbb && bytes[2] == 0xbf
}

/// Strip every leading BOM character; report whether any was present.
pub fn split_bom(text: &str) -> (bool, &str) {
    let stripped = text.trim_start_matches(BOM);
    (stripped.len() != text.len(), stripped)
}

pub fn join_bom(text: &str, bom: bool) -> String {
    let (_, stripped) = split_bom(text);
    if bom {
        format!("{BOM}{stripped}")
    } else {
        stripped.to_string()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
}

pub fn normalize_line_endings(text: &str) -> String {
    text.replace("\r\n", "\n")
}

pub fn detect_line_ending(text: &str) -> LineEnding {
    if text.contains("\r\n") {
        LineEnding::CrLf
    } else {
        LineEnding::Lf
    }
}

pub fn convert_to_line_ending(text: &str, ending: LineEnding) -> String {
    let normalized = normalize_line_endings(text);
    match ending {
        LineEnding::Lf => normalized,
        LineEnding::CrLf => normalized.replace('\n', "\r\n"),
    }
}

/// Non-overlapping occurrence count.
pub fn count_occurrences(content: &str, search: &str) -> usize {
    if search.is_empty() {
        return 0;
    }
    let mut count = 0;
    let mut from = 0;
    while let Some(index) = content[from..].find(search) {
        count += 1;
        from += index + search.len();
    }
    count
}

/// The exact-match edit policy: 0 matches is an error, more than one without
/// `replace_all` demands more surrounding context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    EmptyOldString,
    NoMatch,
    AmbiguousMatch { count: usize },
}

impl EditError {
    /// The user-facing sentences, kept verbatim from the archived tool.
    pub fn message(&self) -> String {
        match self {
            EditError::EmptyOldString => "oldString is required".to_string(),
            EditError::NoMatch => {
                "Could not find oldString in the file. It must match exactly, including whitespace and indentation."
                    .to_string()
            }
            EditError::AmbiguousMatch { .. } => {
                "Found multiple exact matches for oldString. Provide more surrounding context or set replaceAll to true."
                    .to_string()
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditPlan {
    /// The full new file text with the original BOM and line endings restored.
    pub output: String,
    pub replacements: usize,
}

/// Plan an exact edit against the file's current text (BOM included as read).
/// Matching happens on LF-normalized text; the output restores the file's
/// detected line ending and BOM.
pub fn plan_exact_edit(
    current_text: &str,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
) -> Result<EditPlan, EditError> {
    if old_string.is_empty() {
        return Err(EditError::EmptyOldString);
    }
    let (bom, body) = split_bom(current_text);
    let ending = detect_line_ending(body);
    let normalized = normalize_line_endings(body);
    let old_normalized = normalize_line_endings(old_string);
    let new_normalized = normalize_line_endings(new_string);

    let count = count_occurrences(&normalized, &old_normalized);
    if count == 0 {
        return Err(EditError::NoMatch);
    }
    if count > 1 && !replace_all {
        return Err(EditError::AmbiguousMatch { count });
    }

    let replaced = if replace_all {
        normalized.replace(&old_normalized, &new_normalized)
    } else {
        normalized.replacen(&old_normalized, &new_normalized, 1)
    };
    let with_endings = convert_to_line_ending(&replaced, ending);
    Ok(EditPlan {
        output: join_bom(&with_endings, bom),
        replacements: if replace_all { count } else { 1 },
    })
}

/// A concurrent modification detected between read and write. Typed, and the
/// only way to get an `EditPlan` applied is to have checked it: callers hold
/// a `StaleContent` value, not a swallowed exception.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleContent;

pub fn verify_unchanged(expected: &[u8], actual: &[u8]) -> Result<(), StaleContent> {
    if expected == actual {
        Ok(())
    } else {
        Err(StaleContent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_crlf_and_bom_through_an_edit() {
        let original = "\u{feff}alpha\r\nbeta\r\n";
        let plan = plan_exact_edit(original, "beta", "gamma", false).unwrap();
        assert_eq!(plan.output, "\u{feff}alpha\r\ngamma\r\n");
        assert_eq!(plan.replacements, 1);
    }

    #[test]
    fn matches_across_line_ending_styles() {
        let original = "one\r\ntwo\r\nthree\r\n";
        let plan = plan_exact_edit(original, "one\ntwo", "1\n2", false).unwrap();
        assert_eq!(plan.output, "1\r\n2\r\nthree\r\n");
    }

    #[test]
    fn rejects_ambiguity_without_replace_all() {
        let err = plan_exact_edit("x x", "x", "y", false).unwrap_err();
        assert_eq!(err, EditError::AmbiguousMatch { count: 2 });
        let plan = plan_exact_edit("x x", "x", "y", true).unwrap();
        assert_eq!(plan.output, "y y");
        assert_eq!(plan.replacements, 2);
    }

    #[test]
    fn no_match_and_empty_old_are_typed_errors() {
        assert_eq!(plan_exact_edit("abc", "zzz", "y", false).unwrap_err(), EditError::NoMatch);
        assert_eq!(plan_exact_edit("abc", "", "y", false).unwrap_err(), EditError::EmptyOldString);
    }

    #[test]
    fn stale_content_is_a_value_not_an_exception() {
        assert!(verify_unchanged(b"same", b"same").is_ok());
        assert_eq!(verify_unchanged(b"a", b"b").unwrap_err(), StaleContent);
    }

    #[test]
    fn bom_detection_on_bytes() {
        assert!(has_utf8_bom(&[0xef, 0xbb, 0xbf, b'a']));
        assert!(!has_utf8_bom(b"abc"));
    }
}
