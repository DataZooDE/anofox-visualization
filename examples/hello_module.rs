//! A module (#1156): a trivial one, `::HELLO`, that greets the statement's
//! first value. A distribution crate registers its modules once at start-up;
//! a statement carrying one of a module's roles is then drawn by it.
//!
//!     cargo run --example hello_module
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
