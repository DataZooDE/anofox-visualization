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
//! those values; `&` and `|` join conditions, `'capex=true & fin=loan'`) and
//! `source` (provenance, shown beside the field: "fitted from 214 POs",
//! "manual").
//!
//! Long forms (#1233): `section` groups fields under a heading (shown as a
//! list, tabs or an accordion: `::FORM_LAYOUT`), `advanced` (boolean) puts a
//! field in its section's collapsed "More" part, `width` (`half` / `full`)
//! lets two short fields share a row, and a `preset` row (its `options` a
//! JSON object, one entry per choice: `{"Flat": {"use_kwh": 2000}}`) sets
//! several fields at once — it is no key of the variable.
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
    /// Sets several fields at once (#1233); not a key of the variable.
    Preset,
    /// A family and its parameters, or a likely value and a 90 % range
    /// (#1235): one JSON object, `{"family": "Beta", "a": 2, "b": 3}`.
    Distribution,
}

impl FieldType {
    pub const NAMES: &'static [&'static str] = &[
        "number",
        "slider",
        "dropdown",
        "toggle",
        "date",
        "text",
        "preset",
        "distribution",
    ];

    fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "" | "number" | "numeric" | "double" | "integer" => FieldType::Number,
            "slider" | "range" => FieldType::Slider,
            "dropdown" | "select" | "choice" => FieldType::Dropdown,
            "toggle" | "switch" | "checkbox" | "boolean" | "bool" => FieldType::Toggle,
            "date" => FieldType::Date,
            "text" | "string" | "varchar" => FieldType::Text,
            "preset" | "presets" => FieldType::Preset,
            "distribution" | "dist" | "prior" => FieldType::Distribution,
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
            FieldType::Preset => "preset",
            FieldType::Distribution => "distribution",
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
    /// When the field shows: any of these groups holds, a group when every
    /// `(other field, allowed values)` in it does (`a=1 & b=x | c=y`).
    pub show_if: ShowIf,
    pub source: Option<String>,
    /// The heading it sits under (#1233).
    pub section: Option<String>,
    /// In its section's collapsed "More" part.
    pub advanced: bool,
    /// `half`: two such fields share a row; `full` (the default) a row alone.
    pub half: bool,
    /// A preset's choices: `(label, field → value)`.
    pub presets: Vec<(String, Map<String, Json>)>,
}

/// A `show_if`: any group holds when every `(field, allowed values)` in it
/// does.
pub type ShowIf = Vec<Vec<(String, Vec<String>)>>;

