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

/// Keeps a `::ASOF` chip current: re-reads `data-asof` against the viewer's
/// clock and writes the time in the viewer's zone and the page language
/// (same wording as `web/app.js`'s chip); amber past `data-stale` seconds.
pub const ASOF_SCRIPT: &str = r#"(function(){
  var c=document.querySelector('.dp-asof[data-asof]'); if(!c) return;
  var at=+c.getAttribute('data-asof'), st=+c.getAttribute('data-stale')||0, lang=document.documentElement.lang||undefined;
  function rel(s){s=Math.max(0,s);return s<60?'just now':s<3600?Math.floor(s/60)+' min ago':s<172800?Math.floor(s/3600)+' h ago':Math.floor(s/86400)+' d ago';}
  function tick(){var d=new Date(at),now=Date.now(),age=(now-at)/1000,t;
    try{t=d.toLocaleTimeString(lang,{hour:'2-digit',minute:'2-digit'});if(d.toDateString()!==new Date(now).toDateString())t=d.toLocaleDateString(lang)+' '+t;}catch(e){t=d.toISOString().slice(0,16).replace('T',' ');}
    c.textContent='Data as of '+t+' ('+rel(age)+')'; c.title=d.toString(); c.classList.toggle('stale',st>0&&age>st);}
  tick(); setInterval(tick,60000);
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
    // Without an author title the page still names itself.
    let b = &t.brand;
    let header = if b.title.is_none() && b.name.is_none() && b.logo.is_none() {
        format!(
            "<div class=\"dp-head\"><div class=\"dp-head-text\"><h1 class=\"dp-title\">{}</h1></div>{}</div>",
            esc(p.fallback_title),
            theme::asof_html(t)
        )
    } else {
        theme::header_html(t)
    };
    format!(
        "<!doctype html><html lang=\"{lang}\" data-theme=\"{name}\" data-variant=\"{variant}\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">{head_extra}{favicon}<title>{title}</title>\
<style>{css}</style></head>\
<body class=\"dp-static\"><div class=\"grid\">{header}{sub}{controls}{panels}{credit}</div>\
<div id=\"dp-tip\" class=\"dp-tip\"></div><script>{HOVER_SCRIPT}{ASOF_SCRIPT}</script></body></html>",
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

/// The searchable list's script: hides the entries (and then the groups)
/// whose text doesn't hold every word typed.
const LIST_SEARCH_SCRIPT: &str = r#"(function(){
  var q=document.getElementById('dash-q'); if(!q) return;
  var fold=function(s){return s.normalize('NFKD').replace(/[\u0300-\u036f]/g,'').toLowerCase();};
  q.addEventListener('input',function(){
    var w=fold(q.value).split(/\s+/).filter(Boolean), shown=0;
    [].forEach.call(document.querySelectorAll('.dash-list li'),function(li){
      var t=fold(li.getAttribute('data-q')||''), ok=w.every(function(x){return t.indexOf(x)>=0;});
      li.hidden=!ok; if(ok) shown++;});
    [].forEach.call(document.querySelectorAll('.dash-group'),function(g){g.hidden=!g.querySelector('li:not([hidden])');});
    document.getElementById('dash-none').hidden=shown>0;
  });
})();"#;

/// Above this many dashboards the list (and the served nav) gets a search
/// field.
pub const LIST_SEARCH_MIN: usize = 9;

