//! Row-driven forms (`::FORM`, #683): typed parameter fields generated from a
//! query — one row per field — whose values come back as **one** variable
//! holding a JSON object.
//!
//! ```sql
//! SELECT field AS lead_time ::FORM, type, value, options, min, max, step,
//!        label, unit, help, show_if, source
//! FROM node_params WHERE node_id = getvariable('selected') ORDER BY ord;
//! -- then, anywhere: (getvariable('lead_time')->>'mean')::DOUBLE
//! ```
//!
//! The `::FORM` column holds each field's key; its alias names the variable.
//! The other columns are read by name (case-insensitive, all optional but
//! `value`): `type` (`number` — the default —, `slider`, `dropdown`,
//! `toggle`, `date`, `text`), `value` (the current value), `options` (a
//! list, or a comma-separated text, for a dropdown), `min` / `max` / `step`,
//! `label`, `unit`, `help`, `show_if` (`'model=normal'`,
//! `'model=normal|fitted'`: shown only while that other field has one of
//! those values) and `source` (provenance, shown beside the field: "fitted
//! from 214 POs", "manual").
//!
//! A JSON object text is the variable's value everywhere: the browser sets
//! it, `dashboard --check` and the static renderers bind the rows' own
//! values ([`default_json`]), and a locked server receives it like any other
//! string variable — no new value type crosses the wire. Numbers and toggles
//! are JSON numbers / booleans, everything else a string.

use serde_json::{Map, Value as Json};

/// A field's widget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    Number,
    Slider,
    Dropdown,
    Toggle,
    Date,
    Text,
}

impl FieldType {
    pub const NAMES: &'static [&'static str] =
        &["number", "slider", "dropdown", "toggle", "date", "text"];

    fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "" | "number" | "numeric" | "double" | "integer" => FieldType::Number,
            "slider" | "range" => FieldType::Slider,
            "dropdown" | "select" | "choice" => FieldType::Dropdown,
            "toggle" | "switch" | "checkbox" | "boolean" | "bool" => FieldType::Toggle,
            "date" => FieldType::Date,
            "text" | "string" | "varchar" => FieldType::Text,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            FieldType::Number => "number",
            FieldType::Slider => "slider",
            FieldType::Dropdown => "dropdown",
            FieldType::Toggle => "toggle",
            FieldType::Date => "date",
            FieldType::Text => "text",
        }
    }

    fn numeric(self) -> bool {
        matches!(self, FieldType::Number | FieldType::Slider)
    }
}

/// One field of a form.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub key: String,
    pub kind: FieldType,
    /// The current value, already typed (number / bool / string / null).
    pub value: Json,
    pub options: Vec<String>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
    pub label: Option<String>,
    pub unit: Option<String>,
    pub help: Option<String>,
    /// `(other field, allowed values)`.
    pub show_if: Option<(String, Vec<String>)>,
    pub source: Option<String>,
}

impl Field {
    pub fn to_json(&self) -> Json {
        serde_json::json!({
            "key": self.key,
            "type": self.kind.as_str(),
            "value": self.value,
            "options": self.options,
            "min": self.min,
            "max": self.max,
            "step": self.step,
            "label": self.label,
            "unit": self.unit,
            "help": self.help,
            "show_if": self.show_if.as_ref().map(|(f, vs)| serde_json::json!({"field": f, "values": vs})),
            "source": self.source,
        })
    }
}

/// Most fields one form may have.
pub const MAX_FIELDS: usize = 64;

fn text(v: Option<&Json>) -> Option<String> {
    match v? {
        Json::Null => None,
        Json::String(s) => Some(s.clone()),
        v => Some(v.to_string()),
    }
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())
}

fn num(v: Option<&Json>) -> Option<f64> {
    match v? {
        Json::Number(n) => n.as_f64(),
        Json::String(s) => s.trim().parse().ok(),
        _ => None,
    }
    .filter(|f: &f64| f.is_finite())
}

fn truthy(v: &Json) -> bool {
    match v {
        Json::Bool(b) => *b,
        Json::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Json::String(s) => matches!(
            s.trim().to_ascii_lowercase().as_str(),
            "true" | "t" | "1" | "yes" | "on"
        ),
        _ => false,
    }
}

