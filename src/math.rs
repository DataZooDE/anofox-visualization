//! TeX math in `::MARKDOWN` text: `$…$` inline and `$$…$$` display formulas.
//!
//! The browser renders them with KaTeX (`web/app.js` `mathSpans`, the same
//! rules as [`spans`]); headless output shows the TeX source in a monospace
//! font, and `--check` reports a formula KaTeX can't parse ([`check`]).
//!
//! Delimiters follow KaTeX auto-render / pandoc: an opening `$` needs a
//! non-space right after it, a closing `$` a non-space right before it and no
//! digit right after it — so "$5 and $10" stays text. An inline formula
//! ends at its first unescaped `$`. `\$` is a literal dollar
//! and a `` `code` `` span is never math.

/// A piece of Markdown text: plain text or a formula.
#[derive(Debug, Clone, PartialEq)]
pub enum Seg {
    Text(String),
    Math { tex: String, display: bool },
}

/// Split `s` into text and formulas. Text has its `\$` escapes resolved.
pub fn spans(s: &str) -> Vec<Seg> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut text = String::new();
    let mut i = 0;
    let flush = |text: &mut String, out: &mut Vec<Seg>| {
        if !text.is_empty() {
            out.push(Seg::Text(std::mem::take(text)));
        }
    };
    while i < b.len() {
        let rest = &s[i..];
        if rest.starts_with("\\$") {
            text.push('$');
            i += 2;
            continue;
        }
        if let Some(code) = rest.strip_prefix('`') {
            let end = code.find('`').map_or(1, |e| e + 2);
            text.push_str(&rest[..end]);
            i += end;
            continue;
        }
        if let Some(inner) = rest.strip_prefix("$$") {
            if let Some(e) = inner.find("$$") {
                let tex = inner[..e].trim();
                if !tex.is_empty() {
                    flush(&mut text, &mut out);
                    out.push(Seg::Math {
                        tex: tex.to_string(),
                        display: true,
                    });
                    i += e + 4;
                    continue;
                }
            }
            text.push_str("$$");
            i += 2;
            continue;
        }
        if rest.starts_with('$') {
            if let Some(e) = inline_close(rest) {
                flush(&mut text, &mut out);
                out.push(Seg::Math {
                    tex: rest[1..e].to_string(),
                    display: false,
                });
                i += e + 1;
                continue;
            }
        }
        let c = rest.chars().next().unwrap_or(' ');
        text.push(c);
        i += c.len_utf8();
    }
    flush(&mut text, &mut out);
    out
}

/// The closing `$` of an inline formula opening at `rest[0]`: the next
/// unescaped `$`, if it may close (a non-space before it, no digit or `$`
/// after it) — otherwise that `$` is no formula.
fn inline_close(rest: &str) -> Option<usize> {
    let b = rest.as_bytes();
    if b.len() < 3 || b[1].is_ascii_whitespace() || b[1] == b'$' {
        return None;
    }
    let j = (2..b.len()).find(|&j| b[j] == b'$' && b[j - 1] != b'\\')?;
    let after = b.get(j + 1);
    (!b[j - 1].is_ascii_whitespace() && !after.is_some_and(|c| c.is_ascii_digit() || *c == b'$'))
        .then_some(j)
}

/// Commands KaTeX defines (generated from its sources, one per line).
const COMMANDS: &str = include_str!("katex_commands.txt");

/// KaTeX environments (`\begin{…}`).
const ENVIRONMENTS: &[&str] = &[
    "Bmatrix",
    "Bmatrix*",
    "CD",
    "Vmatrix",
    "Vmatrix*",
    "align",
    "align*",
    "alignat",
    "alignat*",
    "aligned",
    "alignedat",
    "array",
    "bmatrix",
    "bmatrix*",
    "cases",
    "darray",
    "dcases",
    "drcases",
    "equation",
    "equation*",
    "gather",
    "gather*",
    "gathered",
    "matrix",
    "matrix*",
    "pmatrix",
    "pmatrix*",
    "rcases",
    "smallmatrix",
    "split",
    "subarray",
    "vmatrix",
    "vmatrix*",
];

/// Commands that link, load or tag HTML: off in the dashboard (KaTeX
/// `trust: false` draws them as an error).
const UNTRUSTED: &[&str] = &[
    "href",
    "url",
    "includegraphics",
    "htmlClass",
    "htmlId",
    "htmlStyle",
    "htmlData",
];

