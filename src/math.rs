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

/// A small, closed set of TeX commands [`tex_to_text`] turns into the single
/// visible character KaTeX would draw for them. Anything outside this list
/// is left exactly as written — safe by construction, since it can never
/// print the *wrong* symbol for a command it doesn't recognise, only that
/// command's own `\name` source (the previous, always-correct fallback).
const SYMBOLS: &[(&str, &str)] = &[
    ("alpha", "α"),
    ("beta", "β"),
    ("gamma", "γ"),
    ("delta", "δ"),
    ("epsilon", "ε"),
    ("varepsilon", "ε"),
    ("zeta", "ζ"),
    ("eta", "η"),
    ("theta", "θ"),
    ("vartheta", "ϑ"),
    ("iota", "ι"),
    ("kappa", "κ"),
    ("lambda", "λ"),
    ("mu", "μ"),
    ("nu", "ν"),
    ("xi", "ξ"),
    ("pi", "π"),
    ("varpi", "ϖ"),
    ("rho", "ρ"),
    ("varrho", "ϱ"),
    ("sigma", "σ"),
    ("varsigma", "ς"),
    ("tau", "τ"),
    ("upsilon", "υ"),
    ("phi", "φ"),
    ("varphi", "ϕ"),
    ("chi", "χ"),
    ("psi", "ψ"),
    ("omega", "ω"),
    ("Alpha", "Α"),
    ("Beta", "Β"),
    ("Gamma", "Γ"),
    ("Delta", "Δ"),
    ("Epsilon", "Ε"),
    ("Zeta", "Ζ"),
    ("Eta", "Η"),
    ("Theta", "Θ"),
    ("Iota", "Ι"),
    ("Kappa", "Κ"),
    ("Lambda", "Λ"),
    ("Mu", "Μ"),
    ("Nu", "Ν"),
    ("Xi", "Ξ"),
    ("Pi", "Π"),
    ("Rho", "Ρ"),
    ("Sigma", "Σ"),
    ("Tau", "Τ"),
    ("Upsilon", "Υ"),
    ("Phi", "Φ"),
    ("Chi", "Χ"),
    ("Psi", "Ψ"),
    ("Omega", "Ω"),
    ("cdot", "·"),
    ("times", "×"),
    ("div", "÷"),
    ("pm", "±"),
    ("mp", "∓"),
    ("leq", "≤"),
    ("le", "≤"),
    ("geq", "≥"),
    ("ge", "≥"),
    ("neq", "≠"),
    ("ne", "≠"),
    ("approx", "≈"),
    ("sim", "∼"),
    ("equiv", "≡"),
    ("propto", "∝"),
    ("infty", "∞"),
    ("partial", "∂"),
    ("nabla", "∇"),
    ("sum", "Σ"),
    ("prod", "Π"),
    ("int", "∫"),
    ("to", "→"),
    ("rightarrow", "→"),
    ("leftarrow", "←"),
    ("leftrightarrow", "↔"),
    ("Rightarrow", "⇒"),
    ("Leftarrow", "⇐"),
    ("in", "∈"),
    ("notin", "∉"),
    ("subset", "⊂"),
    ("supset", "⊃"),
    ("cup", "∪"),
    ("cap", "∩"),
    ("forall", "∀"),
    ("exists", "∃"),
    ("emptyset", "∅"),
    ("ldots", "…"),
    ("cdots", "⋯"),
    ("dots", "…"),
    ("circ", "∘"),
    ("perp", "⊥"),
    ("parallel", "∥"),
    ("hbar", "ℏ"),
    ("ell", "ℓ"),
    ("top", "⊤"),
    ("angle", "∠"),
    ("sqrt", "√"),
    ("quad", " "),
    ("qquad", "  "),
];

/// A digit, sign or the few letters Unicode has a superscript form for
/// (`x^2`, `e^{-x}`, `x^n`).
fn sup_char(c: char) -> Option<char> {
    Some(match c {
        '0' => '⁰',
        '1' => '¹',
        '2' => '²',
        '3' => '³',
        '4' => '⁴',
        '5' => '⁵',
        '6' => '⁶',
        '7' => '⁷',
        '8' => '⁸',
        '9' => '⁹',
        '+' => '⁺',
        '-' => '⁻',
        '=' => '⁼',
        '(' => '⁽',
        ')' => '⁾',
        'n' => 'ⁿ',
        'i' => 'ⁱ',
        _ => return None,
    })
}

