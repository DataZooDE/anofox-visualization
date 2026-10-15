//! Static dashboard pages — the CLI's `dashboard file.sql out.html` and
//! `serve --dashboards`. Both share one page shell: the theme's CSS tokens +
//! `web/theme-base.css` (the browser's dashboard stylesheet), the dashboard
//! header (title / brand / logo), the panels, the mandatory DataZoo credit
//! and a small hover script.

use crate::format::escape_xml as esc;
use crate::theme::{self, ResolvedTheme};

/// Hover tooltips + click-to-focus a series, wired from the `<title>` each SVG
/// mark carries. Static (no wasm).
pub const HOVER_SCRIPT: &str = r#"(function(){
  var tip=document.getElementById('dp-tip'), selected=null;
  var marks=[].slice.call(document.querySelectorAll('.panel svg rect,.panel svg circle,.panel svg polygon,.panel svg polyline'))
    .filter(function(el){var t=el.querySelector('title');return t&&t.textContent.trim();});
  function apply(){marks.forEach(function(el){var s=el.getAttribute('data-series');el.style.opacity=(!selected||s===selected)?'':'0.15';});}
  marks.forEach(function(el){
    var t=el.querySelector('title'), txt=t.textContent;
    var series=txt.indexOf(': ')>=0?txt.slice(0,txt.lastIndexOf(': ')):txt;
    el.removeChild(t); el.setAttribute('data-series',series); el.classList.add('dp-hit'); el.style.cursor='pointer';
    el.addEventListener('mouseenter',function(){tip.textContent=txt;tip.classList.add('show');});
    el.addEventListener('mousemove',function(e){tip.style.left=(e.clientX+14)+'px';tip.style.top=(e.clientY+14)+'px';});
    el.addEventListener('mouseleave',function(){tip.classList.remove('show');});
    el.addEventListener('click',function(e){e.stopPropagation();selected=(selected===series)?null:series;apply();});
  });
  document.addEventListener('click',function(){if(selected!==null){selected=null;apply();}});
})();"#;

/// What goes on a static page.
pub struct StaticPage<'a> {
    pub theme: &'a ResolvedTheme,
    /// `<title>` when the dashboard sets no `::DASHBOARD_TITLE`.
    pub fallback_title: &'a str,
    /// A line under the header (e.g. "rendered from x.sql · 4 panels"); HTML.
    pub subtitle_html: String,
    /// Extra `<head>` markup (e.g. a meta refresh).
    pub head_extra: String,
    /// Controls (served parameter form), HTML.
    pub controls_html: String,
    /// The panels (`<figure class="panel">…` / `<h2 class="section">…`), HTML.
    pub panels_html: String,
}

/// Render a static page.
pub fn render(p: &StaticPage) -> String {
    let t = p.theme;
    let title = theme::page_title(t, p.fallback_title);
    let header = theme::header_html(t);
    // Without an author title the page still names itself.
    let header = if header.is_empty() {
        format!(
            "<header class=\"dp-head\"><div class=\"dp-head-text\"><h1 class=\"dp-title\">{}</h1></div></header>",
            esc(p.fallback_title)
        )
    } else {
        header
    };
    format!(
        "<!doctype html><html lang=\"{lang}\" data-theme=\"{name}\" data-variant=\"{variant}\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">{head_extra}{favicon}<title>{title}</title>\
<style>{css}</style></head>\
<body class=\"dp-static\"><div class=\"grid\">{header}{sub}{controls}{panels}{credit}</div>\
<div id=\"dp-tip\" class=\"dp-tip\"></div><script>{HOVER_SCRIPT}</script></body></html>",
        lang = esc(t.locale.as_deref().unwrap_or("en")),
        name = esc(&t.name),
        variant = t.variant.as_str(),
        head_extra = p.head_extra,
        favicon = theme::favicon_html(t),
        title = esc(&title),
        css = theme::page_css(t),
        sub = if p.subtitle_html.is_empty() {
            String::new()
        } else {
            format!("<div class=\"dp-src\">{}</div>", p.subtitle_html)
        },
        controls = p.controls_html,
        panels = p.panels_html,
        credit = theme::credit_html(),
    )
}

/// A simple themed list page (the `serve --dashboards` index).
pub fn list(t: &ResolvedTheme, title: &str, items: &[(String, String)]) -> String {
    let lis: String = items
        .iter()
        .map(|(href, label)| format!("<li><a href=\"{}\">{}</a></li>", esc(href), esc(label)))
        .collect();
    render(&StaticPage {
        theme: t,
        fallback_title: title,
        subtitle_html: String::new(),
        head_extra: String::new(),
        controls_html: String::new(),
        panels_html: format!("<ul class=\"dash-list\">{lis}</ul>"),
    })
}