fn options_of(v: Option<&Json>) -> Vec<String> {
    match v {
        Some(Json::Array(a)) => a.iter().filter_map(|x| text(Some(x))).collect(),
        Some(Json::String(s)) => {
            // A JSON list as text (a DuckDB list read back as a string), else
            // a comma-separated list.
            if let Ok(Json::Array(a)) = serde_json::from_str::<Json>(s) {
                return options_of(Some(&Json::Array(a)));
            }
            let t = s.trim().trim_start_matches('[').trim_end_matches(']');
            t.split(',')
                .map(|x| x.trim().trim_matches('\'').trim_matches('"').to_string())
                .filter(|x| !x.is_empty())
                .collect()
        }
        _ => Vec::new(),
    }
}

/// `'model=normal'` / `'model=normal|fitted'` → `(model, [normal, fitted])`.
fn parse_show_if(s: &str) -> Option<(String, Vec<String>)> {
    let (f, vs) = s.split_once('=')?;
    let f = f.trim();
    let vs: Vec<String> = vs
        .split('|')
        .map(|v| v.trim().trim_matches('\'').to_string())
        .filter(|v| !v.is_empty())
        .collect();
    (!f.is_empty() && !vs.is_empty()).then(|| (f.to_string(), vs))
}

/// A row's value for `name`, the column matched case-insensitively.
fn col<'a>(row: &'a Map<String, Json>, name: &str) -> Option<&'a Json> {
    row.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v)
}