/// A digit, sign or the few letters Unicode has a subscript form for
/// (`x_i`, `y_{t-1}`).
fn sub_char(c: char) -> Option<char> {
    Some(match c {
        '0' => '₀',
        '1' => '₁',
        '2' => '₂',
        '3' => '₃',
        '4' => '₄',
        '5' => '₅',
        '6' => '₆',
        '7' => '₇',
        '8' => '₈',
        '9' => '₉',
        '+' => '₊',
        '-' => '₋',
        '=' => '₌',
        '(' => '₍',
        ')' => '₎',
        'a' => 'ₐ',
        'e' => 'ₑ',
        'h' => 'ₕ',
        'i' => 'ᵢ',
        'j' => 'ⱼ',
        'k' => 'ₖ',
        'l' => 'ₗ',
        'm' => 'ₘ',
        'n' => 'ₙ',
        'o' => 'ₒ',
        'p' => 'ₚ',
        'r' => 'ᵣ',
        's' => 'ₛ',
        't' => 'ₜ',
        'u' => 'ᵤ',
        'v' => 'ᵥ',
        'x' => 'ₓ',
        _ => return None,
    })
}

/// A `{…}` group starting at `chars[i]` (balanced, possibly nested): its
/// inner text and the index right after the closing `}`. `None` if `i`
/// isn't a `{`, or the group is never closed (left for the caller to show
/// as plain text — safe, since nothing is silently dropped).
fn brace_group(chars: &[char], i: usize) -> Option<(String, usize)> {
    if chars.get(i) != Some(&'{') {
        return None;
    }
    let mut depth = 1;
    let mut j = i + 1;
    while j < chars.len() {
        match chars[j] {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((chars[i + 1..j].iter().collect(), j + 1));
                }
            }
            _ => {}
        }
        j += 1;
    }
    None
}

/// A `^`/`_` argument: a `{…}` group, else the single next character.
/// `None` at the end of the string.
fn sup_sub_token(chars: &[char], i: usize) -> Option<(String, usize)> {
    if let Some(g) = brace_group(chars, i) {
        return Some(g);
    }
    chars.get(i).map(|&c| (c.to_string(), i + 1))
}

/// Whether `s` needs parentheses as a `\frac` numerator/denominator —
/// it has more than one term, so `a+b/c` (no parens) would misread as
/// `a + b/c` rather than `(a+b)/c`. A plain name or a single converted
/// symbol never does.
fn needs_parens(s: &str) -> bool {
    s.contains([' ', '+', '-', '*', '/'])
}