/// The `::FORM_LAYOUT` values: how a form's sections show (#1233).
pub const LAYOUTS: &[&str] = &["list", "tabs", "accordion"];

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
            "show_if": (!self.show_if.is_empty()).then(|| {
                self.show_if
                    .iter()
                    .map(|g| {
                        g.iter()
                            .map(|(f, vs)| serde_json::json!({"field": f, "values": vs}))
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>()
            }),
            "source": self.source,
            "section": self.section,
            "advanced": self.advanced,
            "width": if self.half { "half" } else { "full" },
            "presets": self.presets.iter().map(|(l, v)| serde_json::json!({"label": l, "values": v})).collect::<Vec<_>>(),
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

/// `'model=normal'` / `'model=normal|fitted'` → `[[(model, [normal, fitted])]]`;
/// `&` joins conditions, `|` before another `field=` starts an alternative
/// group (#1233): `'capex=true & fin=loan|lease | profile=home'` →
/// `[[(capex, [true]), (fin, [loan, lease])], [(profile, [home])]]`.
fn parse_show_if(s: &str) -> Option<ShowIf> {
    let mut groups: ShowIf = vec![Vec::new()];
    // (operator before it, token)
    let mut toks: Vec<(char, &str)> = Vec::new();
    let mut op = '&';
    let mut start = 0;
    for (i, c) in s.char_indices() {
        if c == '&' || c == '|' {
            toks.push((op, &s[start..i]));
            op = c;
            start = i + 1;
        }
    }
    toks.push((op, &s[start..]));
    for (op, t) in toks {
        let t = t.trim();
        match t.split_once('=') {
            Some((f, v)) => {
                let (f, v) = (f.trim(), v.trim().trim_matches('\'').to_string());
                if f.is_empty() || v.is_empty() {
                    return None;
                }
                if op == '|' {
                    groups.push(Vec::new());
                }
                groups.last_mut()?.push((f.to_string(), vec![v]));
            }
            // `|b` after `a=x`: another value of that condition
            None if op == '|' => {
                let v = t.trim_matches('\'').to_string();
                if v.is_empty() {
                    return None;
                }
                groups.last_mut()?.last_mut()?.1.push(v);
            }
            None => return None,
        }
    }
    groups.iter().all(|g| !g.is_empty()).then_some(groups)
}

/// A preset's `options`: a JSON object (or its text), one entry per
/// choice, each an object of field values.
fn presets_of(v: Option<&Json>) -> Option<Vec<(String, Map<String, Json>)>> {
    let obj = match v? {
        Json::Object(o) => o.clone(),
        Json::String(s) => match serde_json::from_str::<Json>(s).ok()? {
            Json::Object(o) => o,
            _ => return None,
        },
        _ => return None,
    };
    obj.into_iter()
        .map(|(k, v)| match v {
            Json::Object(o) => Some((k, o)),
            _ => None,
        })
        .collect()
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
        let presets = if kind == FieldType::Preset {
            let p = presets_of(col(row, "options"));
            if p.as_ref().is_none_or(Vec::is_empty) {
                issues.push((
                    "form/bad-preset",
                    format!("field `{key}`: a preset's `options` is a JSON object, one entry per choice: '{{\"Flat\": {{\"use_kwh\": 2000}}}}'"),
                ));
            }
            p.unwrap_or_default()
        } else {
            Vec::new()
        };
        let options = match kind {
            FieldType::Preset => presets.iter().map(|(l, _)| l.clone()).collect(),
            // the families it offers: its options, else all of them
            FieldType::Distribution => {
                let mut fams = Vec::new();
                for o in options_of(col(row, "options")) {
                    match crate::distfield::Family::parse(&o) {
                        Some(f) => fams.push(f.name().to_string()),
                        None => issues.push((
                            "form/bad-distribution",
                            format!(
                                "field `{key}`: option `{o}` is no distribution family ({})",
                                crate::distfield::FAMILIES.join(", ")
                            ),
                        )),
                    }
                }
                if fams.is_empty() {
                    fams = crate::distfield::FAMILIES.iter().map(|f| f.to_string()).collect();
                }
                fams
            }
            _ => options_of(col(row, "options")),
        };
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
            FieldType::Preset => Json::Null,
            // stored as the object, also when the row gave 'Beta(2, 3)'
            FieldType::Distribution => match crate::distfield::Dist::parse(raw.unwrap()) {
                Ok(d) if options.iter().any(|o| o == d.family.name()) => d.to_json(),
                Ok(d) => {
                    issues.push((
                        "form/bad-distribution",
                        format!(
                            "field `{key}`: value {} is a {}, which is not among its options ({})",
                            d.text(),
                            d.family.name(),
                            options.join(", ")
                        ),
                    ));
                    d.to_json()
                }
                Err(e) => {
                    issues.push(("form/bad-distribution", format!("field `{key}`: {e}")));
                    Json::Null
                }
            },
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
                format!("field `{key}`: show_if `{t}` — write it as 'field=value', 'field=a|b' or 'a=x & b=y | c=z'"),
            ));
        }
        let width = text(col(row, "width")).map(|w| w.to_ascii_lowercase());
        if let Some(w) = width.as_deref().filter(|w| *w != "half" && *w != "full") {
            issues.push((
                "form/bad-width",
                format!("field `{key}`: width `{w}` — 'half' or 'full' (drawn full)"),
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
            show_if: show_if.unwrap_or_default(),
            source: text(col(row, "source")),
            section: text(col(row, "section")),
            advanced: col(row, "advanced").is_some_and(truthy),
            half: width.as_deref() == Some("half"),
            presets,
        });
    }
    for f in &out {
        for (other, _) in f.show_if.iter().flatten() {
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
        for (label, vals) in &f.presets {
            for (k, v) in vals {
                match out.iter().find(|g| &g.key == k && g.kind != FieldType::Preset) {
                    None => issues.push((
                        "form/bad-preset",
                        format!(
                            "field `{}`: preset `{label}` sets `{k}`, which is no field of this form",
                            f.key
                        ),
                    )),
                    Some(g) if !value_fits(g, v) => issues.push((
                        "form/bad-preset",
                        format!(
                            "field `{}`: preset `{label}` sets `{k}` to {v}, which that field doesn't take",
                            f.key
                        ),
                    )),
                    _ => {}
                }
            }
        }
    }
    // a section on some rows only: the rest go under an untitled heading
    let sectioned = out.iter().filter(|f| f.section.is_some()).count();
    if sectioned > 0 && sectioned < out.len() {
        let bare: Vec<&str> = out
            .iter()
            .filter(|f| f.section.is_none())
            .map(|f| f.key.as_str())
            .take(5)
            .collect();
        issues.push((
            "form/partial-sections",
            format!(
                "some fields have a `section` and some don't ({}) — those are drawn first, under no heading",
                bare.join(", ")
            ),
        ));
    }
    (out, issues)
}

