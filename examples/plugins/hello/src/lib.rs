//! `::HELLO`: a greeting per row, in the theme's accent colour (#1213).
use anofox_viz_plugin::{
    escape, export_plugin, Column, ContractInput, ContractMacro, ContractOption, Manifest, Plugin,
    RenderInput, Role,
};

struct Hello;

impl Plugin for Hello {
    const MANIFEST: Manifest = Manifest {
        name: "hello",
        roles: &[Role {
            token: "HELLO",
            aliases: &["GREETING"],
            summary: "a greeting per row (example plugin)",
        }],
    };

    fn render(input: &RenderInput) -> Result<String, String> {
        let who = input.role("HELLO").ok_or("no ::HELLO column")?;
        let mut svg = String::new();
        for i in 0..who.values.len() {
            let name = who.str(i).ok_or("::HELLO takes text")?;
            svg.push_str(&format!(
                "<text x=\"16\" y=\"{}\" font-size=\"18\" fill=\"{}\">Hello, {}!</text>",
                32 + 26 * i,
                escape(&input.theme.accent),
                escape(name)
            ));
        }
        Ok(svg)
    }

    // `SELECT anofox_plot_hello(t, greeting := 'Hi') FROM people t`: a
    // greeting per row's `name`.
    const MACROS: &'static [ContractMacro] = &[ContractMacro {
        plot: "hello",
        named: &[("greeting", "'Hello'"), ("height", "200")],
        options: &[ContractOption {
            key: "greeting",
            param: "greeting",
            fallback_var: None,
        }],
        description: "Greets every row's name (example plugin).",
        example: "SELECT anofox_plot_hello(t) FROM (VALUES ('Ada'), ('Max')) t(name)",
    }];

    fn render_contract(input: &ContractInput) -> Result<String, String> {
        let greeting = input
            .options
            .get("greeting")
            .and_then(|g| g.as_str())
            .unwrap_or("Hello");
        let mut svg = String::new();
        for (i, row) in input.rows.iter().enumerate() {
            let name = row
                .get("name")
                .and_then(|n| n.as_str())
                .ok_or("no name column")?;
            svg.push_str(&format!(
                "<text class=\"hello\" x=\"16\" y=\"{}\" font-size=\"18\">{}, {}!</text>",
                32 + 26 * i,
                escape(greeting),
                escape(name)
            ));
        }
        Ok(svg)
    }

    const ASSETS: &'static [(&'static str, &'static [u8])] =
        &[("hello.css", b".hello { font-style: italic; }\n")];

    fn lint(columns: &[Column]) -> Vec<String> {
        match columns.iter().find(|c| c.role == "HELLO") {
            Some(c) if c.values.len() > 10 => {
                vec![format!(
                    "::HELLO greets {} rows; more than 10 will not fit",
                    c.values.len()
                )]
            }
            _ => Vec::new(),
        }
    }
}

export_plugin!(Hello);
