//! Masking (plan-ai-assistant.md §12.7, security review M6): best-effort
//! pseudonymisation of the values Ask sends to the model provider. It is
//! minimisation, not a guarantee — `-- @ask skip` is the tool for a panel
//! that must not leave.
//!
//! - A masked value becomes a sentinel token, `⟦P4⟧` (`E` an e-mail, `T` a
//!   phone number, `I` an IBAN, `P` anything else), which can't occur in
//!   data: a result that already holds `⟦` or `⟧` is not sent at all.
//! - The map is per conversation, built only from this viewer's results,
//!   and dropped with the conversation: tokens are never stable across
//!   viewers.
//! - Translation is exact-token both ways: tokens in the model's answer and
//!   in its `get_panel` vars become real values; real values the map knows
//!   become tokens in the question, the view-state vars and error strings.

use std::collections::HashMap;

use super::config::Mask;

pub const OPEN: char = '⟦';
pub const CLOSE: char = '⟧';

/// Does a column name suggest personal data? (Shared with the linter.)
pub use crate::sql::ask::personal_column;

fn is_email(s: &str) -> bool {
    let s = s.trim();
    match s.split_once('@') {
        Some((l, d)) => {
            !l.is_empty()
                && !s.contains(char::is_whitespace)
                && d.contains('.')
                && !d.starts_with('.')
                && !d.ends_with('.')
        }
        None => false,
    }
}

fn is_phone(s: &str) -> bool {
    let s = s.trim();
    let digits = s.chars().filter(char::is_ascii_digit).count();
    (8..=15).contains(&digits)
        && s.chars()
            .all(|c| c.is_ascii_digit() || " +-/()".contains(c))
        && (s.starts_with('+') || s.starts_with('0') || s.starts_with('('))
}

fn is_iban(s: &str) -> bool {
    let t: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    t.len() >= 15
        && t.len() <= 34
        && t.chars().take(2).all(|c| c.is_ascii_uppercase())
        && t.chars().skip(2).take(2).all(|c| c.is_ascii_digit())
        && t.chars().skip(4).all(|c| c.is_ascii_alphanumeric())
}

/// The token kind of a value that looks personal by itself, if it does.
fn value_kind(s: &str) -> Option<char> {
    if is_email(s) {
        Some('E')
    } else if is_iban(s) {
        Some('I')
    } else if is_phone(s) {
        Some('T')
    } else {
        None
    }
}

/// A result holding the sentinel characters can't be masked safely.
pub fn has_sentinel(s: &str) -> bool {
    s.contains(OPEN) || s.contains(CLOSE)
}

/// One conversation's map.
#[derive(Debug, Default, Clone)]
pub struct Masker {
    to_token: HashMap<String, String>,
    to_real: HashMap<String, String>,
    next: u32,
}

impl Masker {
    pub fn new() -> Masker {
        Masker::default()
    }

    pub fn is_empty(&self) -> bool {
        self.to_real.is_empty()
    }

    /// The token for `real` (a new one the first time).
    fn token(&mut self, real: &str, kind: char) -> String {
        if let Some(t) = self.to_token.get(real) {
            return t.clone();
        }
        self.next += 1;
        let t = format!("{OPEN}{kind}{}{CLOSE}", self.next);
        self.to_token.insert(real.to_string(), t.clone());
        self.to_real.insert(t.clone(), real.to_string());
        t
    }

    /// Should column `name` be masked as a whole under `mode` (plus the
    /// panel's own `-- @ask mask col,…` list)?
    pub fn masks_column(mode: &Mask, name: &str, extra: &[String]) -> bool {
        let listed = |l: &[String]| l.iter().any(|c| c.eq_ignore_ascii_case(name.trim()));
        match mode {
            Mask::Off => false,
            Mask::Columns(cols) => listed(cols) || listed(extra),
            Mask::Auto => listed(extra) || personal_column(name),
        }
    }

    /// Mask one text cell of column `name`. `whole`: the column is masked as
    /// a whole. Err when the value holds a sentinel character.
    pub fn cell(&mut self, mode: &Mask, whole: bool, value: &str) -> Result<String, String> {
        if has_sentinel(value) {
            return Err("a value holds the masking sentinel".into());
        }
        if value.trim().is_empty() || *mode == Mask::Off {
            return Ok(value.to_string());
        }
        if whole {
            return Ok(self.token(value, value_kind(value).unwrap_or('P')));
        }
        if *mode == Mask::Auto {
            if let Some(k) = value_kind(value) {
                return Ok(self.token(value, k));
            }
        }
        Ok(value.to_string())
    }

