//! `::MARKDOWN` text as HTML for the static export (`dashboard file.sql
//! out.html`, #1345): headings, lists, fenced code, blockquotes, rules, GFM
//! tables, links, images and `$…$` formulas, by the browser's own rules —
//! a port of `renderMarkdown` / `markdownWithValues` in `web/app.js`.
//!
//! Raw HTML in the source is escaped, a link keeps only a safe URL, and a
//! `{{col}}` value is filled in after the Markdown is rendered, so a value
//! never becomes markup. Both sides must give the same HTML for every case
//! in `tests/fixtures/markdown_html_vectors.json`
//! (`tests/web/markdown_html_test.mjs` is the browser half of that guard).

use crate::format::escape_xml as esc;

/// How [`render`] draws what the browser fetches or typesets.
pub struct Opts<'a> {
    /// A single newline inside a paragraph is a line break (an `::INFO`
    /// text), not a space.
    pub breaks: bool,
    /// A formula as readable text ([`crate::math::tex_to_text`]) — the
    /// static page has no KaTeX — instead of its TeX source, which the
    /// browser shows until KaTeX replaces it.
    pub tex_text: bool,
    /// An image `src` (raw, as written) → the URL to draw it from, or
    /// `None` for its alt text.
    pub image: &'a mut dyn FnMut(&str) -> Option<String>,
}

/// Markdown → HTML (`renderMarkdown` in `web/app.js`).
pub fn render(src: &str, o: &mut Opts) -> String {
    let src = src.replace('\r', "");
    let lines: Vec<&str> = src.split('\n').collect();
    let mut html = String::new();
    let mut list: Option<&str> = None;
    let close_list = |html: &mut String, list: &mut Option<&str>| {
        if let Some(l) = list.take() {
            html.push_str(&format!("</{l}>"));
        }
    };
    let table_at = |k: usize| {
        k + 1 < lines.len()
            && lines[k].contains('|')
            && lines[k + 1].contains('|')
            && table_sep(lines[k + 1])
    };
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if table_at(i) {
            close_list(&mut html, &mut list);
            let head = cells(line);
            let align: Vec<&str> = cells(lines[i + 1])
                .iter()
                .map(|c| match (c.starts_with(':'), c.ends_with(':')) {
                    (true, true) => "center",
                    (_, true) => "right",
                    (true, _) => "left",
                    _ => "",
                })
                .collect();
            let mut cell = |tag: &str, c: &str, k: usize| {
                let style = match align.get(k) {
                    Some(a) if !a.is_empty() => format!(" style=\"text-align:{a}\""),
                    _ => String::new(),
                };
                format!("<{tag}{style}>{}</{tag}>", inline(c, o))
            };
            html.push_str("<table class=\"md-table\"><thead><tr>");
            for (k, c) in head.iter().enumerate() {
                html.push_str(&cell("th", c, k));
            }
            html.push_str("</tr></thead><tbody>");
            i += 2;
            while i < lines.len() && lines[i].contains('|') && !blank(lines[i]) {
                let row = cells(lines[i]);
                i += 1;
                html.push_str("<tr>");
                for k in 0..head.len() {
                    html.push_str(&cell("td", row.get(k).map_or("", String::as_str), k));
                }
                html.push_str("</tr>");
            }
            html.push_str("</tbody></table>");
            continue;
        }
        if line.starts_with("```") {
            close_list(&mut html, &mut list);
            i += 1;
            let mut code = String::new();
            while i < lines.len() && !lines[i].starts_with("```") {
                code.push_str(&esc(lines[i]));
                code.push('\n');
                i += 1;
            }
            i += 1;
            html.push_str(&format!("<pre><code>{code}</code></pre>"));
            continue;
        }
        if let Some((n, text)) = heading(line) {
            close_list(&mut html, &mut list);
            html.push_str(&format!("<h{n}>{}</h{n}>", inline(text, o)));
            i += 1;
            continue;
        }
        if rule(line) {
            close_list(&mut html, &mut list);
            html.push_str("<hr>");
            i += 1;
            continue;
        }
        if let Some(rest) = line.strip_prefix('>') {
            close_list(&mut html, &mut list);
            let rest = rest.strip_prefix(char::is_whitespace).unwrap_or(rest);
            html.push_str(&format!("<blockquote>{}</blockquote>", inline(rest, o)));
            i += 1;
            continue;
        }
        for (tag, item) in [("ul", bullet(line)), ("ol", numbered(line))] {
            if let Some(text) = item {
                if list != Some(tag) {
                    close_list(&mut html, &mut list);
                    html.push_str(&format!("<{tag}>"));
                    list = Some(tag);
                }
                html.push_str(&format!("<li>{}</li>", inline(text, o)));
                break;
            }
        }
        if bullet(line).is_some() || numbered(line).is_some() {
            i += 1;
            continue;
        }
        if blank(line) {
            close_list(&mut html, &mut list);
            i += 1;
            continue;
        }
        close_list(&mut html, &mut list);
        let mut para = line.to_string();
        i += 1;
        let sep = if o.breaks { "\n" } else { " " };
        while i < lines.len() && !blank(lines[i]) && !special(lines[i]) && !table_at(i) {
            para.push_str(sep);
            para.push_str(lines[i]);
            i += 1;
        }
        let body = inline(&para, o);
        let body = if o.breaks {
            body.replace('\n', "<br>")
        } else {
            body
        };
        html.push_str(&format!("<p>{body}</p>"));
    }
    close_list(&mut html, &mut list);
    html
}