/// Render simple TeX as plain text for headless/static output, which has no
/// KaTeX (#351): Greek letters and a handful of common symbols (`cdot`,
/// `leq`, `infty`, …), `\frac{a}{b}` as `a/b`, and a `^`/`_` argument made of
/// digits/signs/the few letters Unicode has super-/subscript forms for
/// (`x^2`, `x_i`) as that Unicode character. Everything else — an
/// unrecognised command, a matrix, an accent, a `^`/`_` argument with no
/// Unicode form — is left exactly as written, same as before this existed:
/// never the wrong symbol, at worst still the TeX source.
pub fn tex_to_text(tex: &str) -> String {
    let chars: Vec<char> = tex.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '\\' => {
                let Some(&next) = chars.get(i + 1) else {
                    out.push('\\');
                    break;
                };
                if !next.is_ascii_alphabetic() {
                    // a control symbol: \\ \{ \} \$ \% \_ \& \# …
                    out.push(next);
                    i += 2;
                    continue;
                }
                let start = i + 1;
                let mut j = start;
                while chars.get(j).is_some_and(char::is_ascii_alphabetic) {
                    j += 1;
                }
                let name: String = chars[start..j].iter().collect();
                if name == "frac" {
                    let mut k = j;
                    while chars.get(k).is_some_and(|c| c.is_ascii_whitespace()) {
                        k += 1;
                    }
                    let num = brace_group(&chars, k);
                    let (num, mut k) = match num {
                        Some((n, nk)) => (n, nk),
                        None => {
                            out.push_str("\\frac");
                            i = j;
                            continue;
                        }
                    };
                    while chars.get(k).is_some_and(|c| c.is_ascii_whitespace()) {
                        k += 1;
                    }
                    match brace_group(&chars, k) {
                        Some((den, dk)) => {
                            let (n, d) = (tex_to_text(&num), tex_to_text(&den));
                            let n = if needs_parens(&n) {
                                format!("({n})")
                            } else {
                                n
                            };
                            let d = if needs_parens(&d) {
                                format!("({d})")
                            } else {
                                d
                            };
                            out.push_str(&n);
                            out.push('/');
                            out.push_str(&d);
                            k = dk;
                        }
                        None => {
                            out.push_str("\\frac{");
                            out.push_str(&num);
                            out.push('}');
                        }
                    }
                    i = k;
                    continue;
                }
                match SYMBOLS.iter().find(|&&(n, _)| n == name) {
                    Some(&(_, sym)) => out.push_str(sym),
                    None => {
                        // An unrecognised command (`\text`, `\hat`, `\mathrm`,
                        // an accent, a macro, …) that takes a `{…}` argument:
                        // copy the whole `\name{…}` through byte for byte,
                        // unconverted — the argument's braces are *its*
                        // syntax, not free-standing grouping to drop, and a
                        // command this converter doesn't understand is the
                        // one case it must not partially rewrite (it could
                        // easily end up with a different, wrong meaning).
                        if let Some((_, gk)) = brace_group(&chars, j) {
                            out.extend(&chars[i..gk]);
                            i = gk;
                            continue;
                        }
                        out.push('\\');
                        out.push_str(&name);
                    }
                }
                i = j;
            }
            '^' | '_' => {
                let sup = chars[i] == '^';
                if let Some((tok, ni)) = sup_sub_token(&chars, i + 1) {
                    let mapped: Option<String> = tok
                        .chars()
                        .map(|c| if sup { sup_char(c) } else { sub_char(c) })
                        .collect::<Option<Vec<char>>>()
                        .map(|cs| cs.into_iter().collect());
                    if let Some(s) = mapped.filter(|s| !s.is_empty()) {
                        out.push_str(&s);
                        i = ni;
                        continue;
                    }
                }
                out.push(chars[i]);
                i += 1;
            }
            '{' => match brace_group(&chars, i) {
                // A bare `{…}` group (not a `\frac`/`^`/`_` argument): pure
                // TeX grouping, invisible once typeset — drop the braces,
                // keep converting its contents.
                Some((inner, ni)) => {
                    out.push_str(&tex_to_text(&inner));
                    i = ni;
                }
                None => {
                    out.push('{');
                    i += 1;
                }
            },
            '}' => i += 1, // an unmatched close: drop, nothing to pair it with
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

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

    #[test]
    fn tex_to_text_converts_the_simple_cases() {
        assert_eq!(tex_to_text("\\lambda"), "λ");
        assert_eq!(tex_to_text("\\alpha + \\beta"), "α + β");
        assert_eq!(tex_to_text("\\frac{a}{b}"), "a/b");
        assert_eq!(tex_to_text("a \\cdot b"), "a · b");
        assert_eq!(tex_to_text("x^2"), "x²");
        assert_eq!(tex_to_text("x_i"), "xᵢ");
        assert_eq!(tex_to_text("x^{10}"), "x¹⁰");
        assert_eq!(tex_to_text("y_{t-1}"), "yₜ₋₁");
        // the #351 example: `\hat{…}` (an accent) is outside this small
        // converter's scope, so it's left as TeX source — safe, not wrong —
        // while the rest (Greek letters, subscripts) still reads as text.
        assert_eq!(
            tex_to_text("\\hat{y}_t = \\alpha y_t + (1-\\alpha)\\hat{y}_{t-1}"),
            "\\hat{y}ₜ = α yₜ + (1-α)\\hat{y}ₜ₋₁"
        );
    }

    #[test]
    fn tex_to_text_frac_parenthesises_multi_term_operands() {
        assert_eq!(tex_to_text("\\frac{a+b}{c}"), "(a+b)/c");
        assert_eq!(tex_to_text("\\frac{n}{2}"), "n/2");
        assert_eq!(tex_to_text("\\frac{\\alpha}{\\beta}"), "α/β");
        // nested: the numerator is itself a fraction
        assert_eq!(tex_to_text("\\frac{\\frac{a}{b}}{c}"), "(a/b)/c");
    }

    #[test]
    fn tex_to_text_drops_bare_grouping_braces() {
        assert_eq!(tex_to_text("{x+y}"), "x+y");
        assert_eq!(tex_to_text("a{b}c"), "abc");
    }

    #[test]
    fn tex_to_text_leaves_what_it_does_not_understand() {
        // an unknown command with a brace argument: copied through whole,
        // not partly unwrapped — `\text{{{n}}}` keeps its placeholder intact.
        assert_eq!(tex_to_text("\\text{{{n}}}"), "\\text{{{n}}}");
        assert_eq!(tex_to_text("\\mathrm{kg}"), "\\mathrm{kg}");
        // an unmapped command with no argument
        assert_eq!(tex_to_text("\\nope"), "\\nope");
        // a superscript with no Unicode form falls back to literal text
        assert_eq!(tex_to_text("x^{abc}"), "x^abc");
        assert_eq!(tex_to_text("x^y"), "x^y");
        // never panics on a dangling backslash, brace or sub/sup
        assert_eq!(tex_to_text("x^"), "x^");
        assert_eq!(tex_to_text("x_"), "x_");
        assert_eq!(tex_to_text("\\"), "\\");
        assert_eq!(tex_to_text("\\frac{a}"), "\\frac{a}");
        assert_eq!(tex_to_text("{a"), "{a");
        assert_eq!(tex_to_text("a}"), "a");
    }

    #[test]
    fn tex_to_text_is_a_no_op_on_plain_text() {
        for s in ["revenue", "2026 sales", "a + b = c", ""] {
            assert_eq!(tex_to_text(s), s);
        }
    }
}