/// Does field `f` take value `v` (a preset's): the right type, inside
/// min/max, one of a dropdown's options.
fn value_fits(f: &Field, v: &Json) -> bool {
    match f.kind {
        FieldType::Number | FieldType::Slider => v
            .as_f64()
            .is_some_and(|x| f.min.is_none_or(|lo| x >= lo) && f.max.is_none_or(|hi| x <= hi)),
        FieldType::Toggle => v.is_boolean(),
        FieldType::Dropdown => v.as_str().is_some_and(|s| f.options.iter().any(|o| o == s)),
        FieldType::Date | FieldType::Text => v.is_string() || v.is_null(),
        FieldType::Preset => false,
        FieldType::Distribution => crate::distfield::Dist::parse(v)
            .is_ok_and(|d| f.options.iter().any(|o| o == d.family.name())),
    }
}

/// The variable's value from the fields' own values: a JSON object text,
/// keys in field order (a preset is no key).
pub fn default_json(fields: &[Field]) -> String {
    let mut m = Map::new();
    for f in fields.iter().filter(|f| f.kind != FieldType::Preset) {
        m.insert(f.key.clone(), f.value.clone());
    }
    Json::Object(m).to_string()
}

/// [`fields`] + [`default_json`] as JSON for the browser (wasm
/// `form_fields`): `{"fields": [{key, type, value, options, min, max, step,
/// label, unit, help, show_if: [[{field, values}]] | null (any group, every
/// condition in it), source, section, advanced, width: half | full, presets:
/// [{label, values}]}], "default":
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
            vec![vec![(
                "model".into(),
                vec!["normal".into(), "fixed".into()]
            )]]
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

    #[test]
    fn show_if_joins_conditions() {
        let c = |f: &str, vs: &[&str]| {
            (
                f.to_string(),
                vs.iter().map(|v| v.to_string()).collect::<Vec<_>>(),
            )
        };
        assert_eq!(
            parse_show_if("capex=true & fin=loan|lease | profile=One measured home"),
            Some(vec![
                vec![c("capex", &["true"]), c("fin", &["loan", "lease"])],
                vec![c("profile", &["One measured home"])],
            ])
        );
        assert_eq!(
            parse_show_if("a=1|b=2"),
            Some(vec![vec![c("a", &["1"])], vec![c("b", &["2"])]])
        );
        for bad in ["", "a", "a=1 &", "a=1 & b", "=1", "a="] {
            assert_eq!(parse_show_if(bad), None, "{bad}");
        }
    }

    #[test]
    fn sections_advanced_width_presets() {
        let r = rows(serde_json::json!([
            {"c": "size", "type": "preset", "value": null, "section": "Household",
             "options": "{\"Flat\": {\"use\": 2000, \"heat\": \"gas\"}, \"House\": {\"use\": 5000}}"},
            {"c": "use", "value": 4000, "min": 500, "max": 20000, "section": "Household", "width": "half"},
            {"c": "heat", "type": "dropdown", "options": ["gas", "pump"], "value": "gas", "section": "Household", "advanced": true},
            {"c": "loan", "value": 5, "section": "Money", "show_if": "heat=pump & use=4000 | heat=gas"},
        ]));
        let (f, issues) = fields("c", &r);
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(f[0].kind, FieldType::Preset);
        assert_eq!(f[0].options, ["Flat", "House"]);
        assert_eq!(f[0].presets[0].1["heat"], "gas");
        assert!(f[1].half && !f[2].half && f[2].advanced);
        assert_eq!(f[3].section.as_deref(), Some("Money"));
        assert_eq!(f[3].show_if.len(), 2);
        // a preset is no key of the variable
        assert_eq!(
            default_json(&f),
            r#"{"use":4000.0,"heat":"gas","loan":5.0}"#
        );
        let j: Json = serde_json::from_str(&fields_json("c", &r)).unwrap();
        assert_eq!(j["fields"][1]["width"], "half");
        assert_eq!(j["fields"][0]["presets"][1]["label"], "House");
        assert_eq!(j["fields"][3]["show_if"][1][0]["field"], "heat");
    }

    #[test]
    fn long_form_problems() {
        let r = rows(serde_json::json!([
            {"c": "p", "type": "preset", "value": null, "options": "{\"A\": {\"x\": 99, \"nope\": 1}}"},
            {"c": "q", "type": "preset", "value": null, "options": "not json"},
            {"c": "x", "value": 1, "max": 10, "section": "S", "width": "third"},
            {"c": "y", "value": 1, "show_if": "x=1 & zz=2"},
        ]));
        let (_, issues) = fields("c", &r);
        let msgs: Vec<String> = issues.iter().map(|(c, m)| format!("{c}: {m}")).collect();
        for want in [
            "form/bad-preset: field `p`: preset `A` sets `x` to 99",
            "form/bad-preset: field `p`: preset `A` sets `nope`",
            "form/bad-preset: field `q`",
            "form/bad-width: field `x`",
            "form/bad-show-if: field `y`: show_if names `zz`",
            "form/partial-sections:",
        ] {
            assert!(
                msgs.iter().any(|m| m.starts_with(want)),
                "{want}: {msgs:#?}"
            );
        }
    }
}
