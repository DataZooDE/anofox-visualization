//! A small, total (never-panicking) SQL lexer shared by every piece of SQL
//! surgery in the crate: statement splitting, select-list item splitting,
//! top-level keyword detection, comment stripping and `::ROLE` detection.
//!
//! It understands DuckDB/Postgres lexical structure well enough that `;`,
//! `,`, `--`, `/*`, `::` and keywords inside literals or comments are never
//! mistaken for structure:
//!
//! - `'single quoted'` strings with `''` escapes (spanning lines),
//! - `E'…'` strings with backslash escapes,
//! - `"double quoted"` identifiers with `""` escapes,
//! - `$$ … $$` and `$tag$ … $tag$` dollar-quoted strings,
//! - `-- line` comments and nested `/* block */` comments.
//!
//! Unterminated literals/comments simply run to the end of the input.

/// Token category.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokKind {
    /// Identifier or keyword (`select`, `my_col`, `É`).
    Word,
    /// Numeric literal (`1`, `2.5e3`, `.5`).
    Number,
    /// `"quoted identifier"`.
    QuotedIdent,
    /// `'string'` or `E'string'`.
    String,
    /// `$$…$$` / `$tag$…$tag$`.
    DollarString,
    /// `-- …` up to (not including) the newline.
    LineComment,
    /// `/* … */` (nesting supported).
    BlockComment,
    /// Spaces, tabs, newlines.
    Whitespace,
    /// Any other single character (`(`, `)`, `,`, `;`, `:`, `*`, …).
    Punct,
}

/// A token: its kind and byte range in the source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokKind,
    pub start: usize,
    pub end: usize,
}

