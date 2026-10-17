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

/// The landing page's filters (search, tag chips, "certified only"): hides
/// the cards that don't match, then the empty groups; updates the counts.
const LANDING_SCRIPT: &str = r#"(function(){
  var q=document.getElementById('lp-q'); if(!q) return;
  var cert=document.getElementById('lp-cert'), live=document.getElementById('lp-status'),
      none=document.getElementById('lp-none'), cards=[].slice.call(document.querySelectorAll('.lp-card')),
      chips=[].slice.call(document.querySelectorAll('.lp-chip[data-tag]')), clear=document.getElementById('lp-clear');
  var fold=function(s){return s.normalize('NFKD').replace(/[̀-ͯ]/g,'').toLowerCase();};
  var pressed=function(b){return b.getAttribute('aria-pressed')==='true';};
  function apply(){
    var w=fold(q.value).split(/\s+/).filter(Boolean), tags=chips.filter(pressed).map(function(b){return b.getAttribute('data-tag');}),
        onlyCert=cert&&pressed(cert), shown=0;
    cards.forEach(function(c){
      var t=fold(c.getAttribute('data-q')||''), ct=(c.getAttribute('data-tags')||'').split('|');
      var ok=w.every(function(x){return t.indexOf(x)>=0;})&&tags.every(function(x){return ct.indexOf(x)>=0;})&&(!onlyCert||c.hasAttribute('data-cert'));
      c.hidden=!ok; if(ok) shown++;});
    [].forEach.call(document.querySelectorAll('.lp-group'),function(g){
      var n=g.querySelectorAll('.lp-card:not([hidden])').length, c=g.querySelector('.lp-n');
      g.hidden=!n; if(c) c.textContent=n;});
    [].forEach.call(document.querySelectorAll('.lp-sub'),function(s){s.hidden=!s.querySelector('.lp-card:not([hidden])');});
    var active=w.length||tags.length||onlyCert;
    none.hidden=shown>0; clear.hidden=!active;
    live.textContent=active?(shown+' of '+cards.length+' dashboards'):'';
  }
  q.addEventListener('input',apply);
  q.addEventListener('keydown',function(e){if(e.key==='Escape'&&q.value){q.value='';apply();}});
  chips.concat(cert?[cert]:[]).forEach(function(b){b.addEventListener('click',function(){b.setAttribute('aria-pressed',String(!pressed(b)));apply();});});
  clear.addEventListener('click',function(){q.value='';chips.concat(cert?[cert]:[]).forEach(function(b){b.setAttribute('aria-pressed','false');});apply();q.focus();});
})();"#;

/// From this many dashboards on, the landing page gets a search field, tag
/// chips and "certified only" (and the served nav becomes a menu).
pub const LIST_SEARCH_MIN: usize = 9;

/// One dashboard on the landing page.
pub struct LandingItem<'a> {
    /// Its link (`/d/<id>`).
    pub href: String,
    /// Its header comments.
    pub meta: &'a crate::host::serving::DashboardMeta,
    /// When its file last changed (`YYYY-MM-DD`).
    pub updated: Option<String>,
}

/// The `/` page of a served folder (the locked server, `serve --dashboards`).
pub struct Landing<'a> {
    /// The server's theme (light and dark; `auto` follows the viewer's
    /// `prefers-color-scheme`).
    pub theme: &'a theme::ThemeSet,
    /// The installation's name, tagline, logo and favicon (the app-bar
    /// settings), if set.
    pub app: Option<&'a crate::host::app_brand::AppBrand>,
    pub items: Vec<LandingItem<'a>>,
}

