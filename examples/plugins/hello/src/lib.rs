//! `::HELLO`: a greeting per row, in the theme's accent colour (#1213).
use anofox_viz_plugin::{escape, export_plugin, Column, Manifest, Plugin, RenderInput, Role};

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