/// `{{col}}` / `{{col:fmt}}` filled from the statement's own row, then
/// [`render`] (`markdownWithValues` in `web/app.js`): the Markdown is
/// rendered with inert private-use tokens in the values' place; a token in
/// text becomes the escaped value, one in a formula's TeX (`data-tex`) TeX
/// text, any other inside a tag is dropped. Unknown columns stay as written.
pub fn render_with_values(
    src: &str,
    rows: &[(String, Vec<crate::Value>)],
    l: &crate::format::Locale,
    o: &mut Opts,
) -> String {
    let mut vals: Vec<String> = Vec::new();
    let md = crate::dashboard::markdown_fill_with(src, rows, l, &mut |v| {
        vals.push(v);
        format!("\u{E000}{}\u{E001}", vals.len() - 1)
    });
    let html = render(&md, o);
    if vals.is_empty() {
        return html;
    }
    let val = |k: &str| {
        k.parse::<usize>()
            .ok()
            .and_then(|k| vals.get(k))
            .map_or("", String::as_str)
    };
    parts(&html, |part| {
        if part.starts_with('<') {
            let part = match part.find(" data-tex=\"") {
                Some(a) => {
                    let a = a + " data-tex=\"".len();
                    let b = part[a..].find('"').map_or(part.len(), |b| a + b);
                    let tex = tokens(&part[a..b], |k| esc(&tex_text(val(k))));
                    format!("{}{tex}{}", &part[..a], &part[b..])
                }
                None => part.to_string(),
            };
            tokens(&part, |_| String::new())
        } else {
            tokens(part, |k| esc(val(k)))
        }
    })
}

