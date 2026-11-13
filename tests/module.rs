//! #1156: a module's roles parse, list and render; clashes are refused.
#[path = "../examples/hello_module.rs"]
mod hello_module;

use anofox_visualization::module::{self, Module, ModuleRole};
use anofox_visualization::{parse_role, render, roles, Column, RenderOptions, Role, Value};
use std::sync::Once;

fn setup() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| module::register(&hello_module::HELLO).unwrap());
}

#[test]
fn a_module_role_parses_and_lists() {
    setup();
    let role = parse_role("hello").expect("::HELLO parses");
    assert!(matches!(role, Role::Module(_)));
    assert_eq!(parse_role("GREETING"), Some(role), "the alias");
    assert_eq!(role.token(), "HELLO");
    assert!(role.renders());
    assert_eq!(role.category(), "module");
    assert!(
        roles::text().contains("::HELLO"),
        "dashboard --roles lists it"
    );
    assert!(
        roles::roles_json().contains("\"token\":\"HELLO\""),
        "the browser's list"
    );
    assert_eq!(module::names(), vec!["hello"]);
    // the built-in roles are unchanged
    assert_eq!(parse_role("BARCHART").map(|r| r.token()), Some("BARCHART"));
}

#[test]
fn a_module_draws_its_panel() {
    setup();
    let role = parse_role("HELLO").unwrap();
    let svg = render(
        &[Column::new("who", role, vec![Value::Str("<world>".into())])],
        320,
        120,
    )
    .unwrap();
    assert!(svg.starts_with("<svg"), "{svg}");
    assert!(svg.contains(">Hello, &lt;world&gt;</text>"), "{svg}");
}

struct Clash(&'static str, &'static str);
impl Module for Clash {
    fn name(&self) -> &'static str {
        self.0
    }
    fn roles(&self) -> &'static [ModuleRole] {
        match self.1 {
            "BARCHART" => &[ModuleRole {
                token: "BARCHART",
                aliases: &[],
                summary: "",
            }],
            _ => &[ModuleRole {
                token: "HELLO2",
                aliases: &["greeting"],
                summary: "",
            }],
        }
    }
    fn render(&self, _: &[Column], _: u32, _: u32, _: &RenderOptions) -> Result<String, String> {
        Ok(String::new())
    }
}

#[test]
fn clashes_are_refused() {
    setup();
    static A: Clash = Clash("bars", "BARCHART");
    static B: Clash = Clash("hello2", "HELLO2");
    static C: Clash = Clash("hello", "OTHER");
    let e = module::register(&A).unwrap_err();
    assert!(e.contains("::BARCHART is a role already"), "{e}");
    let e = module::register(&B).unwrap_err();
    assert!(e.contains("::greeting is a role already"), "{e}");
    let e = module::register(&C).unwrap_err();
    assert!(e.contains("registered already"), "{e}");
    assert!(
        parse_role("HELLO2").is_none(),
        "a refused module adds nothing"
    );
}
