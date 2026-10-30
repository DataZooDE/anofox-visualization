//! #751: chart tooltip templates (`::TOOLTIP_FORMAT`, `::TOOLTIP_HEADER`),
//! Highcharts-style: `{name}` or `{name:format}`, `{{` / `}}` literal braces.
//! The browser fills them from the hovered mark's result row
//! (`data-row`, see [`crate::with_row_index`]); the result is plain text
//! (the browser sets it as `textContent`).
//!
//! A format is a `::YFORMAT` keyword (`€`, `$`, `percent`, `' kg'`,
//! `'CHF '`…), `.Nf` (N decimals), or — for a date / timestamp value — a
//! strftime pattern (`%d %b %Y`, ggplot-rs's subset). Without one, a name
//! takes its default (the panel's `::YFORMAT` for `y`, `::XFORMAT` for `x`).

use crate::format::{Locale, ValueFmt};
use serde_json::{Map, Value as J};

/// The placeholders of `template`: (name, format).
pub fn placeholders(template: &str) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    walk(template, &mut |name, spec| {
        out.push((name.to_string(), spec.map(str::to_string)));
        String::new()
    });
    out
}

/// `template` with every placeholder replaced by its field from `fields`
/// (a missing or null field is empty), formatted by its own format, else by
/// `defaults[name]`.
pub fn fill(
    template: &str,
    fields: &Map<String, J>,
    defaults: &Map<String, J>,
    l: &Locale,
) -> String {
    walk(template, &mut |name, spec| {
        let spec = spec
            .map(str::to_string)
            .or_else(|| defaults.get(name).and_then(J::as_str).map(str::to_string));
        fields
            .get(name)
            .map(|v| value(v, spec.as_deref(), l))
            .unwrap_or_default()
    })
}

fn walk(template: &str, f: &mut dyn FnMut(&str, Option<&str>) -> String) -> String {
    let mut out = String::new();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                out.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                out.push('}');
            }
            '{' => {
                let mut inner = String::new();
                let mut closed = false;
                for nc in chars.by_ref() {
                    if nc == '}' {
                        closed = true;
                        break;
                    }
                    inner.push(nc);
                }
                if !closed {
                    out.push('{');
                    out.push_str(&inner);
                    continue;
                }
                let (name, spec) = match inner.split_once(':') {
                    Some((n, s)) => (n.trim(), Some(s)),
                    None => (inner.trim(), None),
                };
                out.push_str(&f(name, spec.filter(|s| !s.is_empty())));
            }
            c => out.push(c),
        }
    }
    out
}

fn value(v: &J, spec: Option<&str>, l: &Locale) -> String {
    match v {
        J::Null => String::new(),
        J::Bool(b) => b.to_string(),
        J::Number(n) => {
            let x = n.as_f64().unwrap_or(f64::NAN);
            number(x, spec, l)
        }
        J::String(s) => {
            if let Some(secs) = crate::sql::parse_iso_epoch(s) {
                return match spec {
                    Some(p) if p.contains('%') => {
                        ggplot_rs::format::format_timestamp(secs as f64, p, Some(&l.gg()))
                    }
                    _ => l.format_date(s),
                };
            }
            match (spec, s.trim().parse::<f64>()) {
                (Some(p), Ok(x)) if !p.contains('%') => number(x, Some(p), l),
                _ => s.clone(),
            }
        }
        other => other.to_string(),
    }
}

fn number(x: f64, spec: Option<&str>, l: &Locale) -> String {
    if let Some(d) = spec
        .and_then(|p| p.strip_prefix('.'))
        .and_then(|p| p.strip_suffix('f'))
        .and_then(|d| d.parse::<usize>().ok())
        .filter(|d| *d <= 10)
    {
        if !x.is_finite() {
            return "–".into();
        }
        let s = format!("{:.*}", d, x.abs());
        let (int, frac) = s.split_once('.').unwrap_or((&s, ""));
        let mut g = String::new();
        for (i, ch) in int.chars().enumerate() {
            if i > 0 && (int.len() - i) % 3 == 0 {
                g.push(',');
            }
            g.push(ch);
        }
        let en = if frac.is_empty() {
            g
        } else {
            format!("{g}.{frac}")
        };
        let sign = if x < 0.0 && x.abs() * 10f64.powi(d as i32) >= 0.5 {
            "-"
        } else {
            ""
        };
        return format!("{sign}{}", l.localize(&en));
    }
    ValueFmt::parse(spec.unwrap_or("")).full(x, l)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn m(v: J) -> Map<String, J> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn fills_names_formats_and_defaults() {
        let l = Locale::default();
        let f = m(json!({"series": "EU", "y": 1234.5, "Units": 12, "x": "2026-03-01"}));
        let d = m(json!({"y": "€"}));
        assert_eq!(
            fill("{series}: {y} ({Units} units)", &f, &d, &l),
            "EU: €1,234.50 (12 units)"
        );
        assert_eq!(fill("{y:.1f}", &f, &d, &l), "1,234.5");
        assert_eq!(fill("{y: kg}", &f, &d, &l), "1,234.5 kg");
        assert_eq!(fill("{x:%d %b %Y}", &f, &d, &l), "01 Mar 2026");
        assert_eq!(fill("{{literal}} {missing}|", &f, &d, &l), "{literal} |");
        assert_eq!(fill("{unterminated", &f, &d, &l), "{unterminated");
    }

    #[test]
    fn a_locale_applies() {
        let l = Locale::from_tag("de-DE").unwrap();
        let f = m(json!({"y": 1234.5, "x": "2026-03-01"}));
        assert_eq!(fill("{y:.2f}", &f, &Map::new(), &l), "1.234,50");
        assert_eq!(fill("{x:%-d. %B}", &f, &Map::new(), &l), "1. März");
    }

    #[test]
    fn values_stay_text() {
        let l = Locale::default();
        let f = m(json!({"n": "<script>x</script>"}));
        // escaping is the browser's (textContent); the filler never adds markup
        assert_eq!(fill("{n}", &f, &Map::new(), &l), "<script>x</script>");
    }

    #[test]
    fn lists_placeholders() {
        assert_eq!(
            placeholders("{x:%b} {y} {{no}}"),
            vec![
                ("x".to_string(), Some("%b".to_string())),
                ("y".to_string(), None)
            ]
        );
    }
}
