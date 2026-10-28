//! `-- @lint_allow` lines (#639): a deliberate exception to one `design/*`
//! lint, for exactly the statement below.
//!
//! ```sql
//! -- @lint_allow design/many-series   (12 sources, fixed colours, on purpose)
//! SELECT ts ::XAXIS, source ::CATEGORY, gw ::AREACHART_STACKED FROM mix;
//! ```
//!
//! `dashboard --check` still lists the lint, as a note marked "allowed", so
//! the exception stays visible in review — but it no longer fails
//! `--strict`. Only `design/*` advisories can be allowed; the linter flags a
//! code outside `design/` (`lint/allow-not-design`) and one the statement
//! doesn't trigger (`lint/allow-unused`). A `-- @lint_allow` on the first
//! statement (the header block) also covers the script-wide
//! `design/too-many-panels`.

/// The lint codes after `-- @lint_allow` lines of one raw statement (comma-
/// or whitespace-separated; a `(…)` reason after them is ignored).
pub fn parse(raw: &str) -> Vec<String> {
    raw.lines()
        .filter_map(|l| l.trim().strip_prefix("--"))
        .filter_map(|l| l.trim().strip_prefix("@lint_allow"))
        .filter(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
        .flat_map(|rest| {
            rest.split('(')
                .next()
                .unwrap_or("")
                .split([',', ' ', '\t'])
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_codes() {
        assert_eq!(
            parse("-- @lint_allow design/many-series\nSELECT 1;"),
            ["design/many-series"]
        );
        assert_eq!(
            parse("-- @lint_allow design/many-series, design/pie-slices\n"),
            ["design/many-series", "design/pie-slices"]
        );
        assert_eq!(
            parse("-- @lint_allow design/many-series (12 sources on purpose)\n"),
            ["design/many-series"]
        );
        assert_eq!(
            parse("-- @lint_allow design/a\n-- @lint_allow design/b\nSELECT 1;"),
            ["design/a", "design/b"]
        );
    }

    #[test]
    fn ignores_other_lines() {
        assert!(parse("-- @lint_allowx design/a\n").is_empty());
        assert!(parse("-- @lint_allow\nSELECT 1;").is_empty());
        assert!(parse("-- lint_allow design/a\n").is_empty());
    }
}
