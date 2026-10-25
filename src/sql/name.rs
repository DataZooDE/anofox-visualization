//! `-- @name <name>` lines (#518): a stable, author-given name for what the
//! following statement draws — a panel, an input control, a `::GROUP` box, a
//! `::TAB`/`::SUBTAB` button, a `::VIEW` switcher, or a `::LABEL`/`::SECTION`
//! heading. The browser tags the element it builds with
//! `data-dp-name="<name>"` (`web/app.js`'s `tagName`), so a name doubles as a
//! stable e2e selector and, later, a guided tour's target (`-- @tour`, #518
//! PR B).
//!
//! ```sql
//! -- @name severity_kpi
//! SELECT max(severity) ::METRIC, 'Worst severity' ::LABEL FROM problems;
//! ```
//!
//! Parsed like [`super::shared`]'s `-- @shared`: comment lines directly above
//! the statement, read from the raw (unstripped) text `split_statements`
//! hands each statement — so a blank comment line between the `-- @name` and
//! the statement still keeps them attached (`split_statements` already
//! groups a statement's leading comments this way for `@requires`/`@shared`).
//! Unlike `@shared`, only one name applies per statement: the first
//! `-- @name` line with a non-empty value wins; a later one on the same
//! statement is ignored here (nothing needs flagging it — reusing a name on
//! two *statements* is what `name/duplicate` catches, in [`crate::lint`]).
//!
//! Whether the value is a well-formed identifier ([`valid`], `name/bad`),
//! reserved (`page`, `name/reserved`), unique on the dashboard
//! (`name/duplicate`) or sits on a statement that draws nothing
//! (`name/not-drawn`) are all checked by the linter, not here: this module
//! only extracts the raw text, so a malformed name still reaches
//! [`crate::sql::Panel::name`] and surfaces as a diagnostic instead of
//! silently vanishing.

/// The value of the first `-- @name <name>` line directly above a statement.
/// `None` when there's no such line, or every one is empty.
pub fn parse(raw: &str) -> Option<String> {
    raw.lines()
        .filter_map(|l| l.trim().strip_prefix("--"))
        .filter_map(|l| l.trim().strip_prefix("@name"))
        .filter(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
        .map(str::trim)
        .find(|s| !s.is_empty())
        .map(str::to_string)
}

/// Is `name` a well-formed panel name — `^[A-Za-z_][A-Za-z0-9_-]{0,63}$`
/// (ASCII, at most 64 characters total)?
pub fn valid(name: &str) -> bool {
    let mut cs = name.chars();
    match cs.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    name.len() <= 64 && cs.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_first_nonempty_name_line() {
        assert_eq!(parse("-- @name kpi\nSELECT 1;"), Some("kpi".to_string()));
        assert_eq!(parse("-- @name  kpi  \n"), Some("kpi".to_string()));
        // A second `@name` line is ignored (the first wins).
        assert_eq!(
            parse("-- @name first\n-- @name second\nSELECT 1;"),
            Some("first".to_string())
        );
        // An empty line doesn't count; the next one does.
        assert_eq!(
            parse("-- @name \n-- @name kpi\nSELECT 1;"),
            Some("kpi".to_string())
        );
    }

    #[test]
    fn no_at_name_line_is_none() {
        assert_eq!(parse("-- plain comment\nSELECT 1;"), None);
        assert_eq!(parse("-- @namex kpi\n"), None);
        assert_eq!(parse("-- @name\nSELECT 1;"), None);
        assert_eq!(parse("-- @name \nSELECT 1;"), None);
    }

    #[test]
    fn a_blank_line_between_the_comment_and_the_statement_still_attaches() {
        // split_statements keeps leading comments attached to the following
        // statement across a blank line, as it does for `@requires`/`@shared`
        // (see `src/sql.rs`'s split_statements tests) — this only checks that
        // `parse` itself doesn't care where the blank line sits.
        assert_eq!(parse("-- @name kpi\n\nSELECT 1;"), Some("kpi".to_string()));
    }

    #[test]
    fn valid_names() {
        assert!(valid("kpi"));
        assert!(valid("_kpi"));
        assert!(valid("kpi_1"));
        assert!(valid("kpi-1"));
        assert!(valid("a"));
        assert!(valid(&"a".repeat(64)));
    }

    #[test]
    fn invalid_names() {
        assert!(!valid(""));
        assert!(!valid("1kpi")); // can't start with a digit
        assert!(!valid("-kpi")); // can't start with a hyphen
        assert!(!valid("kpi name")); // no spaces
        assert!(!valid("kpi.name")); // no dots
        assert!(!valid(&"a".repeat(65))); // too long
    }
}
