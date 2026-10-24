//! `-- @shared` lines (#398): a deliberately shared `::ROWSELECT` / `input`
//! variable name.
//!
//! ```sql
//! -- @shared item
//! SELECT 'single'::ROWSELECT;
//! SELECT name AS item, id * 2 AS "Doubled" ::TABLE FROM items;
//! ```
//!
//! By default two `::ROWSELECT`s (or a `::ROWSELECT` and an `input`) writing
//! the same variable are flagged (`interaction/rowselect-shared-name` /
//! `interaction/name-clash`): the browser can't tell a deliberate fan-in
//! (several registers driving one detail panel) or hand-off (a dropdown and
//! a row click that either may set) from an accidental alias. A `-- @shared`
//! comment directly above the directive/input statement documents the
//! intent next to the panel, so `dashboard --check --strict` skips those two
//! warnings for exactly the named variables — every other clash still is
//! flagged. A name listed here that this statement doesn't itself set (a
//! typo, or the wrong column) is flagged on its own
//! (`interaction/shared-unused`), so it never silently disables nothing.

/// The names after `-- @shared` lines of one raw statement (comma- or
/// whitespace-separated, repeats kept in the order written).
pub fn parse(raw: &str) -> Vec<String> {
    raw.lines()
        .filter_map(|l| l.trim().strip_prefix("--"))
        .filter_map(|l| l.trim().strip_prefix("@shared"))
        .filter(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
        .flat_map(|rest| rest.split([',', ' ', '\t']))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_one_or_more_comma_or_space_separated_names() {
        assert_eq!(parse("-- @shared item\nSELECT 1;"), ["item"]);
        assert_eq!(parse("-- @shared item, product\n"), ["item", "product"]);
        assert_eq!(parse("-- @shared item product\n"), ["item", "product"]);
        assert_eq!(
            parse("-- @shared item,  product , region\n"),
            ["item", "product", "region"]
        );
    }

    #[test]
    fn no_at_shared_line_is_empty() {
        assert!(parse("-- plain comment\nSELECT 1;").is_empty());
        assert!(parse("-- @sharedx item\n").is_empty());
    }

    #[test]
    fn empty_at_shared_names_nothing() {
        assert!(parse("-- @shared\nSELECT 1;").is_empty());
        assert!(parse("-- @shared \nSELECT 1;").is_empty());
    }

    #[test]
    fn two_lines_accumulate() {
        assert_eq!(
            parse("-- @shared item\n-- @shared product\nSELECT 1;"),
            ["item", "product"]
        );
    }
}