    /// Real values the map knows → their tokens (the question, view-state
    /// vars, error strings), longest first so a value inside another isn't
    /// split.
    pub fn mask_text(&self, s: &str) -> String {
        let mut reals: Vec<&String> = self.to_token.keys().filter(|r| r.len() >= 2).collect();
        reals.sort_by_key(|r| std::cmp::Reverse(r.len()));
        let mut out = s.to_string();
        for r in reals {
            if out.contains(r.as_str()) {
                out = out.replace(r.as_str(), &self.to_token[r]);
            }
        }
        out
    }

    /// Tokens → real values (the answer, `get_panel` vars, citations). An
    /// unknown token stays as it is.
    pub fn unmask_text(&self, s: &str) -> String {
        if !s.contains(OPEN) {
            return s.to_string();
        }
        let mut out = String::with_capacity(s.len());
        let mut rest = s;
        while let Some(i) = rest.find(OPEN) {
            out.push_str(&rest[..i]);
            let after = &rest[i..];
            match after.find(CLOSE) {
                Some(j) => {
                    let tok = &after[..j + CLOSE.len_utf8()];
                    match self.to_real.get(tok) {
                        Some(real) => out.push_str(real),
                        None => out.push_str(tok),
                    }
                    rest = &after[j + CLOSE.len_utf8()..];
                }
                None => {
                    out.push_str(after);
                    rest = "";
                }
            }
        }
        out.push_str(rest);
        out
    }

    /// Does `s` still hold a token (a pseudonym that must not reach the
    /// viewer)?
    pub fn holds_token(&self, s: &str) -> bool {
        self.to_real.keys().any(|t| s.contains(t.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_personal_columns_in_english_and_german() {
        for n in [
            "name",
            "Customer",
            "customer_name",
            "E-Mail",
            "phone",
            "Kunde",
            "Mitarbeiter",
            "Ansprechpartner",
            "Straße",
            "Telefon",
            "Personalnummer",
            "customerName",
        ] {
            assert!(personal_column(n), "{n}");
        }
        for n in [
            "region",
            "Revenue",
            "Umsatz",
            "Region",
            "sku",
            "on_time_pct",
            "Stock value",
        ] {
            assert!(!personal_column(n), "{n}");
        }
    }

    #[test]
    fn detects_values() {
        assert_eq!(value_kind("ada@example.com"), Some('E'));
        assert_eq!(value_kind("DE89 3704 0044 0532 0130 00"), Some('I'));
        assert_eq!(value_kind("+49 30 1234567"), Some('T'));
        assert_eq!(value_kind("0171/2345678"), Some('T'));
        for v in ["EU", "12345", "2024-01-01", "SKU-033", "1,234.5"] {
            assert_eq!(value_kind(v), None, "{v}");
        }
    }

    #[test]
    fn round_trip_and_sentinel() {
        let mut m = Masker::new();
        let a = m.cell(&Mask::Auto, true, "Ada Lovelace").unwrap();
        let b = m.cell(&Mask::Auto, false, "ada@example.com").unwrap();
        assert_eq!(a, "⟦P1⟧");
        assert_eq!(b, "⟦E2⟧");
        assert_eq!(
            m.cell(&Mask::Auto, true, "Ada Lovelace").unwrap(),
            a,
            "stable within one map"
        );
        assert_eq!(m.cell(&Mask::Auto, false, "EU").unwrap(), "EU");
        assert_eq!(
            m.mask_text("Why did Ada Lovelace order less?"),
            "Why did ⟦P1⟧ order less?"
        );
        assert_eq!(
            m.unmask_text("⟦P1⟧ (⟦E2⟧) and ⟦P9⟧"),
            "Ada Lovelace (ada@example.com) and ⟦P9⟧"
        );
        assert!(m.cell(&Mask::Auto, false, "x⟦P1⟧").is_err());
        assert!(m.holds_token("see ⟦P1⟧"));
        // off: nothing changes
        assert_eq!(Masker::new().cell(&Mask::Off, true, "Ada").unwrap(), "Ada");
    }

    #[test]
    fn column_modes() {
        assert!(Masker::masks_column(&Mask::Auto, "Kunde", &[]));
        assert!(!Masker::masks_column(&Mask::Auto, "region", &[]));
        assert!(Masker::masks_column(
            &Mask::Auto,
            "region",
            &["Region".into()]
        ));
        let cols = Mask::Columns(vec!["supplier".into()]);
        assert!(Masker::masks_column(&cols, "Supplier", &[]));
        assert!(!Masker::masks_column(&cols, "name", &[]));
        assert!(!Masker::masks_column(&Mask::Off, "name", &["name".into()]));
    }
}
