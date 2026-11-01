//! Citations (plan-ai-assistant.md §12.3, security review M4): a citation is
//! *verified* only when the panel was read in this conversation and a row
//! of that result matches every key the model gave **and** holds the cited
//! value in the cited column — the right value in the wrong row is not
//! verified. The client builds a chip's text from the verified cell, never
//! from the model's label.

use serde_json::{json, Map, Value};

/// A panel result read by `get_panel`, with real (unmasked) values keyed by
/// the column's display name.
#[derive(Debug, Clone)]
pub struct Read {
    pub panel: usize,
    /// The vars of the `get_panel` call (real values).
    pub vars: Map<String, Value>,
    pub columns: Vec<String>,
    pub rows: Vec<Map<String, Value>>,
}

/// What the model cited, after unmasking.
#[derive(Debug, Clone, Default)]
pub struct Claim {
    pub panel: usize,
    pub label: String,
    pub column: Option<String>,
    pub value: Option<String>,
    pub row: Vec<(String, String)>,
}

/// A citation as the client gets it.
#[derive(Debug, Clone, PartialEq)]
pub struct Citation {
    pub panel: usize,
    pub verified: bool,
    /// The verified cell (column, the result's own value), when there is one.
    pub column: Option<String>,
    pub value: Option<Value>,
    /// The matched row's key cells, as the result holds them.
    pub row: Vec<(String, Value)>,
    /// The vars the panel was read with.
    pub vars: Map<String, Value>,
    /// The model's own label: shown, if at all, as unverified text.
    pub label: String,
}

impl Citation {
    /// As the client gets it: `row` as `[[column, value], …]`.
    pub fn to_json(&self) -> Value {
        let mut o = json!({
            "panel": self.panel,
            "verified": self.verified,
            "row": self.row.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
            "vars": self.vars,
            "label": self.label,
        });
        if let (Some(c), Some(v)) = (&self.column, &self.value) {
            o["column"] = json!(c);
            o["value"] = v.clone();
        }
        o
    }
}

/// A number in a cited text: `1,234.5`, `1.234,5`, `12 %`, `€ 3.2`.
fn number(s: &str) -> Option<(f64, bool, usize)> {
    let t = s.trim();
    let pct = t.contains('%');
    let core: String = t
        .chars()
        .filter(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '-' | '+' | 'e' | 'E'))
        .collect();
    if core.is_empty() || !core.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    // The last of '.' / ',' is the decimal mark when it has 1–2 or 4+ digits
    // after it; three digits after a lone separator read as thousands.
    let last = core.rfind(['.', ',']);
    let (int, frac) = match last {
        Some(i) => {
            let after = &core[i + 1..];
            let both = core.contains('.') && core.contains(',');
            if after.len() == 3 && !both {
                (core.replace([',', '.'], ""), String::new())
            } else {
                (core[..i].replace([',', '.'], ""), after.to_string())
            }
        }
        None => (core.clone(), String::new()),
    };
    let text = if frac.is_empty() {
        int
    } else {
        format!("{int}.{frac}")
    };
    text.parse::<f64>().ok().map(|n| (n, pct, frac.len()))
}

/// Does the cited text `cited` match the cell `cell`, within the rounding
/// the citation shows?
pub fn same_value(cited: &str, cell: &Value) -> bool {
    let cell_text = match cell {
        Value::String(s) => s.clone(),
        Value::Null => return cited.trim().is_empty() || cited.trim().eq_ignore_ascii_case("null"),
        v => v.to_string(),
    };
    if cited.trim().eq_ignore_ascii_case(cell_text.trim()) {
        return true;
    }
    let cell_num = match cell {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => number(s).map(|(n, _, _)| n),
        _ => None,
    };
    let (Some(c), Some((n, pct, decimals))) = (cell_num, number(cited)) else {
        return false;
    };
    // Rounded to what the citation shows (at least 2 significant digits'
    // worth of slack for "1.2 million"-style short forms is not attempted).
    let half = 0.5 * 10f64.powi(-(decimals as i32));
    let close = |a: f64, b: f64| (a - b).abs() <= half + 1e-9 * a.abs().max(b.abs());
    close(c, n) || (pct && close(c * 100.0, n))
}