/// The `/` list of a served folder: the dashboards grouped by `-- @group`
/// (ungrouped first, then the groups by name; `A / B` is group A,
/// sub-heading B), each with its owner, a "✓ certified" badge and its
/// tags; a search field above more than 8. `items`: (link, header).
pub fn list_dashboards(
    t: &ResolvedTheme,
    title: &str,
    items: &[(String, &crate::host::serving::DashboardMeta)],
) -> String {
    use std::collections::BTreeMap;
    // group → sub-heading → entries; "" = none
    let mut groups: BTreeMap<(bool, String), BTreeMap<String, Vec<String>>> = BTreeMap::new();
    for (href, m) in items {
        let (g, sub) = match m.group.as_deref() {
            Some(g) => match g.split_once(" / ") {
                Some((a, b)) => (a.to_string(), b.to_string()),
                None => (g.to_string(), String::new()),
            },
            None => (String::new(), String::new()),
        };
        let cert = m
            .certified
            .as_deref()
            .map(|d| {
                format!(
                    " <span class=\"dash-cert\" title=\"certified on {d}\">✓ certified</span>",
                    d = esc(d)
                )
            })
            .unwrap_or_default();
        let owner = m
            .owner
            .as_deref()
            .map(|o| format!("<span class=\"dash-owner\">{}</span>", esc(o)))
            .unwrap_or_default();
        let tags: String = m
            .tags
            .iter()
            .map(|t| format!("<span class=\"dash-tag\">{}</span>", esc(t)))
            .collect();
        let meta = if owner.is_empty() && tags.is_empty() {
            String::new()
        } else {
            format!("<span class=\"dash-meta\">{owner}{tags}</span>")
        };
        let q = [
            m.title.as_str(),
            m.group.as_deref().unwrap_or(""),
            m.owner.as_deref().unwrap_or(""),
            &m.tags.join(" "),
            if m.certified.is_some() {
                "certified"
            } else {
                ""
            },
        ]
        .iter()
        .filter(|p| !p.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
        groups
            .entry((!g.is_empty(), g.to_lowercase() + "\u{0}" + &g))
            .or_default()
            .entry(sub)
            .or_default()
            .push(format!(
                "<li data-q=\"{q}\"><a href=\"{href}\">{label}</a>{cert}{meta}</li>",
                q = esc(&q),
                href = esc(href),
                label = esc(&m.title),
            ));
    }
    let mut body = String::new();
    for ((grouped, key), subs) in groups {
        let name = key.split_once('\u{0}').map(|(_, n)| n).unwrap_or("");
        body.push_str("<section class=\"dash-group\">");
        if grouped {
            body.push_str(&format!("<h2 class=\"dash-group-h\">{}</h2>", esc(name)));
        }
        for (sub, lis) in subs {
            if !sub.is_empty() {
                body.push_str(&format!("<h3 class=\"dash-sub-h\">{}</h3>", esc(&sub)));
            }
            body.push_str(&format!("<ul class=\"dash-list\">{}</ul>", lis.concat()));
        }
        body.push_str("</section>");
    }
    // a search field above more than 8; its script after the list it filters
    let (input, script) = if items.len() >= LIST_SEARCH_MIN {
        (
            "<input id=\"dash-q\" class=\"dash-q\" type=\"search\" placeholder=\"Search dashboards…\" \
             aria-label=\"Search dashboards\" autocomplete=\"off\">"
                .to_string(),
            format!("<script>{LIST_SEARCH_SCRIPT}</script>"),
        )
    } else {
        (String::new(), String::new())
    };
    let none = "<p id=\"dash-none\" class=\"dash-none\" hidden>No dashboard matches.</p>";
    render(&StaticPage {
        theme: t,
        fallback_title: title,
        subtitle_html: String::new(),
        head_extra: String::new(),
        controls_html: String::new(),
        panels_html: format!("<div class=\"dash-index\">{input}{body}{none}</div>{script}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::serving::parse_dashboard_meta;

    #[test]
    fn grouped_list() {
        let t = theme::resolve("").unwrap().default_variant().clone();
        let metas = [
            parse_dashboard_meta("z", "-- @title Zeta\nSELECT 1;"),
            parse_dashboard_meta(
                "s",
                "-- @title Weekly <sales>\n-- @group Sales / EMEA\n-- @tags kpi\n-- @owner Fin & Co\n-- @certified 2026-10-15\nSELECT 1;",
            ),
            parse_dashboard_meta("o", "-- @title Ops board\n-- @group ops\nSELECT 1;"),
            parse_dashboard_meta("m", "-- @title Margin\n-- @group Sales\nSELECT 1;"),
        ];
        let items: Vec<(String, &crate::host::serving::DashboardMeta)> = metas
            .iter()
            .map(|m| (format!("/d/{}", m.title.len()), m))
            .collect();
        let html = list_dashboards(&t, "Dashboards", &items);
        let at = |s: &str| html.find(s).unwrap_or_else(|| panic!("{s} missing"));
        // ungrouped first, then the groups by name (case-insensitive), a
        // sub-heading for `A / B`
        assert!(at(">Zeta<") < at(">ops</h2>") && at(">ops</h2>") < at(">Sales</h2>"));
        assert!(at(">Sales</h2>") < at(">Margin<") && at(">Margin<") < at(">EMEA</h3>"));
        assert!(at(">EMEA</h3>") < at("Weekly &lt;sales&gt;"));
        assert!(html.contains("✓ certified") && html.contains("certified on 2026-10-15"));
        assert!(html.contains("<span class=\"dash-owner\">Fin &amp; Co</span>"));
        assert!(html.contains("<span class=\"dash-tag\">kpi</span>"));
        assert!(
            !html.contains("id=\"dash-q\""),
            "no search for 4 dashboards"
        );
        let many: Vec<_> = (0..LIST_SEARCH_MIN).map(|_| items[0].clone()).collect();
        let html = list_dashboards(&t, "Dashboards", &many);
        assert!(html.contains("id=\"dash-q\"") && html.contains("dash-none"));
    }
}