/// Whether KaTeX can typeset `tex` — balanced braces, `\left`/`\right` and
/// `\begin`/`\end`, known commands and environments. `{{col}}` placeholders
/// count as plain text. `Err` says what is wrong.
pub fn check(tex: &str) -> Result<(), String> {
    let b = tex.as_bytes();
    let (mut depth, mut lr) = (0i32, 0i32);
    let mut envs: Vec<String> = Vec::new();
    let mut i = 0;
    // `{name}` right after `\begin` / `\end`
    let group = |i: usize| -> Option<(String, usize)> {
        let r = tex[i..].trim_start();
        let at = i + (tex[i..].len() - r.len());
        let r = r.strip_prefix('{')?;
        let e = r.find('}')?;
        Some((r[..e].trim().to_string(), at + e + 2))
    };
    while i < b.len() {
        match b[i] {
            b'{' if tex[i..].starts_with("{{") => {
                if let Some(e) = tex[i + 2..].find("}}") {
                    let inner = &tex[i + 2..i + 2 + e];
                    if !inner.contains(['{', '}', '\\']) {
                        i += e + 4;
                        continue;
                    }
                }
                depth += 1;
                i += 1;
            }
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                depth -= 1;
                if depth < 0 {
                    return Err("a `}` closes nothing".into());
                }
                i += 1;
            }
            b'%' => i = tex[i..].find('\n').map_or(b.len(), |e| i + e + 1),
            b'\\' => {
                let n = tex[i + 1..]
                    .bytes()
                    .take_while(u8::is_ascii_alphabetic)
                    .count();
                if n == 0 {
                    // \{ \} \, \\ … — a control symbol
                    i += 1 + tex[i + 1..].chars().next().map_or(0, char::len_utf8);
                    continue;
                }
                let name = &tex[i + 1..i + 1 + n];
                i += 1 + n;
                if UNTRUSTED.contains(&name) {
                    return Err(format!("\\{name} is turned off in dashboards"));
                }
                match name {
                    "left" => lr += 1,
                    "right" => {
                        lr -= 1;
                        if lr < 0 {
                            return Err("\\right without a \\left".into());
                        }
                    }
                    "begin" | "end" => {
                        let Some((env, next)) = group(i) else {
                            return Err(format!("\\{name} needs an environment name"));
                        };
                        i = next;
                        if name == "begin" {
                            if !ENVIRONMENTS.contains(&env.as_str()) {
                                return Err(format!("unknown environment `{env}`"));
                            }
                            envs.push(env);
                        } else if envs.pop().as_deref() != Some(env.as_str()) {
                            return Err(format!("\\end{{{env}}} does not match its \\begin"));
                        }
                    }
                    _ if !COMMANDS.lines().any(|c| c == name) => {
                        return Err(format!("unknown command \\{name}"));
                    }
                    _ => {}
                }
            }
            _ => i += tex[i..].chars().next().map_or(1, char::len_utf8),
        }
    }
    if depth > 0 {
        return Err("a `{` is never closed".into());
    }
    if lr > 0 {
        return Err("\\left without a \\right".into());
    }
    if let Some(env) = envs.pop() {
        return Err(format!("\\begin{{{env}}} without \\end{{{env}}}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn math(s: &str) -> Vec<(String, bool)> {
        spans(s)
            .into_iter()
            .filter_map(|g| match g {
                Seg::Math { tex, display } => Some((tex, display)),
                Seg::Text(_) => None,
            })
            .collect()
    }

    #[test]
    fn delimiters_follow_auto_render() {
        assert_eq!(math("a $x^2$ b"), vec![("x^2".into(), false)]);
        assert_eq!(math("$$ \\hat{y}_t $$"), vec![("\\hat{y}_t".into(), true)]);
        // amounts, escapes and code stay text
        assert!(math("costs $5 and $10 today").is_empty());
        assert_eq!(
            math("$5 and $10, then $x$ and $$y$$"),
            vec![("x".into(), false), ("y".into(), true)]
        );
        assert!(math("a $ x$ and $x $ b").is_empty());
        assert!(math("`$x$` and \\$y\\$").is_empty());
        assert_eq!(spans("pay \\$5"), vec![Seg::Text("pay $5".into())]);
        // a closing $ before a digit doesn't close
        assert!(math("$a$1").is_empty());
        assert_eq!(math("$\\$5$"), vec![("\\$5".into(), false)]);
        assert_eq!(math("ünï $α$ ok"), vec![("α".into(), false)]);
    }

    #[test]
    fn check_finds_what_katex_rejects() {
        assert!(check("\\hat{y}_t = \\alpha y_t + (1-\\alpha)\\hat{y}_{t-1}").is_ok());
        assert!(check("\\left( \\frac{a}{b} \\right) \\{x\\}").is_ok());
        assert!(check("\\begin{aligned} a &= b \\\\ c &= d \\end{aligned}").is_ok());
        assert!(check("\\text{{{growth:pct}}} \\cdot {{n}}").is_ok());
        assert!(check("\\frac{a}{b").unwrap_err().contains("never closed"));
        assert!(check("a}").is_err());
        assert!(check("\\foo x").unwrap_err().contains("\\foo"));
        assert!(check("\\left( x").is_err());
        assert!(check("\\begin{matrix} x \\end{pmatrix}").is_err());
        assert!(check("\\begin{nope} x \\end{nope}").is_err());
        assert!(check("\\href{https://x}{y}")
            .unwrap_err()
            .contains("turned off"));
    }
}