/// The landing page: a header (the installation's name / logo / tagline, or
/// "Dashboards" and a count), the dashboards as cards in groups (ungrouped
/// first, then `-- @group` by name, `A / B` as a sub-heading; each card its
/// title, description, tags, owner, ✓ certified, updated), and — from
/// [`LIST_SEARCH_MIN`] dashboards — a search field, tag chips and "certified
/// only". Server-rendered and escaped; the filters are a small inline script.
pub fn landing(l: &Landing) -> String {
    use std::collections::BTreeMap;
    let light = &l.theme.light;
    let base = l.theme.default_variant();
    let css = match l.theme.mode {
        theme::Mode::Auto => format!(
            "{}\n@media (prefers-color-scheme: dark){{{}}}\n{BASE_LANDING}",
            theme::page_css(light),
            l.theme.dark.css_vars(),
        ),
        _ => format!("{}\n{BASE_LANDING}", theme::page_css(base)),
    };
    let app = l.app;
    let name = app
        .and_then(|a| a.name.as_deref())
        .filter(|n| !n.is_empty())
        .unwrap_or("Dashboards");
    let n = l.items.len();
    let groups_n = l
        .items
        .iter()
        .filter_map(|i| i.meta.group.as_deref())
        .map(|g| g.split(" / ").next().unwrap_or(g))
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let count = format!(
        "{n} dashboard{}{}",
        if n == 1 { "" } else { "s" },
        match groups_n {
            0 => String::new(),
            1 => " in 1 group".into(),
            g => format!(" in {g} groups"),
        }
    );
    let tagline = app
        .and_then(|a| a.tagline.as_deref())
        .filter(|t| !t.is_empty())
        .map(|t| format!("{} · {count}", esc(t)))
        .unwrap_or_else(|| esc(&count).to_string());
    let logo = app
        .and_then(|a| a.logo.as_deref())
        .filter(|u| crate::host::app_brand::image_url_ok(u))
        .map(|u| format!("<img class=\"lp-logo\" src=\"{}\" alt=\"\">", esc(u)))
        .unwrap_or_default();
    let show_name = app.and_then(|a| a.name_visible) != Some(false) || logo.is_empty();
    let h1 = if show_name {
        format!("<h1 class=\"lp-title\">{}</h1>", esc(name))
    } else {
        format!("<h1 class=\"lp-title sr-only\">{}</h1>", esc(name))
    };
    let favicon = app
        .and_then(|a| a.favicon.as_deref())
        .filter(|u| crate::host::app_brand::image_url_ok(u))
        .map(|u| format!("<link rel=\"icon\" href=\"{}\">", esc(u)))
        .unwrap_or_default();

    // group → sub-heading → cards; the key sorts the ungrouped first, then
    // case-insensitively (as the served nav, web/app.js servedNavGroups)
    let mut groups: BTreeMap<(bool, String), BTreeMap<String, Vec<String>>> = BTreeMap::new();
    let mut tag_n: BTreeMap<&str, usize> = BTreeMap::new();
    let mut any_cert = false;
    for it in &l.items {
        let m = it.meta;
        let (g, sub) = match m.group.as_deref() {
            Some(g) => match g.split_once(" / ") {
                Some((a, b)) => (a.to_string(), b.to_string()),
                None => (g.to_string(), String::new()),
            },
            None => (String::new(), String::new()),
        };
        for t in &m.tags {
            *tag_n.entry(t).or_default() += 1;
        }
        any_cert |= m.certified.is_some();
        let desc = m
            .description
            .as_deref()
            .map(|d| format!("<p class=\"lp-desc\">{}</p>", esc(d)))
            .unwrap_or_default();
        let mut meta = String::new();
        if let Some(d) = &m.certified {
            meta.push_str(&format!(
                "<span class=\"lp-cert\" title=\"Certified on {d}\">✓ certified</span>",
                d = esc(d)
            ));
        }
        if let Some(o) = &m.owner {
            meta.push_str(&format!("<span class=\"lp-owner\">{}</span>", esc(o)));
        }
        if let Some(u) = &it.updated {
            meta.push_str(&format!(
                "<span class=\"lp-updated\">Updated <time datetime=\"{u}\">{u}</time></span>",
                u = esc(u)
            ));
        }
        let tags: String = m
            .tags
            .iter()
            .map(|t| format!("<li>{}</li>", esc(t)))
            .collect();
        let tags = if tags.is_empty() {
            String::new()
        } else {
            format!("<ul class=\"lp-tags\" aria-label=\"Tags\">{tags}</ul>")
        };
        let q = [
            m.title.as_str(),
            m.description.as_deref().unwrap_or(""),
            m.group.as_deref().unwrap_or(""),
            m.owner.as_deref().unwrap_or(""),
            &m.tags.join(" "),
        ]
        .iter()
        .filter(|p| !p.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
        let card = format!(
            "<li class=\"lp-card\" data-q=\"{q}\" data-tags=\"{tg}\"{cert}>\
             <a class=\"lp-link\" href=\"{href}\">{title}</a>{desc}\
             {meta_html}{tags}</li>",
            q = esc(&q),
            tg = esc(&m.tags.join("|")),
            cert = if m.certified.is_some() {
                " data-cert"
            } else {
                ""
            },
            href = esc(&it.href),
            title = esc(&m.title),
            meta_html = if meta.is_empty() {
                String::new()
            } else {
                format!("<p class=\"lp-meta\">{meta}</p>")
            },
        );
        groups
            .entry((!g.is_empty(), g.to_lowercase() + "\u{0}" + &g))
            .or_default()
            .entry(sub)
            .or_default()
            .push(card);
    }
    let mut body = String::new();
    for (i, ((grouped, key), subs)) in groups.into_iter().enumerate() {
        let gname = key.split_once('\u{0}').map(|(_, n)| n).unwrap_or("");
        let total: usize = subs.values().map(Vec::len).sum();
        let head = if grouped {
            format!(
                "<h2 class=\"lp-group-h\" id=\"lp-g{i}\">{} <span class=\"lp-n\" aria-label=\"dashboards\">{total}</span></h2>",
                esc(gname)
            )
        } else {
            String::new()
        };
        let label = if grouped {
            format!(" aria-labelledby=\"lp-g{i}\"")
        } else {
            " aria-label=\"Dashboards\"".to_string()
        };
        body.push_str(&format!("<section class=\"lp-group\"{label}>{head}"));
        for (sub, cards) in subs {
            if sub.is_empty() {
                body.push_str(&format!("<ul class=\"lp-cards\">{}</ul>", cards.concat()));
            } else {
                body.push_str(&format!(
                    "<div class=\"lp-sub\"><h3 class=\"lp-sub-h\">{}</h3><ul class=\"lp-cards\">{}</ul></div>",
                    esc(&sub),
                    cards.concat()
                ));
            }
        }
        body.push_str("</section>");
    }
    if l.items.is_empty() {
        body.push_str("<p class=\"lp-empty\">No dashboards to show.</p>");
    }

    let tools = if n >= LIST_SEARCH_MIN {
        let mut tags: Vec<(&str, usize)> = tag_n.into_iter().collect();
        tags.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        let chips: String = tags
            .iter()
            .take(12)
            .map(|(t, _)| {
                format!(
                    "<button type=\"button\" class=\"lp-chip\" data-tag=\"{t}\" aria-pressed=\"false\">{t}</button>",
                    t = esc(t)
                )
            })
            .collect();
        let cert = if any_cert {
            "<button type=\"button\" class=\"lp-chip lp-chip-cert\" id=\"lp-cert\" aria-pressed=\"false\">✓ certified only</button>"
        } else {
            ""
        };
        format!(
            "<div class=\"lp-tools\" role=\"search\">\
             <input id=\"lp-q\" class=\"lp-q\" type=\"search\" placeholder=\"Search dashboards…\" \
             aria-label=\"Search dashboards\" autocomplete=\"off\">\
             <div class=\"lp-chips\" role=\"group\" aria-label=\"Filter\">{cert}{chips}\
             <button type=\"button\" class=\"lp-chip lp-clear\" id=\"lp-clear\" hidden>Clear</button></div>\
             <p class=\"lp-status\" id=\"lp-status\" role=\"status\"></p></div>"
        )
    } else {
        String::new()
    };
    let script = if n >= LIST_SEARCH_MIN {
        format!("<script>{LANDING_SCRIPT}</script>")
    } else {
        String::new()
    };
    format!(
        "<!doctype html><html lang=\"en\" data-theme=\"{tname}\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<meta name=\"color-scheme\" content=\"{scheme}\">{favicon}<title>{title}</title><style>{css}</style></head>\
<body class=\"dp-static dp-landing\"><div class=\"lp\">\
<header class=\"lp-head\">{logo}<div class=\"lp-head-text\">{h1}<p class=\"lp-tagline\">{tagline}</p></div></header>\
<main class=\"lp-main\">{tools}{body}<p id=\"lp-none\" class=\"lp-empty\" hidden>No dashboard matches.</p></main>\
{credit}</div>{script}</body></html>",
        tname = esc(&l.theme.name),
        scheme = match l.theme.mode {
            theme::Mode::Auto => "light dark",
            theme::Mode::Light => "light",
            theme::Mode::Dark => "dark",
        },
        title = esc(name),
        credit = theme::credit_html(),
    )
}

/// The landing page's own rules, after the theme tokens (tokens only, like
/// `web/theme-base.css`).
const BASE_LANDING: &str = include_str!("../web/landing.css");

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::serving::parse_dashboard_meta;

    fn page(
        metas: &[crate::host::serving::DashboardMeta],
        app: Option<&crate::host::app_brand::AppBrand>,
    ) -> String {
        let set = theme::resolve("").unwrap();
        landing(&Landing {
            theme: &set,
            app,
            items: metas
                .iter()
                .enumerate()
                .map(|(i, m)| LandingItem {
                    href: format!("/d/x{i}"),
                    meta: m,
                    updated: Some("2026-10-17".into()),
                })
                .collect(),
        })
    }

    #[test]
    fn landing_page() {
        let metas = [
            parse_dashboard_meta("z", "-- @title Zeta\nSELECT 1;"),
            parse_dashboard_meta(
                "s",
                "-- @title Weekly <sales>\n-- @group Sales / EMEA\n-- @tags kpi\n-- @owner Fin & Co\n-- @certified 2026-10-15\n-- @description Which \"stores\" sell?\nSELECT 1;",
            ),
            parse_dashboard_meta("o", "-- @title Ops board\n-- @group ops\nSELECT 1;"),
            parse_dashboard_meta("m", "-- @title Margin\n-- @group Sales\nSELECT 1;"),
        ];
        let html = page(&metas, None);
        let at = |s: &str| html.find(s).unwrap_or_else(|| panic!("{s} missing"));
        // ungrouped first, then the groups by name (case-insensitive) with
        // counts, a sub-heading for `A / B`
        assert!(at(">Zeta<") < at(">ops <") && at(">ops <") < at(">Sales <"));
        assert!(at(">Sales <") < at(">Margin<") && at(">Margin<") < at(">EMEA</h3>"));
        assert!(at(">EMEA</h3>") < at("Weekly &lt;sales&gt;"));
        assert!(
            html.contains("aria-label=\"dashboards\">2</span>"),
            "Sales counts 2"
        );
        assert!(html.contains("✓ certified") && html.contains("Certified on 2026-10-15"));
        assert!(html.contains("<span class=\"lp-owner\">Fin &amp; Co</span>"));
        assert!(html.contains("<p class=\"lp-desc\">Which &quot;stores&quot; sell?</p>"));
        assert!(html.contains("<time datetime=\"2026-10-17\">"));
        assert!(html.contains("<h1 class=\"lp-title\">Dashboards</h1>"));
        assert!(html.contains("4 dashboards in 2 groups"));
        assert!(
            html.contains("prefers-color-scheme: dark"),
            "the default theme follows the viewer"
        );
        assert!(html.contains("Powered by"));
        assert!(
            !html.contains("id=\"lp-q\"") && !html.contains("<script>"),
            "no filters for 4"
        );
        // the filters from LIST_SEARCH_MIN on
        let many: Vec<_> = (0..LIST_SEARCH_MIN).map(|_| metas[1].clone()).collect();
        let html = page(&many, None);
        assert!(
            html.contains("id=\"lp-q\"")
                && html.contains("id=\"lp-cert\"")
                && html.contains("data-tag=\"kpi\"")
        );
        assert!(html.contains("<script>"));
        // the installation's name, tagline and logo
        let mut app = crate::host::app_brand::AppBrand::default();
        app.set("name", "Bakery <BI>", false).unwrap();
        app.set("tagline", "Sales & stock", false).unwrap();
        app.set("logo", "https://example.com/l.png", false).unwrap();
        let html = page(&metas[..1], Some(&app));
        assert!(html.contains("<h1 class=\"lp-title\">Bakery &lt;BI&gt;</h1>"));
        assert!(html.contains("Sales &amp; stock · 1 dashboard"));
        assert!(html.contains("<img class=\"lp-logo\" src=\"https://example.com/l.png\""));
        assert!(html.contains("<title>Bakery &lt;BI&gt;</title>"));
        assert!(page(&[], None).contains("No dashboards to show."));
    }
}
