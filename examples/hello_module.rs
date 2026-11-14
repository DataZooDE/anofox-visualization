//! A module (#1156): a trivial one, `::HELLO`, that greets the statement's
//! first value. A distribution crate registers its modules once at start-up;
//! a statement carrying one of a module's roles is then drawn by it.
//!
//!     cargo run --example hello_module
use anofox_visualization::macros::MacroSpec;
use anofox_visualization::module::{self, Module, ModuleRole};
use anofox_visualization::{parse_role, render, value_text, Column, RenderOptions, Value};

pub struct Hello;

impl Module for Hello {
    fn name(&self) -> &'static str {
        "hello"
    }
    fn roles(&self) -> &'static [ModuleRole] {
        &[ModuleRole {
            token: "HELLO",
            aliases: &["GREETING"],
            summary: "a greeting: \"Hello, <value>\" in the middle of the panel",
        }]
    }
    fn render(
        &self,
        cols: &[Column],
        width: u32,
        height: u32,
        o: &RenderOptions,
    ) -> Result<String, String> {
        let col = cols
            .iter()
            .find(|c| c.role == parse_role("HELLO").unwrap())
            .ok_or("no ::HELLO column")?;
        let who = col.values.first().map(value_text).unwrap_or_default();
        let (r, g, b) = o.theme.colors.heading;
        Ok(format!(
            "<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" dominant-baseline=\"middle\" font-size=\"20\" fill=\"#{r:02X}{g:02X}{b:02X}\">Hello, {}</text>",
            width / 2,
            height / 2,
            anofox_visualization::format::escape_xml(&who)
        ))
    }
    /// A contract macro: `SELECT anofox_plot_hello(t, greeting := 'Hi') FROM
    /// people t` greets every row's `name`.
    fn macros(&self) -> &'static [MacroSpec] {
        &[MacroSpec {
            name: "anofox_plot_hello",
            params: &["tbl"],
            named: &[
                ("greeting", "'Hello'"),
                ("theme", "NULL"),
                ("locale", "NULL"),
                ("width", "640"),
                ("height", "200"),
            ],
            body: anofox_visualization::contract_body!("hello", "'greeting', greeting"),
            description: "Greets every row's name.",
            example: "SELECT anofox_plot_hello(t) FROM (VALUES ('Ada'), ('Max')) t(name)",
            tags: &[],
        }]
    }
    fn render_contract(
        &self,
        plot: &str,
        rows: &[serde_json::Map<String, serde_json::Value>],
        options: &serde_json::Map<String, serde_json::Value>,
        width: u32,
        height: u32,
        _o: &RenderOptions,
    ) -> Option<Result<String, String>> {
        if plot != "hello" {
            return None;
        }
        let names: Vec<String> = rows
            .iter()
            .filter_map(|r| r.get("name").and_then(|v| v.as_str()).map(str::to_string))
            .collect();
        if names.is_empty() {
            return Some(Err("anofox_plot_hello needs a name column".into()));
        }
        let greeting = options
            .get("greeting")
            .and_then(|v| v.as_str())
            .unwrap_or("Hello");
        Some(Ok(format!(
            "<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" font-size=\"18\">{}, {}</text>",
            width / 2,
            height / 2,
            anofox_visualization::format::escape_xml(greeting),
            anofox_visualization::format::escape_xml(&names.join(" and "))
        )))
    }
    /// Its browser files, served at `/modules/hello/…`.
    fn assets(&self) -> &'static [(&'static str, &'static [u8])] {
        &[
            ("hello.js", include_bytes!("hello_assets/hello.js")),
            ("hello.css", include_bytes!("hello_assets/hello.css")),
        ]
    }
    /// `--check` warns about a greeting to nobody.
    fn lint(&self, cols: &[Column]) -> Vec<String> {
        let empty = cols
            .iter()
            .filter(|c| c.role == parse_role("HELLO").unwrap())
            .flat_map(|c| c.values.first())
            .any(|v| value_text(v).trim().is_empty());
        if empty {
            vec!["the ::HELLO value is empty — it greets nobody".to_string()]
        } else {
            Vec::new()
        }
    }
}

pub static HELLO: Hello = Hello;

#[allow(dead_code)]
fn main() -> Result<(), String> {
    module::register(&HELLO)?;
    // SELECT 'world'::HELLO
    let role = parse_role("HELLO").ok_or("::HELLO is not a role")?;
    let svg = render(
        &[Column::new("who", role, vec![Value::Str("world".into())])],
        320,
        120,
    )?;
    println!("{svg}");
    Ok(())
}