/// Read a `::FORM` panel's rows (`key_col` = the `::FORM` column's name) into
/// its fields, plus `(code, message)` problems for `dashboard --check`. A
/// row with no key, or one past [`MAX_FIELDS`], is left out.
pub fn fields(
    key_col: &str,
    rows: &[Map<String, Json>],
) -> (Vec<Field>, Vec<(&'static str, String)>) {
    let mut out: Vec<Field> = Vec::new();
    let mut issues = Vec::new();
    if rows.len() > MAX_FIELDS {
        issues.push((
            "form/too-many-fields",
            format!("{} fields — a form shows at most {MAX_FIELDS}", rows.len()),
        ));
    }
    if let Some(first) = rows.first() {
        if col(first, "value").is_none() {
            issues.push((
                "form/no-value",
                "a ::FORM needs a `value` column (each field's current value)".into(),
            ));
        }
    }
    for row in rows.iter().take(MAX_FIELDS) {
        let Some(key) = text(row.get(key_col)) else {
            issues.push((
                "form/no-key",
                "a ::FORM row with an empty field key is left out".into(),
            ));
            continue;
        };
        if out.iter().any(|f| f.key == key) {
            issues.push((
                "form/duplicate-field",
                format!("field `{key}` appears twice — the first row wins"),
            ));
            continue;
        }
        let ty = text(col(row, "type")).unwrap_or_default();
        let kind = FieldType::parse(&ty).unwrap_or_else(|| {
            issues.push((
                "form/unknown-type",
                format!(
                    "field `{key}`: type `{ty}` — use one of {} (drawn as a text field)",
                    FieldType::NAMES.join(", ")
                ),
            ));
            FieldType::Text
        });
        let options = options_of(col(row, "options"));
        let (min, max, step) = (
            num(col(row, "min")),
            num(col(row, "max")),
            num(col(row, "step")),
        );
        let raw = col(row, "value");
        let value = match kind {
            _ if raw.is_none_or(Json::is_null) => Json::Null,
            FieldType::Number | FieldType::Slider => match num(raw) {
                Some(f) => serde_json::Number::from_f64(f).map_or(Json::Null, Json::Number),
                None => {
                    issues.push((
                        "form/bad-value",
                        format!("field `{key}`: value {} is not a number", raw.unwrap()),
                    ));
                    Json::Null
                }
            },
            FieldType::Toggle => Json::Bool(raw.is_some_and(truthy)),
            _ => text(raw).map_or(Json::Null, Json::String),
        };
        if kind == FieldType::Dropdown {
            if options.is_empty() {
                issues.push((
                    "form/no-options",
                    format!("field `{key}`: a dropdown needs `options` (a list)"),
                ));
            } else if let Json::String(v) = &value {
                if !options.contains(v) {
                    issues.push((
                        "form/bad-value",
                        format!("field `{key}`: value `{v}` is not one of its options"),
                    ));
                }
            }
        }
        if kind.numeric() {
            if let (Some(lo), Some(hi)) = (min, max) {
                if lo > hi {
                    issues.push((
                        "form/bad-range",
                        format!("field `{key}`: min {lo} > max {hi}"),
                    ));
                }
            }
            if let Some(v) = value.as_f64() {
                if min.is_some_and(|lo| v < lo) || max.is_some_and(|hi| v > hi) {
                    issues.push((
                        "form/bad-value",
                        format!("field `{key}`: value {v} is outside its min/max"),
                    ));
                }
            }
            if kind == FieldType::Slider && (min.is_none() || max.is_none()) {
                issues.push((
                    "form/slider-range",
                    format!("field `{key}`: a slider needs `min` and `max`"),
                ));
            }
        }
        let show_if_text = text(col(row, "show_if"));
        let show_if = show_if_text.as_deref().and_then(parse_show_if);
        if let (Some(t), None) = (&show_if_text, &show_if) {
            issues.push((
                "form/bad-show-if",
                format!("field `{key}`: show_if `{t}` — write it as 'field=value' or 'field=a|b'"),
            ));
        }
        out.push(Field {
            key,
            kind,
            value,
            options,
            min,
            max,
            step,
            label: text(col(row, "label")),
            unit: text(col(row, "unit")),
            help: text(col(row, "help")),
            show_if,
            source: text(col(row, "source")),
        });
    }
    for f in &out {
        if let Some((other, _)) = &f.show_if {
            if !out.iter().any(|g| &g.key == other) {
                issues.push((
                    "form/bad-show-if",
                    format!(
                        "field `{}`: show_if names `{other}`, which is no field of this form",
                        f.key
                    ),
                ));
            }
        }
    }
    (out, issues)
}

/// The variable's value from the fields' own values: a JSON object text,
/// keys in field order.
pub fn default_json(fields: &[Field]) -> String {
    let mut m = Map::new();
    for f in fields {
        m.insert(f.key.clone(), f.value.clone());
    }
    Json::Object(m).to_string()
}

/// [`fields`] + [`default_json`] as JSON for the browser (wasm
/// `form_fields`): `{"fields": [{key, type, value, options, min, max, step,
/// label, unit, help, show_if: {field, values} | null, source}], "default":
/// "<json text>", "issues": ["…"]}`.
pub fn fields_json(key_col: &str, rows: &[Map<String, Json>]) -> String {
    let (fs, issues) = fields(key_col, rows);
    serde_json::json!({
        "fields": fs.iter().map(Field::to_json).collect::<Vec<_>>(),
        "default": default_json(&fs),
        "issues": issues.iter().map(|(_, m)| m.clone()).collect::<Vec<_>>(),
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(v: Json) -> Vec<Map<String, Json>> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|r| r.as_object().unwrap().clone())
            .collect()
    }

    #[test]
    fn fields_and_default() {
        let r = rows(serde_json::json!([
            {"lead_time": "model", "type": "dropdown", "value": "normal", "options": ["fixed", "normal"]},
            {"lead_time": "mean", "type": "number", "value": 5, "min": 0, "unit": "days", "show_if": "model=normal|fixed"},
            {"lead_time": "sd", "TYPE": "slider", "value": "1.5", "min": 0, "max": 10, "step": 0.5, "show_if": "model=normal"},
            {"lead_time": "fitted", "type": "toggle", "value": "true", "source": "214 POs"},
        ]));
        let (f, issues) = fields("lead_time", &r);
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(f.len(), 4);
        assert_eq!(f[2].kind, FieldType::Slider);
        assert_eq!(
            f[1].show_if,
            Some(("model".into(), vec!["normal".into(), "fixed".into()]))
        );
        assert_eq!(
            default_json(&f),
            r#"{"model":"normal","mean":5.0,"sd":1.5,"fitted":true}"#
        );
        // Options as text (a DuckDB list read back as a string).
        assert_eq!(options_of(Some(&Json::String("[a, b]".into()))), ["a", "b"]);
    }

    #[test]
    fn problems() {
        let r = rows(serde_json::json!([
            {"f": "a", "type": "dial", "value": 1},
            {"f": "b", "type": "dropdown", "value": "x", "options": "y,z"},
            {"f": "c", "type": "number", "value": "abc"},
            {"f": "d", "type": "slider", "value": 50, "min": 0, "max": 10},
            {"f": "e", "type": "dropdown", "value": null},
            {"f": "g", "value": 1, "show_if": "nope=1"},
            {"f": "a", "value": 2},
        ]));
        let (_, issues) = fields("f", &r);
        let codes: Vec<&str> = issues.iter().map(|(c, _)| *c).collect();
        for c in [
            "form/unknown-type",
            "form/bad-value",
            "form/no-options",
            "form/duplicate-field",
            "form/bad-show-if",
        ] {
            assert!(codes.contains(&c), "{c}: {codes:?}");
        }
        assert_eq!(
            codes.iter().filter(|c| **c == "form/bad-value").count(),
            3,
            "{codes:?}"
        );
    }
}
