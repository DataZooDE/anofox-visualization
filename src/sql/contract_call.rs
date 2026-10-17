//! Contract plot **calls** in a dashboard script — the browser / host side of
//! the `anofox_plot_*` macros.
//!
//! The macros (`anofox_plot_terms(t)`, … see [`crate::macros`]) are DuckDB SQL
//! macros of the extension. A host that runs SQL without the extension
//! (DuckDB-Wasm in the demo, a plain `duckdb` CLI behind `dashboard`) cannot
//! call them, so the planner rewrites a statement of the form
//!
//! ```sql
//! SELECT [group cols,] anofox_plot_xxx(alias [, name := expr …]) [, 'Title'::TITLE]
//! FROM … alias [WHERE … GROUP BY …]
//! ```
//!
//! into what the macro would aggregate itself:
//!
//! ```sql
//! SELECT [group cols AS c{i},] to_json(list(alias ORDER BY alias)) AS c{k},
//!        json_object('name', expr, …) AS c{k+1} [, 'Title' AS c{j}] FROM …
//! ```
//!
//! and renders each result row with [`render_call`] — the same contract
//! renderer the macro reaches through `anofox_render`. So one script works
//! natively (the extension's macro, or this rewrite) and in the browser.
//!
//! Named arguments are evaluated by DuckDB (constants, `getvariable(…)`,
//! scalar subqueries such as `tests := (SELECT list(t) FROM tukey t)`), keyed
//! by parameter name; [`render_call`] maps them onto the macro's `options`
//! from the macro table, so a macro added later is picked up without changes
//! here. `width :=` / `height :=` are ignored — the dashboard sizes panels.

use super::lex::{self, TokKind, Token};
use crate::RenderError;
use serde_json::{Map, Value as J};

/// The macro-call item of a contract statement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContractCall {
    /// Lower-cased macro name (`anofox_plot_terms`).
    pub macro_name: String,
    /// The row alias passed as the first argument.
    pub alias: String,
    /// Named arguments `(parameter, SQL expression)` in call order.
    pub args: Vec<(String, String)>,
}

/// Where a planned contract panel's pieces sit in the rewritten result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContractPanel {
    pub macro_name: String,
    /// Output column (`c{rows}`) holding the rows as a JSON array.
    pub rows: usize,
    /// Output column (`c{args}`) holding the named arguments as a JSON object.
    pub args: usize,
    /// Un-annotated select items (`GROUP BY` keys): `(output index, name)` —
    /// one panel per result row, labelled by these values.
    pub groups: Vec<(usize, String)>,
}

fn sig(src: &str) -> Vec<Token> {
    lex::tokenize(src)
        .into_iter()
        .filter(|t| !t.is_trivia())
        .collect()
}