/// Verify `claim` against the results read in this conversation (the
/// latest read of that panel first).
pub fn verify(claim: &Claim, reads: &[Read]) -> Citation {
    let mut out = Citation {
        panel: claim.panel,
        verified: false,
        column: None,
        value: None,
        row: Vec::new(),
        vars: Map::new(),
        label: claim.label.clone(),
    };
    let col_of = |r: &Read, name: &str| -> Option<String> {
        r.columns
            .iter()
            .find(|c| c.eq_ignore_ascii_case(name.trim()))
            .cloned()
    };
    for read in reads.iter().rev().filter(|r| r.panel == claim.panel) {
        out.vars = read.vars.clone();
        // A citation of a panel without a value points at the panel only.
        let (Some(column), Some(value)) = (&claim.column, &claim.value) else {
            out.verified = true;
            return out;
        };
        let Some(col) = col_of(read, column) else {
            continue;
        };
        let keys: Option<Vec<(String, &str)>> = claim
            .row
            .iter()
            .map(|(k, v)| col_of(read, k).map(|c| (c, v.as_str())))
            .collect();
        let Some(keys) = keys else { continue };
        let hit = read.rows.iter().find(|row| {
            keys.iter()
                .all(|(k, v)| row.get(k).is_some_and(|cell| same_value(v, cell)))
                && row.get(&col).is_some_and(|cell| same_value(value, cell))
        });
        if let Some(row) = hit {
            out.verified = true;
            out.column = Some(col.clone());
            out.value = row.get(&col).cloned();
            out.row = keys
                .iter()
                .map(|(k, _)| (k.clone(), row.get(k).cloned().unwrap_or(Value::Null)))
                .collect();
            return out;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read() -> Read {
        let rows = vec![
            json!({"Region": "EU", "Revenue": 1234.56, "Share": 0.124}),
            json!({"Region": "US", "Revenue": 980.0, "Share": 0.3}),
        ];
        Read {
            panel: 3,
            vars: Map::new(),
            columns: vec!["Region".into(), "Revenue".into(), "Share".into()],
            rows: rows
                .into_iter()
                .map(|r| r.as_object().unwrap().clone())
                .collect(),
        }
    }

    fn claim(row: &[(&str, &str)], col: &str, value: &str) -> Claim {
        Claim {
            panel: 3,
            label: "x".into(),
            column: Some(col.into()),
            value: Some(value.into()),
            row: row
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
        }
    }

    #[test]
    fn numbers_match_within_the_shown_rounding() {
        assert!(same_value("1,234.6", &json!(1234.56)));
        assert!(same_value("1.234,56", &json!(1234.56)));
        assert!(same_value("1235", &json!(1234.56)));
        assert!(!same_value("1240", &json!(1234.56)));
        assert!(same_value("12.4 %", &json!(0.124)));
        assert!(same_value("EU", &json!("eu")));
    }

    #[test]
    fn the_right_value_in_the_wrong_row_is_not_verified() {
        let r = [read()];
        assert!(verify(&claim(&[("region", "EU")], "Revenue", "1234.56"), &r).verified);
        assert!(!verify(&claim(&[("region", "US")], "Revenue", "1234.56"), &r).verified);
        assert!(!verify(&claim(&[("nope", "US")], "Revenue", "980"), &r).verified);
        let c = verify(&claim(&[("Region", "US")], "share", "30 %"), &r);
        assert!(c.verified);
        assert_eq!(c.value, Some(json!(0.3)));
        assert_eq!(c.column.as_deref(), Some("Share"));
    }

    #[test]
    fn an_unread_panel_is_not_verified() {
        let mut c = claim(&[], "Revenue", "980");
        c.panel = 9;
        assert!(!verify(&c, &[read()]).verified);
        // the panel alone, read: points at it
        let mut c = Claim {
            panel: 3,
            ..Default::default()
        };
        assert!(verify(&c, &[read()]).verified);
        c.panel = 4;
        assert!(!verify(&c, &[read()]).verified);
    }
}