impl Token {
    /// The token's source text.
    pub fn text<'a>(&self, src: &'a str) -> &'a str {
        &src[self.start..self.end]
    }
    /// Comment or whitespace (no syntactic meaning).
    pub fn is_trivia(&self) -> bool {
        matches!(
            self.kind,
            TokKind::Whitespace | TokKind::LineComment | TokKind::BlockComment
        )
    }
    /// A `Punct` token equal to `c`.
    pub fn is_punct(&self, src: &str, c: char) -> bool {
        self.kind == TokKind::Punct && self.text(src).starts_with(c)
    }
    /// A `Word` token equal (case-insensitively) to `kw`.
    pub fn is_word(&self, src: &str, kw: &str) -> bool {
        self.kind == TokKind::Word && self.text(src).eq_ignore_ascii_case(kw)
    }
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}
fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// Tokenize `src` completely. Concatenating every token's text reproduces the
/// input exactly.
pub fn tokenize(src: &str) -> Vec<Token> {
    let chars: Vec<(usize, char)> = src.char_indices().collect();
    let n = chars.len();
    let pos = |i: usize| if i < n { chars[i].0 } else { src.len() };
    let at = |i: usize| chars.get(i).map(|c| c.1);
    let mut out = Vec::new();
    let mut i = 0usize;
    // Scan a quoted run starting after the opening quote at `i`; `q` is the
    // quote char, doubled to escape; `backslash` enables `\x` escapes.
    let quoted = |mut i: usize, q: char, backslash: bool| -> usize {
        while i < n {
            let c = chars[i].1;
            if backslash && c == '\\' {
                i += 2;
                continue;
            }
            if c == q {
                if at(i + 1) == Some(q) {
                    i += 2;
                    continue;
                }
                return i + 1;
            }
            i += 1;
        }
        n
    };
    while i < n {
        let c = chars[i].1;
        let start = i;
        let kind;
        if c.is_whitespace() {
            while i < n && chars[i].1.is_whitespace() {
                i += 1;
            }
            kind = TokKind::Whitespace;
        } else if c == '-' && at(i + 1) == Some('-') {
            while i < n && chars[i].1 != '\n' {
                i += 1;
            }
            kind = TokKind::LineComment;
        } else if c == '/' && at(i + 1) == Some('*') {
            i += 2;
            let mut depth = 1;
            while i < n && depth > 0 {
                if chars[i].1 == '/' && at(i + 1) == Some('*') {
                    depth += 1;
                    i += 2;
                } else if chars[i].1 == '*' && at(i + 1) == Some('/') {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            i = i.min(n);
            kind = TokKind::BlockComment;
        } else if c == '\'' {
            i = quoted(i + 1, '\'', false);
            kind = TokKind::String;
        } else if (c == 'E' || c == 'e') && at(i + 1) == Some('\'') {
            i = quoted(i + 2, '\'', true);
            kind = TokKind::String;
        } else if c == '"' {
            i = quoted(i + 1, '"', false);
            kind = TokKind::QuotedIdent;
        } else if c == '$' && dollar_tag_end(&chars, i).is_some() {
            let tag_end = dollar_tag_end(&chars, i).unwrap_or(i + 1);
            let tag: String = chars[i..=tag_end].iter().map(|c| c.1).collect();
            let tag_len = tag_end + 1 - i;
            i = tag_end + 1;
            // Find the closing tag.
            let mut found = n;
            let mut j = i;
            while j + tag_len <= n {
                if chars[j].1 == '$' && chars[j..j + tag_len].iter().map(|c| c.1).eq(tag.chars()) {
                    found = j + tag_len;
                    break;
                }
                j += 1;
            }
            i = found;
            kind = TokKind::DollarString;
        } else if is_ident_start(c) {
            while i < n && is_ident_char(chars[i].1) {
                i += 1;
            }
            kind = TokKind::Word;
        } else if c.is_ascii_digit() || (c == '.' && at(i + 1).is_some_and(|d| d.is_ascii_digit()))
        {
            while i < n {
                let d = chars[i].1;
                if d.is_ascii_digit() || d == '.' || d == '_' {
                    i += 1;
                } else if (d == 'e' || d == 'E')
                    && at(i + 1).is_some_and(|x| x.is_ascii_digit() || x == '+' || x == '-')
                {
                    i += 2;
                } else {
                    break;
                }
            }
            i = i.min(n);
            kind = TokKind::Number;
        } else {
            i += 1;
            kind = TokKind::Punct;
        }
        out.push(Token {
            kind,
            start: pos(start),
            end: pos(i),
        });
    }
    out
}

/// If `chars[i]` opens a dollar quote (`$$` or `$tag$`), the index of the
/// closing `$` of the opening delimiter.
fn dollar_tag_end(chars: &[(usize, char)], i: usize) -> Option<usize> {
    let mut j = i + 1;
    if chars.get(j).is_some_and(|c| c.1.is_ascii_digit()) {
        return None; // `$1` is a parameter
    }
    while let Some(&(_, c)) = chars.get(j) {
        if c == '$' {
            return Some(j);
        }
        if !(c.is_alphanumeric() || c == '_') {
            return None;
        }
        j += 1;
    }
    None
}

/// Remove `--` and `/* */` comments (outside literals). Line comments are
/// dropped up to the newline (kept); block comments become one space.
pub fn strip_comments(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    for t in tokenize(sql) {
        match t.kind {
            TokKind::LineComment => {}
            TokKind::BlockComment => out.push(' '),
            _ => out.push_str(t.text(sql)),
        }
    }
    out
}

/// Split `sql` on a top-level separator character (outside literals and
/// comments, at bracket depth 0 for `,`; `;` splits at any depth).
fn split_on(sql: &str, sep: char, depth_aware: bool) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut last = 0usize;
    for t in tokenize(sql) {
        if t.kind != TokKind::Punct {
            continue;
        }
        let c = t.text(sql).chars().next().unwrap_or(' ');
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
        if c == sep && (!depth_aware || depth == 0) {
            out.push(sql[last..t.start].to_string());
            last = t.end;
        }
    }
    let tail = &sql[last..];
    if !tail.trim().is_empty() {
        out.push(tail.to_string());
    }
    out
}

/// Split a script into statements on `;` outside literals and comments.
pub fn split_statements(sql: &str) -> Vec<String> {
    split_on(sql, ';', false)
}

/// Split a select list (or any comma list) on top-level commas — outside
/// literals/comments and `()`/`[]`/`{}` nesting.
pub fn split_top_commas(s: &str) -> Vec<String> {
    split_on(s, ',', true)
}

/// Byte offset of the first `kw` keyword (a whole `Word`, case-insensitive) at
/// bracket depth 0 and outside literals/comments, at or after byte `from`.
pub fn find_top_level_keyword(s: &str, kw: &str, from: usize) -> Option<usize> {
    find_top_level_any(s, &[kw], from).map(|(p, _)| p)
}

/// Like [`find_top_level_keyword`] for several keywords: the first match and
/// which keyword it was.
pub fn find_top_level_any<'k>(s: &str, kws: &[&'k str], from: usize) -> Option<(usize, &'k str)> {
    let mut depth = 0i32;
    // The two previous significant tokens: a word right after `::` is a cast
    // type / role (`'x'::GROUP`), never a clause keyword.
    let mut prev: [Option<Token>; 2] = [None, None];
    for t in tokenize(s) {
        if t.is_trivia() {
            continue;
        }
        let after_cast = matches!(prev, [Some(a), Some(b)]
            if a.is_punct(s, ':') && b.is_punct(s, ':') && a.end == b.start);
        match t.kind {
            TokKind::Punct => match t.text(s) {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" | "}" => depth -= 1,
                _ => {}
            },
            TokKind::Word if depth == 0 && t.start >= from && !after_cast => {
                if let Some(k) = kws.iter().find(|k| t.is_word(s, k)) {
                    return Some((t.start, k));
                }
            }
            _ => {}
        }
        prev = [prev[1], Some(t)];
    }
    None
}