/// Is `name` a contract plot macro name (`anofox_plot` / `anofox_plot_*`)?
pub fn is_contract_macro(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == "anofox_plot"
        || n.strip_prefix("anofox_plot_").is_some_and(|r| {
            !r.is_empty() && r.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

/// Parse one select-list item as `anofox_plot_xxx(alias [, name := expr]…)`
/// (an optional trailing `AS alias` is allowed and dropped). `None` for
/// anything else — including positional extra arguments.
pub fn parse_call(item: &str) -> Option<ContractCall> {
    let item = lex::trailing_alias(item.trim()).0;
    let toks = sig(item);
    let (name, open) = (toks.first()?, toks.get(1)?);
    if name.kind != TokKind::Word
        || !is_contract_macro(name.text(item))
        || !open.is_punct(item, '(')
    {
        return None;
    }
    // The call's closing paren must be the last token.
    let mut depth = 0i32;
    let mut close = None;
    for (i, t) in toks.iter().enumerate().skip(1) {
        if t.kind != TokKind::Punct {
            continue;
        }
        match t.text(item) {
            "(" | "[" | "{" => depth += 1,
            ")" | "]" | "}" => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close?;
    if close + 1 != toks.len() {
        return None;
    }
    let inner = &item[open.end..toks[close].start];
    let parts = lex::split_top_commas(inner);
    let mut parts = parts.iter().map(|p| p.trim());
    let alias = lex::simple_ident(parts.next()?)?;
    let mut args = Vec::new();
    for p in parts {
        let (k, v) = named_arg(p)?;
        args.push((k, v));
    }
    Some(ContractCall {
        macro_name: name.text(item).to_ascii_lowercase(),
        alias,
        args,
    })
}

/// `name := expr` (or `name => expr`) → `(name, expr)`.
fn named_arg(p: &str) -> Option<(String, String)> {
    let t = sig(p);
    let (n, a, b) = (t.first()?, t.get(1)?, t.get(2)?);
    let op_ok = a.end == b.start
        && a.kind == TokKind::Punct
        && b.kind == TokKind::Punct
        && matches!((a.text(p), b.text(p)), (":", "=") | ("=", ">"));
    if n.kind != TokKind::Word || !op_ok {
        return None;
    }
    let expr = p[b.end..].trim();
    if expr.is_empty() {
        return None;
    }
    Some((n.text(p).to_ascii_lowercase(), expr.to_string()))
}

/// A SQL string literal.
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// A quoted SQL identifier (so a reserved word or odd alias still parses).
fn ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// If `stmt` is a contract statement, the statement with the macro call
/// replaced by its two payload items (`to_json(list(alias ORDER BY alias))`,
/// `json_object(…)`), the call, and the call's select-list position. Only the
/// first call is taken; any `::ROLE` items are left for [`super::rewrite`].
pub fn split_contract(stmt: &str) -> Option<(String, ContractCall, usize)> {
    if !super::is_query_statement(stmt) {
        return None;
    }
    let (sel, end) = super::main_select_list(stmt)?;
    let items = lex::split_top_commas(&stmt[sel..end]);
    let (pos, call) = items
        .iter()
        .enumerate()
        .find_map(|(i, it)| parse_call(it).map(|c| (i, c)))?;
    let a = ident(&call.alias);
    let args = if call.args.is_empty() {
        "json_object()".to_string()
    } else {
        let kv: Vec<String> = call
            .args
            .iter()
            .map(|(k, v)| format!("{}, {v}", quote(k)))
            .collect();
        format!("json_object({})", kv.join(", "))
    };
    let mut out: Vec<String> = Vec::with_capacity(items.len() + 1);
    for (i, it) in items.iter().enumerate() {
        if i == pos {
            out.push(format!("to_json(list({a} ORDER BY {a})) AS anofox_rows"));
            out.push(format!("{args} AS anofox_args"));
        } else {
            out.push(it.trim().to_string());
        }
    }
    let sql = format!("{} {} {}", &stmt[..sel], out.join(", "), &stmt[end..]);
    Some((sql, call, pos))
}

/// Plan a contract statement: the rewritten SQL (role casts → `c{i}` as for
/// any panel), its roles, and where the payload columns are.
pub(crate) fn plan_contract(stmt: &str) -> Option<(super::Panel, ContractCall)> {
    let (split, call, pos) = split_contract(stmt)?;
    let (sql, roles, names) = super::rewrite(&split);
    // Group keys: the un-annotated, non-star items other than the payload.
    let (sel, end) = super::main_select_list(&split)?;
    let groups = lex::split_top_commas(&split[sel..end])
        .iter()
        .enumerate()
        .filter(|(i, it)| {
            *i != pos
                && *i != pos + 1
                && !roles.iter().any(|(j, _)| j == i)
                && !lex::is_star_item(it)
        })
        .map(|(i, it)| {
            let (core, alias) = lex::trailing_alias(it.trim());
            let name = alias
                .or_else(|| lex::simple_ident(core))
                .unwrap_or_else(|| core.to_string());
            (i, name)
        })
        .collect();
    let panel = super::Panel {
        setup: false,
        sql,
        roles,
        names,
        contract: Some(ContractPanel {
            macro_name: call.macro_name.clone(),
            rows: pos,
            args: pos + 1,
            groups,
        }),
        requires: Vec::new(),
        requires_issues: Vec::new(),
    };
    Some((panel, call))
}

/// How a macro's named parameters land in the renderer spec: the `plot` name
/// and `(parameter, options key)` pairs, read from the macro table
/// ([`crate::macros::MACROS`] bodies built by `contract_body!`). A macro not
/// (yet) in the table renders as plot `<name without anofox_plot_>` with every
/// parameter passed under its own name.
pub fn macro_binding(macro_name: &str) -> (String, Option<Vec<(String, String)>>) {
    let m = macro_name.to_ascii_lowercase();
    let fallback = || m.strip_prefix("anofox_plot_").unwrap_or("auto").to_string();
    let Some(spec) = crate::macros::MACROS.iter().find(|s| s.name == m) else {
        return (fallback(), None);
    };
    let body = spec.body;
    let plot = literal_after(body, "'plot',").unwrap_or_else(fallback);
    let mut map = Vec::new();
    if let Some(at) = body.find("'options',") {
        let rest = &body[at + "'options',".len()..];
        if let Some(open) = rest.find("json_object(") {
            let inner = &rest[open + "json_object(".len()..];
            if let Some(close) = matching_close(inner) {
                let parts = lex::split_top_commas(&inner[..close]);
                for kv in parts.chunks(2) {
                    if let [k, v] = kv {
                        let key = k.trim().trim_matches('\'').to_string();
                        if let Some(param) = lex::simple_ident(v.trim()) {
                            map.push((param.to_ascii_lowercase(), key));
                        }
                    }
                }
            }
        }
    }
    (plot, Some(map))
}

fn literal_after(body: &str, marker: &str) -> Option<String> {
    let rest = body[body.find(marker)? + marker.len()..].trim_start();
    let rest = rest.strip_prefix('\'')?;
    Some(rest[..rest.find('\'')?].to_string())
}

/// Byte index of the `)` closing an already-opened paren.
fn matching_close(s: &str) -> Option<usize> {
    let mut depth = 1i32;
    for t in lex::tokenize(s) {
        if t.kind == TokKind::Punct {
            match t.text(s) {
                "(" => depth += 1,
                ")" => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(t.start);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

/// A JSON payload cell: a JSON text (DuckDB-Wasm, `to_json` as VARCHAR) or an
/// already-nested value (a CLI's `-json` output) — parsed leniently (bare
/// `NaN`/`Infinity` read as null).
fn payload(v: &J) -> Result<J, String> {
    match v {
        J::String(s) => {
            let clean = super::sanitize_json_numbers(s);
            serde_json::from_str(&clean).map_err(|e| format!("payload JSON: {e}"))
        }
        J::Null => Ok(J::Null),
        other => Ok(other.clone()),
    }
}

/// Render one contract call: `rows_json` = the rows as a JSON array (or
/// JSON text of one), `args_json` = the named arguments as a JSON object keyed
/// by parameter name. A macro this build does not know yet renders as a note.
pub fn render_call(
    macro_name: &str,
    rows_json: &str,
    args_json: &str,
    width: u32,
    height: u32,
    primary: &str,
) -> Result<String, RenderError> {
    let parse = |s: &str, what: &str| -> Result<J, RenderError> {
        if s.trim().is_empty() {
            return Ok(J::Null);
        }
        let clean = super::sanitize_json_numbers(s);
        let v: J = serde_json::from_str(&clean)
            .map_err(|e| RenderError::BadSpec(format!("{what} JSON: {e}")))?;
        payload(&v).map_err(RenderError::BadSpec)
    };
    let rows = parse(rows_json, "rows")?;
    let args = parse(args_json, "args")?;
    render_values(macro_name, rows, args, width, height, primary)
}

fn render_values(
    macro_name: &str,
    rows: J,
    args: J,
    width: u32,
    height: u32,
    primary: &str,
) -> Result<String, RenderError> {
    let mut opts = crate::RenderOptions::from_primary_arg(primary).map_err(RenderError::BadSpec)?;
    render_values_opts(macro_name, rows, args, width, height, &mut opts)
}

/// [`render_values`] with explicit render options (theme, locale, brand).
fn render_values_opts(
    macro_name: &str,
    rows: J,
    args: J,
    width: u32,
    height: u32,
    opts: &mut crate::RenderOptions,
) -> Result<String, RenderError> {
    let rows: Vec<Map<String, J>> = match rows {
        J::Null => Vec::new(),
        J::Array(a) => a
            .into_iter()
            .map(|r| match r {
                J::Object(m) => Ok(m),
                _ => Err(RenderError::BadSpec(
                    "`rows` must be an array of objects".into(),
                )),
            })
            .collect::<Result<_, _>>()?,
        _ => return Err(RenderError::BadSpec("rows must be a JSON array".into())),
    };
    let args: Map<String, J> = match args {
        J::Object(m) => m,
        J::Null => Map::new(),
        _ => return Err(RenderError::BadSpec("args must be a JSON object".into())),
    };
    let (plot, binding) = macro_binding(macro_name);
    if !crate::contract::PLOTS.contains(&plot.as_str()) {
        let note = format!("{macro_name}() ships with the next anofox-visualization release");
        return crate::note_svg(None, &note, width, height)
            .finish(crate::Place::Doc)
            .map(|(svg, _)| opts.theme.finish_svg(svg))
            .map_err(RenderError::Render);
    }
    let mut options = Map::new();
    for (k, v) in args {
        let key = match &binding {
            Some(b) => match b.iter().find(|(p, _)| *p == k) {
                Some((_, key)) => key.clone(),
                None if k == "width" || k == "height" => continue,
                None => k,
            },
            None if k == "width" || k == "height" => continue,
            None => k,
        };
        // A nested JSON text (a subquery argument rendered as text) is unwrapped.
        let v = match v {
            J::String(s) if s.trim_start().starts_with(['[', '{']) => {
                payload(&J::String(s.clone())).unwrap_or(J::String(s))
            }
            v => v,
        };
        options.insert(key, v);
    }
    // A `theme :=` / `locale :=` macro argument (or the anofox_theme /
    // anofox_locale variables) overrides the dashboard's render context.
    let mut ctx = Map::new();
    for k in ["theme", "variant", "locale", "primary"] {
        if let Some(v) = options.remove(k) {
            ctx.insert(k.to_string(), v);
        }
    }
    if !ctx.is_empty() {
        opts.apply_context(&J::Object(ctx))
            .map_err(RenderError::BadSpec)?;
    }
    let (w, h) = (
        crate::clamp_dim(width as u64),
        crate::clamp_dim(height as u64),
    );
    crate::guard(|| {
        crate::contract::render(&plot, &rows, &options, w, h, opts, crate::Place::Doc)
            .map(|(svg, warn)| crate::finish_contract_svg(svg, &warn, opts))
    })
}

/// [`render_rows`] with explicit render options (the headless dashboard
/// passes its resolved theme, so custom JSON themes reach contract plots).
pub fn render_rows_opts(
    cp: &ContractPanel,
    rows: &[Map<String, J>],
    width: u32,
    height: u32,
    opts: &crate::RenderOptions,
) -> Vec<(String, Result<String, RenderError>)> {
    rows.iter()
        .map(|r| {
            let label = row_label(cp, r);
            let get = |i: usize| r.get(&format!("c{i}")).cloned().unwrap_or(J::Null);
            let out = payload(&get(cp.rows))
                .and_then(|rows| payload(&get(cp.args)).map(|a| (rows, a)))
                .map_err(RenderError::BadSpec)
                .and_then(|(rows, args)| {
                    render_values_opts(&cp.macro_name, rows, args, width, height, &mut opts.clone())
                });
            (label, out)
        })
        .collect()
}

fn row_label(cp: &ContractPanel, r: &Map<String, J>) -> String {
    cp.groups
        .iter()
        .filter_map(|(i, _)| r.get(&format!("c{i}")))
        .map(|v| match v {
            J::String(s) => s.clone(),
            J::Null => "NULL".into(),
            v => v.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Render every result row of a planned contract panel (one SVG per row, the
/// `GROUP BY` keys as its label) — for hosts that hold the rows as JSON maps
/// keyed `c{i}`.
pub fn render_rows(
    cp: &ContractPanel,
    rows: &[Map<String, J>],
    width: u32,
    height: u32,
    primary: &str,
) -> Vec<(String, Result<String, RenderError>)> {
    match crate::RenderOptions::from_primary_arg(primary) {
        Ok(o) => render_rows_opts(cp, rows, width, height, &o),
        Err(e) => rows
            .iter()
            .map(|r| (row_label(cp, r), Err(RenderError::BadSpec(e.clone()))))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_calls() {
        let c = parse_call("anofox_plot_terms(t)").unwrap();
        assert_eq!(c.macro_name, "anofox_plot_terms");
        assert_eq!(c.alias, "t");
        assert!(c.args.is_empty());
        let c = parse_call("ANOFOX_PLOT_OBS( o , kind := 'qq', label_top:=5 ) AS svg").unwrap();
        assert_eq!(c.macro_name, "anofox_plot_obs");
        assert_eq!(
            c.args,
            vec![
                ("kind".into(), "'qq'".into()),
                ("label_top".into(), "5".into())
            ]
        );
        let c = parse_call(
            "anofox_plot_compare(r, value => 'order_value', tests := (SELECT list(t) FROM x t, y))",
        )
        .unwrap();
        assert_eq!(c.args[1].1, "(SELECT list(t) FROM x t, y)");
        assert_eq!(
            parse_call("anofox_plot(\"My Rows\")").unwrap().alias,
            "My Rows"
        );
        // Not contract calls.
        for bad in [
            "anofox_render(x)",
            "anofox_plot_terms(t) + 1",
            "anofox_plot_terms(t, 'qq')",
            "anofox_plot_terms(t.x)",
            "anofox_plotx(t)",
            "anofox_plot_terms",
            "'anofox_plot_terms(t)'",
        ] {
            assert!(parse_call(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn rewrites_statements() {
        let (sql, call, pos) = split_contract(
            "SELECT anofox_plot_terms(c), 'Coefs'::TITLE FROM tidy c WHERE grp = 'a,b'",
        )
        .unwrap();
        assert_eq!(pos, 0);
        assert_eq!(call.alias, "c");
        assert!(
            sql.contains("to_json(list(\"c\" ORDER BY \"c\")) AS anofox_rows"),
            "{sql}"
        );
        assert!(sql.contains("json_object() AS anofox_args"), "{sql}");
        assert!(sql.contains("WHERE grp = 'a,b'"), "{sql}");
        // Named args become a json_object keyed by parameter.
        let (sql, _, _) =
            split_contract("SELECT anofox_plot_obs(o, kind := 'qq') FROM aug o").unwrap();
        assert!(
            sql.contains("json_object('kind', 'qq') AS anofox_args"),
            "{sql}"
        );
        // Not contract statements.
        for s in [
            "CREATE TABLE x AS SELECT anofox_plot_terms(t) FROM t",
            "SELECT 1",
            "SELECT * FROM (SELECT anofox_plot_terms(t) FROM t)",
            "SELECT 'anofox_plot_terms(t)'::TITLE",
        ] {
            assert!(split_contract(s).is_none(), "{s}");
        }
    }

    #[test]
    fn plans_with_roles_and_groups() {
        let p = &crate::sql::plan(
            "SELECT curve_type, anofox_plot_curve(c), 'Curves'::TITLE FROM curves c GROUP BY curve_type",
        )[0];
        assert!(!p.setup);
        let cp = p.contract.as_ref().unwrap();
        assert_eq!((cp.rows, cp.args), (1, 2));
        assert_eq!(cp.groups, vec![(0, "curve_type".to_string())]);
        assert_eq!(p.roles, vec![(3, crate::Role::Title)]);
        assert!(p.sql.contains("curve_type AS c0"), "{}", p.sql);
        assert!(p.sql.contains("AS c1"), "{}", p.sql);
        assert!(p.sql.contains("json_object() AS c2"), "{}", p.sql);
        assert!(p.sql.contains("'Curves' AS c3"), "{}", p.sql);
        // Without a title it is still a panel, not setup.
        let p = &crate::sql::plan("SELECT anofox_plot(t) FROM x t")[0];
        assert!(!p.setup && p.roles.is_empty() && p.contract.is_some());
        // Ordinary panels carry no contract.
        assert!(crate::sql::plan("SELECT a::XAXIS, b::BARCHART FROM t")[0]
            .contract
            .is_none());
    }

    #[test]
    fn binds_macro_options() {
        let (plot, b) = macro_binding("anofox_plot_terms");
        assert_eq!(plot, "terms");
        let b = b.unwrap();
        for k in [
            "highlight",
            "lambda_min",
            "reference",
            "title",
            "x_label",
            "y_label",
        ] {
            assert!(b.contains(&(k.into(), k.into())), "{k}");
        }
        let (plot, b) = macro_binding("anofox_plot_obs");
        assert_eq!(plot, "obs");
        let b = b.unwrap();
        assert!(b.contains(&("kind".into(), "kind".into())));
        assert!(b.contains(&("label_top".into(), "label_top".into())));
        assert_eq!(macro_binding("anofox_plot").0, "auto");
        // Every bundled contract macro resolves to a known plot.
        for m in crate::macros::MACROS
            .iter()
            .filter(|m| is_contract_macro(m.name))
        {
            let (plot, _) = macro_binding(m.name);
            assert!(
                crate::contract::PLOTS.contains(&plot.as_str()),
                "{}",
                m.name
            );
        }
        // Unknown (future) macros fall back to their suffix.
        assert_eq!(macro_binding("anofox_plot_wibble"), ("wibble".into(), None));
    }

    #[test]
    fn renders_calls() {
        let rows = r#"[{"term":"x1","estimate":0.5,"conf_low":0.2,"conf_high":0.8},
                       {"term":"x2","estimate":-0.3,"conf_low":-0.7,"conf_high":NaN}]"#;
        let svg = render_call("anofox_plot_terms", rows, "{}", 600, 360, "").unwrap();
        assert!(svg.starts_with("<svg"), "{svg}");
        // Rows as JSON *text* (DuckDB-Wasm) work too, and width/height args are ignored.
        let txt = serde_json::to_string(rows).unwrap();
        let svg = render_call("anofox_plot_terms", &txt, r#"{"width":10}"#, 600, 360, "").unwrap();
        assert!(svg.contains("x1"));
        // Options reach the renderer (a bad kind is the renderer's error).
        let obs = r#"[{"row_id":1,"fitted":1,"residual":0.1},{"row_id":2,"fitted":2,"residual":-0.2},
                      {"row_id":3,"fitted":3,"residual":0.05}]"#;
        let e = render_call("anofox_plot_obs", obs, r#"{"kind":"nope"}"#, 600, 360, "")
            .unwrap_err()
            .to_string();
        assert!(e.contains("kind 'nope'"), "{e}");
        assert!(render_call("anofox_plot_obs", obs, r#"{"kind":"qq"}"#, 600, 360, "").is_ok());
        // A macro this build doesn't have yet is a note, not an error.
        let svg = render_call("anofox_plot_wibble", "[]", "{}", 600, 360, "").unwrap();
        assert!(svg.contains("next anofox-visualization release"), "{svg}");
        // Malformed input is a clean error.
        assert!(render_call("anofox_plot_terms", "{", "{}", 600, 360, "").is_err());
        assert!(render_call("anofox_plot_terms", "[]", "[1]", 600, 360, "").is_err());
    }

    #[test]
    fn renders_result_rows() {
        let p = &crate::sql::plan(
            "SELECT curve_type, anofox_plot_curve(c) FROM curves c GROUP BY curve_type",
        )[0];
        let cp = p.contract.as_ref().unwrap();
        let rows: Vec<Map<String, J>> = serde_json::from_str(
            r#"[{"c0":"roc","c1":"[{\"curve_type\":\"roc\",\"x\":0,\"y\":0},{\"curve_type\":\"roc\",\"x\":1,\"y\":1}]","c2":"{}"},
                {"c0":"pr","c1":[{"curve_type":"pr","x":0,"y":1},{"curve_type":"pr","x":1,"y":0.5}],"c2":{}}]"#,
        )
        .unwrap();
        let out = render_rows(cp, &rows, 500, 300, "");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].0, "roc");
        assert!(out
            .iter()
            .all(|(_, r)| r.as_ref().is_ok_and(|s| s.starts_with("<svg"))));
    }
}