/// A value as TeX text: `\text{…}` with TeX's special characters escaped
/// (`texText` in `web/app.js`).
fn tex_text(v: &str) -> String {
    let mut out = String::from("\\text{");
    for c in v.chars() {
        match c {
            '\\' => out.push_str("\\textbackslash{}"),
            '^' => out.push_str("\\textasciicircum{}"),
            '~' => out.push_str("\\textasciitilde{}"),
            '{' | '}' | '$' | '%' | '&' | '#' | '_' => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push('}');
    out
}

/// `html` split into tags (`<…>`) and the text between them, each mapped
/// by `f` (the browser's `/<[^>]*>|[^<]+/g` replace).
fn parts(html: &str, mut f: impl FnMut(&str) -> String) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while !rest.is_empty() {
        let end = if rest.starts_with('<') {
            match rest.find('>') {
                Some(e) => e + 1,
                None => {
                    out.push_str(rest);
                    break;
                }
            }
        } else {
            rest.find('<').unwrap_or(rest.len())
        };
        out.push_str(&f(&rest[..end]));
        rest = &rest[end..];
    }
    out
}

/// Every `{{…}}` value token (`\u{E000}k\u{E001}`) in `s` replaced by `f(k)`.
fn tokens(s: &str, mut f: impl FnMut(&str) -> String) -> String {
    token_pass(s, '\u{E000}', '\u{E001}', &mut f)
}

/// Every `open` digits `close` run in `s` replaced by `f(digits)`.
fn token_pass(s: &str, open: char, close: char, f: &mut dyn FnMut(&str) -> String) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(a) = rest.find(open) {
        out.push_str(&rest[..a]);
        let after = &rest[a + open.len_utf8()..];
        let digits = after.len() - after.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if digits > 0 && after[digits..].starts_with(close) {
            out.push_str(&f(&after[..digits]));
            rest = &after[digits + close.len_utf8()..];
        } else {
            out.push(open);
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// One line's inline Markdown: formulas first (as inert tokens), then the
/// inline rules, then the formulas and images back in, outside tags only.
fn inline(s: &str, o: &mut Opts) -> String {
    // these private-use characters are this function's own token markers,
    // never input: a typed one could forge an image or formula
    let s: String = s
        .chars()
        .filter(|c| !('\u{E002}'..='\u{E005}').contains(c))
        .collect();
    let mut math: Vec<(String, bool)> = Vec::new();
    let mut md = String::new();
    for seg in crate::math::spans(&s) {
        match seg {
            crate::math::Seg::Text(t) => md.push_str(&t),
            crate::math::Seg::Math { tex, display } => {
                math.push((tex, display));
                md.push_str(&format!("\u{E002}{}\u{E003}", math.len() - 1));
            }
        }
    }
    let mut imgs: Vec<String> = Vec::new();
    let html = inline_md(&md, &mut imgs, &math, o);
    if math.is_empty() && imgs.is_empty() {
        return html;
    }
    let idx = |k: &str| k.parse::<usize>().ok();
    let span = |(tex, display): &(String, bool)| {
        let class = if *display {
            "md-math md-math-display"
        } else {
            "md-math"
        };
        let shown = if o.tex_text {
            esc(&crate::math::tex_to_text(tex))
        } else {
            format!("<code class=\"md-tex\">{}</code>", esc(tex))
        };
        format!(
            "<span class=\"{class}\" data-tex=\"{}\">{shown}</span>",
            esc(tex)
        )
    };
    parts(&html, |part| {
        let tag = part.starts_with('<');
        let p = token_pass(part, '\u{E002}', '\u{E003}', &mut |k| match idx(k)
            .and_then(|k| math.get(k))
        {
            Some(m) if !tag => span(m),
            _ => String::new(),
        });
        token_pass(
            &p,
            '\u{E004}',
            '\u{E005}',
            &mut |k| match idx(k).and_then(|k| imgs.get(k)) {
                Some(img) if !tag => img.clone(),
                _ => String::new(),
            },
        )
    })
}

/// HTML entities `esc` wrote, undone (to check a URL as written).
fn unesc(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// The inline rules, in the browser's order: images (tokenized, before
/// links — `![x [a](b)](c)` must never make a live link inside an alt),
/// `code`, **bold**, __bold__, *italic*, [links](url).
fn inline_md(s: &str, imgs: &mut Vec<String>, math: &[(String, bool)], o: &mut Opts) -> String {
    let s = esc(s);
    // ![alt](src): alt without `]`, src without `)` or whitespace
    let s = scan(&s, |r| {
        let alt_raw = r.strip_prefix("![")?;
        let a = alt_raw.find(']')?;
        let src = alt_raw[a + 1..].strip_prefix('(')?;
        let b = src.find(|c: char| c == ')' || c.is_whitespace())?;
        if b == 0 || !src[b..].starts_with(')') {
            return None;
        }
        let alt = token_pass(&alt_raw[..a], '\u{E002}', '\u{E003}', &mut |k| {
            k.parse::<usize>()
                .ok()
                .and_then(|k| math.get(k))
                .map_or(String::new(), |m| esc(&m.0))
        });
        let len = 2 + a + 2 + b + 1;
        let out = match (o.image)(&unesc(&src[..b])) {
            Some(url) => {
                imgs.push(format!(
                    "<img class=\"md-img\" src=\"{}\" alt=\"{alt}\" loading=\"lazy\" decoding=\"async\">",
                    esc(&url)
                ));
                format!("\u{E004}{}\u{E005}", imgs.len() - 1)
            }
            None => alt,
        };
        Some((len, out))
    });
    let s = scan(&s, |r| {
        let body = r.strip_prefix('`')?;
        let e = body.find('`')?;
        (e > 0).then(|| (e + 2, format!("<code>{}</code>", &body[..e])))
    });
    let s = scan(&s, |r| pair(r, "**", '*', "strong"));
    let s = scan(&s, |r| pair(r, "__", '_', "strong"));
    let s = italic(&s);
    scan(&s, |r| {
        let t = r.strip_prefix('[')?;
        let a = t.find(']')?;
        let u = t[a + 1..].strip_prefix('(')?;
        let b = u.find(')')?;
        if a == 0 || b == 0 {
            return None;
        }
        let (text, url) = (&t[..a], &u[..b]);
        let out = match safe_url(&unesc(url)) {
            Some(href) => format!(
                "<a href=\"{}\" target=\"_blank\" rel=\"noopener\">{text}</a>",
                esc(&href)
            ),
            None => text.to_string(),
        };
        Some((1 + a + 2 + b + 1, out))
    })
}

/// A global regex replace by hand: at each position, `m` either matches
/// (its length in bytes and the replacement) or the scan moves on one
/// character.
fn scan(s: &str, mut m: impl FnMut(&str) -> Option<(usize, String)>) -> String {
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        match m(&s[i..]) {
            Some((len, rep)) => {
                out.push_str(&rep);
                i += len;
            }
            None => {
                let c = s[i..].chars().next().unwrap_or(' ');
                out.push(c);
                i += c.len_utf8();
            }
        }
    }
    out
}

/// `**x**` / `__x__`: a non-empty run without `mark` between two `delim`s.
fn pair(r: &str, delim: &str, mark: char, tag: &str) -> Option<(usize, String)> {
    let body = r.strip_prefix(delim)?;
    let e = body.find(mark)?;
    (e > 0 && body[e..].starts_with(delim)).then(|| {
        (
            2 * delim.len() + e,
            format!("<{tag}>{}</{tag}>", &body[..e]),
        )
    })
}

/// `*x*` not touching another `*` before it (`/(^|[^*])\*([^*]+)\*/g`).
fn italic(s: &str) -> String {
    let em = |r: &str| -> Option<(usize, String)> {
        let body = r.strip_prefix('*')?;
        let e = body.find('*')?;
        (e > 0).then(|| (e + 2, format!("<em>{}</em>", &body[..e])))
    };
    let mut first = true;
    scan(s, |r| {
        let at_start = std::mem::replace(&mut first, false);
        if at_start {
            if let Some(m) = em(r) {
                return Some(m);
            }
        }
        let c = r.chars().next()?;
        if c == '*' {
            return None;
        }
        let (len, rep) = em(&r[c.len_utf8()..])?;
        Some((c.len_utf8() + len, format!("{c}{rep}")))
    })
}

/// A URL safe for an `href`: http(s), mailto or relative (no scheme); a
/// protocol-relative one becomes https (`safeUrl` in `web/app.js`).
fn safe_url(u: &str) -> Option<String> {
    let s = u.trim();
    // a browser ignores whitespace and control characters inside a scheme
    let probe: String = s
        .chars()
        .filter(|c| *c > '\u{20}')
        .collect::<String>()
        .to_lowercase();
    let scheme = probe.find(':').map(|e| &probe[..e]).filter(|sc| {
        sc.starts_with(|c: char| c.is_ascii_lowercase())
            && sc
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "+.-".contains(c))
    });
    match scheme {
        None if s.starts_with("//") => Some(format!("https:{s}")),
        None => Some(s.to_string()),
        Some("http" | "https" | "mailto") => Some(s.to_string()),
        Some(_) => None,
    }
}

fn blank(l: &str) -> bool {
    l.trim().is_empty()
}

/// `# text` … `###### text` → (level, text).
fn heading(l: &str) -> Option<(usize, &str)> {
    let n = l.len() - l.trim_start_matches('#').len();
    let rest = &l[n..];
    ((1..=6).contains(&n) && rest.starts_with(char::is_whitespace)).then(|| (n, rest.trim_start()))
}

/// A rule: three or more of one of `-` `*` `_`, spaces allowed between.
fn rule(l: &str) -> bool {
    let t = l.trim();
    let Some(c) = t.chars().next().filter(|c| "-*_".contains(*c)) else {
        return false;
    };
    t.chars().all(|x| x == c || x.is_whitespace()) && t.chars().filter(|x| *x == c).count() >= 3
}

/// `- item` / `* item` / `+ item` → the item's text.
fn bullet(l: &str) -> Option<&str> {
    let t = l.trim_start();
    let rest = t.strip_prefix(['-', '*', '+'])?;
    rest.starts_with(char::is_whitespace)
        .then(|| rest.trim_start())
}

/// `1. item` → the item's text.
fn numbered(l: &str) -> Option<&str> {
    let t = l.trim_start();
    let d = t.len() - t.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    let rest = t[d..].strip_prefix('.')?;
    (d > 0 && rest.starts_with(char::is_whitespace)).then(|| rest.trim_start())
}

/// Does `l` start a block other than a paragraph line (it ends the
/// paragraph before it)?
fn special(l: &str) -> bool {
    let n = l.len() - l.trim_start_matches('#').len();
    let t = l.trim_start();
    let marker = |r: Option<&str>| r.is_some_and(|r| r.starts_with(char::is_whitespace));
    ((1..=6).contains(&n) && l[n..].starts_with(char::is_whitespace))
        || l.starts_with("```")
        || l.starts_with('>')
        || marker(t.strip_prefix(['-', '*', '+']))
        || {
            let d = t.len() - t.trim_start_matches(|c: char| c.is_ascii_digit()).len();
            d > 0 && marker(t[d..].strip_prefix('.'))
        }
}

/// A GFM table's separator row: `|---|:--:|--:|`.
fn table_sep(l: &str) -> bool {
    let t = l.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    t.split('|').all(|c| {
        let c = c.trim();
        let c = c.strip_prefix(':').unwrap_or(c);
        let c = c.strip_suffix(':').unwrap_or(c);
        !c.is_empty() && c.chars().all(|x| x == '-')
    })
}

/// A table row's cells: outer pipes dropped, split at each pipe not
/// escaped as `\|`, which becomes a plain pipe.
fn cells(l: &str) -> Vec<String> {
    let t = l.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = match t.strip_suffix('|') {
        Some(s) if !s.ends_with('\\') => s,
        _ => t,
    };
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev = '\0';
    for c in t.chars() {
        if c == '|' && prev != '\\' {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
        prev = c;
    }
    out.push(cur);
    out.iter().map(|c| c.trim().replace("\\|", "|")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The browser's own verdict for an image `src` in the plain builder:
    /// any source `imageSrc()` accepts, as written.
    fn plain(md: &str) -> String {
        let mut img = |s: &str| {
            (!matches!(crate::image::src_ok(s), crate::image::SrcKind::Bad(_)))
                .then(|| s.to_string())
        };
        render(
            md,
            &mut Opts {
                breaks: false,
                tex_text: false,
                image: &mut img,
            },
        )
    }

    #[test]
    fn matches_the_browser_on_the_shared_vectors() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/markdown_html_vectors.json"
        );
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let vectors = v["vectors"].as_array().unwrap();
        assert!(vectors.len() > 20);
        for e in vectors {
            let (md, want) = (e["md"].as_str().unwrap(), e["html"].as_str().unwrap());
            assert_eq!(plain(md), want, "markdown: {md:?}");
        }
    }

    #[test]
    fn values_never_become_markup() {
        let rows = vec![(
            "md:name".to_string(),
            vec![crate::Value::Str("<b>x</b> $y$".into())],
        )];
        let mut img = |_: &str| None;
        let mut o = Opts {
            breaks: false,
            tex_text: false,
            image: &mut img,
        };
        let h = render_with_values(
            "Hi **{{name}}** [l]({{name}})",
            &rows,
            &Default::default(),
            &mut o,
        );
        assert_eq!(h, "<p>Hi <strong>&lt;b&gt;x&lt;/b&gt; $y$</strong> <a href=\"\" target=\"_blank\" rel=\"noopener\">l</a></p>");
    }

    #[test]
    fn a_formula_reads_as_text_in_the_static_page() {
        let mut img = |_: &str| None;
        let mut o = Opts {
            breaks: false,
            tex_text: true,
            image: &mut img,
        };
        let h = render("Error $\\alpha^2$", &mut o);
        assert!(
            h.starts_with("<p>Error <span class=\"md-math\" data-tex=\"\\alpha^2\">"),
            "{h}"
        );
        assert!(!h.contains("md-tex"), "{h}");
    }
}