/// The first keyword of a statement (uppercased), skipping comments,
/// whitespace and opening parentheses — e.g. `SELECT`, `WITH`, `CREATE`.
pub fn first_keyword(sql: &str) -> Option<String> {
    tokenize(sql)
        .into_iter()
        .filter(|t| !t.is_trivia())
        .find(|t| !t.is_punct(sql, '('))
        .filter(|t| t.kind == TokKind::Word)
        .map(|t| t.text(sql).to_ascii_uppercase())
}

/// If `item` ends in a `::TOKEN` cast (a bare word; whitespace allowed around
/// `::`), return `(expression before the cast, token)`. `x::DECIMAL(10,2)`
/// or `x::INT[]` end in punctuation and return `None`.
pub fn trailing_cast(item: &str) -> Option<(&str, &str)> {
    let toks: Vec<Token> = tokenize(item)
        .into_iter()
        .filter(|t| !t.is_trivia())
        .collect();
    let k = toks.len();
    if k < 4 {
        return None; // need at least `expr : : WORD`
    }
    let (w, c2, c1) = (toks[k - 1], toks[k - 2], toks[k - 3]);
    if w.kind == TokKind::Word
        && c2.is_punct(item, ':')
        && c1.is_punct(item, ':')
        && c1.end == c2.start
    {
        Some((item[..c1.start].trim(), w.text(item)))
    } else {
        None
    }
}

/// Split a trailing top-level `AS alias` (bare or `"quoted"`) off an
/// expression: `(core, Some(alias))`. `CAST(x AS INT)` keeps its inner `AS`.
pub fn trailing_alias(expr: &str) -> (&str, Option<String>) {
    let toks: Vec<Token> = tokenize(expr)
        .into_iter()
        .filter(|t| !t.is_trivia())
        .collect();
    let k = toks.len();
    if k < 3 {
        return (expr, None);
    }
    let (alias, as_kw) = (toks[k - 1], toks[k - 2]);
    if !as_kw.is_word(expr, "AS") {
        return (expr, None);
    }
    let name = match alias.kind {
        TokKind::Word => alias.text(expr).to_string(),
        TokKind::QuotedIdent => {
            let t = alias.text(expr);
            let inner = t.strip_prefix('"').unwrap_or(t);
            let inner = inner.strip_suffix('"').unwrap_or(inner);
            inner.replace("\"\"", "\"")
        }
        _ => return (expr, None),
    };
    // `AS` must sit at bracket depth 0 (it always does when the alias is the
    // final token of a balanced item, but stay defensive).
    let mut depth = 0i32;
    for t in &toks[..k - 2] {
        if t.kind == TokKind::Punct {
            match t.text(expr) {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" | "}" => depth -= 1,
                _ => {}
            }
        }
    }
    let core = expr[..as_kw.start].trim_end();
    if depth != 0 || core.is_empty() || name.is_empty() {
        return (expr, None);
    }
    (core, Some(name))
}

/// `Some(name)` if `s` is exactly one bare or quoted identifier.
pub fn simple_ident(s: &str) -> Option<String> {
    let toks: Vec<Token> = tokenize(s).into_iter().filter(|t| !t.is_trivia()).collect();
    match toks.as_slice() {
        [t] if t.kind == TokKind::Word => Some(t.text(s).to_string()),
        [t] if t.kind == TokKind::QuotedIdent => {
            let x = t.text(s);
            Some(x.trim_matches('"').replace("\"\"", "\""))
        }
        _ => None,
    }
}

/// A select-list item that expands to several columns (`*`, `t.*`,
/// `* EXCLUDE (…)`, `COLUMNS(…)`) — passed through unaliased.
pub fn is_star_item(item: &str) -> bool {
    let toks: Vec<Token> = tokenize(item)
        .into_iter()
        .filter(|t| !t.is_trivia())
        .collect();
    match toks.first() {
        Some(t) if t.is_punct(item, '*') => true,
        Some(t) if t.is_word(item, "COLUMNS") => toks.get(1).is_some_and(|p| p.is_punct(item, '(')),
        Some(_) => {
            // qualifier.* (optionally followed by EXCLUDE/REPLACE …)
            toks.windows(2)
                .any(|w| w[0].is_punct(item, '.') && w[1].is_punct(item, '*'))
                && toks.iter().take_while(|t| !t.is_punct(item, '*')).all(|t| {
                    matches!(t.kind, TokKind::Word | TokKind::QuotedIdent) || t.is_punct(item, '.')
                })
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_never_panics() {
        for s in [
            "",
            "'",
            "\"",
            "$$",
            "$a$",
            "/*",
            "--",
            "E'\\",
            "é'ü",
            "a::",
            "::",
            "1e",
            ".5",
            "$1",
            "SELECT 'a''b' -- c\n/* d /* e */ f */ ;",
        ] {
            let toks = tokenize(s);
            let joined: String = toks.iter().map(|t| t.text(s)).collect();
            assert_eq!(joined, s);
        }
    }
}
