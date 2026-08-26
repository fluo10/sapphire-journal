use std::path::PathBuf;

use grain_id::GrainId;
use schemars::JsonSchema;
use serde::Deserialize;

/// A reference to a journal entry — a filesystem path, a GrainId, or a title.
///
/// This is the canonical input type for commands that operate on a single entry
/// (show, fix, remove, etc.).  Parse raw CLI user input with [`EntryRef::parse`],
/// then resolve it to a concrete [`PathBuf`] via [`ops::resolve_entry`].
///
/// # Syntax (CLI)
///
/// | Input form              | Resolved as     |
/// |-------------------------|-----------------|
/// | `@abc1234`              | `Id(GrainId)` |
/// | `path/to/file.md`       | `Path(...)`     |
/// | `./relative.md`         | `Path(...)`     |
/// | `~/absolute.md`         | `Path(...)`     |
/// | `anything_else`         | `Title(...)`    |
///
/// The `@` prefix is required for IDs to avoid ambiguity with titles that
/// happen to be 7 alphanumeric characters.  If the part after `@` cannot be
/// parsed as a valid [`GrainId`], the `@` is treated as part of the string
/// and the usual path/title heuristics apply.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EntryRef {
    /// A filesystem path to the entry file.
    Path(PathBuf),
    /// A fully-parsed GrainId (the `@` prefix has been stripped and validated).
    Id(GrainId),
    /// An exact entry title (case-sensitive).
    Title(String),
}

impl EntryRef {
    /// Classify a raw CLI string as a path, an ID, or a title.
    ///
    /// - Starts with `@` **and** the remainder parses as a [`GrainId`]
    ///   → [`EntryRef::Id`].
    /// - Contains `/` or `\`, starts with `.` or `~`, or ends with `.md`
    ///   → [`EntryRef::Path`].
    /// - Anything else (including `@foo` where `foo` is not a valid GrainId)
    ///   → [`EntryRef::Title`].
    pub fn parse(s: &str) -> Self {
        if let Some(rest) = s.strip_prefix('@') {
            // grain-id 0.15's `GrainId::from_str` checks `s.len() == 7` in
            // *bytes* and then makes seven unconditional `chars().next()`
            // calls; a non-ASCII string that happens to be exactly 7 bytes
            // (e.g. "日本1": 3 + 3 + 1) is fewer than 7 characters, so the
            // fourth `.unwrap()` panics instead of returning `None`. This
            // guards an *upstream panic*, not just an invalid id — `rest`
            // failing to parse is already handled by falling through below,
            // so don't delete this as redundant with that. Crockford base32
            // is ASCII-only anyway, so this can never reject a real id.
            if rest.is_ascii() {
                if let Ok(id) = rest.parse::<GrainId>() {
                    return EntryRef::Id(id);
                }
            }
            // Invalid GrainId after `@` — fall through to path/title heuristics.
        }
        if s.contains('/')
            || s.contains(std::path::MAIN_SEPARATOR)
            || s.starts_with('.')
            || s.starts_with('~')
            || s.ends_with(".md")
        {
            EntryRef::Path(PathBuf::from(s))
        } else {
            EntryRef::Title(s.to_owned())
        }
    }
}

impl From<&str> for EntryRef {
    fn from(s: &str) -> Self {
        Self::parse(s)
    }
}

impl From<String> for EntryRef {
    fn from(s: String) -> Self {
        Self::parse(&s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "日本1" is 3 + 3 + 1 = 7 bytes but only 3 characters. grain-id 0.15's
    /// `GrainId::from_str` checks `s.len() == 7` in *bytes* and then does
    /// seven unconditional `chars().next().unwrap()` calls, so this string
    /// panics inside `rest.parse::<GrainId>()` without the non-ASCII guard.
    /// This is the CLI-facing counterpart to `dedupe::entry_id`'s guard
    /// (Task 9) — `EntryRef::parse` is what a raw `sapphire-journal entry
    /// show "@..."` argument goes through.
    #[test]
    fn parse_at_seven_byte_non_ascii_falls_through_to_title_instead_of_panicking() {
        let entry = EntryRef::parse("@日本1");
        assert!(
            matches!(entry, EntryRef::Title(ref t) if t == "@日本1"),
            "expected the documented fall-through to Title for an invalid id after `@`, got {entry:?}"
        );
    }
}
