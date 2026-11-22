//! Static dashboard pages — the CLI's `dashboard file.sql out.html` and
//! `serve --dashboards`. Both share one page shell: the theme's CSS tokens +
//! `web/theme-base.css` (the browser's dashboard stylesheet), the dashboard
//! header (title / brand / logo), the panels and a small hover script.

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
/// (same wording as `web/app.js`'s chip, in the page language: #877); amber past `data-stale` seconds. A
/// date-only chip (`data-asof-day`) reads as a calendar day instead — no
/// time, no time-zone shift (#282).
pub const ASOF_SCRIPT: &str = r#"(function(){
  var c=document.querySelector('.dp-asof[data-asof]'); if(!c) return;
  var at=+c.getAttribute('data-asof'), st=+c.getAttribute('data-stale')||0, lang=document.documentElement.lang||undefined;
  var day=c.getAttribute('data-asof-day');
  var lg=String(lang||'en').slice(0,2).toLowerCase(),W=lg==='de'?['Stand:','gerade eben','heute','vor # Min.','vor # Std.','vor # Tagen']:lg==='fr'?['Données au','à l’instant','aujourd’hui','il y a # min','il y a # h','il y a # j']:['Data as of','just now','today','# min ago','# h ago','# d ago'];
  function w(i,n){return W[i].replace('#',n);}
  function rel(s){s=Math.max(0,s);return s<60?W[1]:s<3600?w(3,Math.floor(s/60)):s<172800?w(4,Math.floor(s/3600)):w(5,Math.floor(s/86400));}
  function tick(){
    if(day){var p=day.split('-'),d=new Date(+p[0],+p[1]-1,+p[2]),now=Date.now(),n=new Date(now),days=Math.round((new Date(n.getFullYear(),n.getMonth(),n.getDate())-d)/864e5),t,age=(now-d.getTime())/1000;
      try{t=d.toLocaleDateString(lang);}catch(e){t=day;}
      c.textContent=W[0]+' '+t+' ('+(days<=0?W[2]:w(5,days))+')'; c.title=day; c.classList.toggle('stale',st>0&&age>st); return;}
    var d=new Date(at),now=Date.now(),age=(now-at)/1000,t;
    try{t=d.toLocaleTimeString(lang,{hour:'2-digit',minute:'2-digit'});if(d.toDateString()!==new Date(now).toDateString())t=d.toLocaleDateString(lang)+' '+t;}catch(e){t=d.toISOString().slice(0,16).replace('T',' ');}
    c.textContent=W[0]+' '+t+' ('+rel(age)+')'; c.title=d.toString(); c.classList.toggle('stale',st>0&&age>st);}
  tick(); setInterval(tick,60000);
})();"#;

/// A no-JS `form.controls` (the `@param` selects of `serve --dashboards`'
/// script-free pages) submits on change, same as a JS-driven dashboard's own
/// control would; the HTML keeps a visible submit button so the page still
/// works with this script blocked or absent (#557: the CSP (#252) forbids
/// the inline `onchange` attribute this replaced — a bare event-handler
/// attribute isn't covered by a script nonce).
pub const CONTROLS_SCRIPT: &str = r#"(function(){
  [].slice.call(document.querySelectorAll('form.controls select')).forEach(function(el){
    el.addEventListener('change',function(){el.form.submit();});
  });
})();"#;

/// What goes on a static page.
pub struct StaticPage<'a> {
    pub theme: &'a ResolvedTheme,
    /// `<title>` when the dashboard sets no `::DASHBOARD_TITLE`.
    pub fallback_title: &'a str,
    /// The dashboard's `-- @description`, under the title (#1345); text.
    pub description: Option<&'a str>,
    /// A line under the header (e.g. "rendered from x.sql · 4 panels"); HTML.
    pub subtitle_html: String,
    /// Extra `<head>` markup (e.g. a meta refresh).
    pub head_extra: String,
    /// Controls (served parameter form), HTML.
    pub controls_html: String,
    /// The panels (`<figure class="panel">…` / `<h2 class="section">…`), HTML.
    pub panels_html: String,
    /// Fit the panels to the window ([`static_fit`]: a top-level `::FIT`
    /// in a dashboard without tabs).
    pub fit: bool,
}

/// A panel's place on a static page (#1292): the `::SPAN` (of 12 columns)
/// and `::HEIGHT` statements right before it, as the browser and
/// `dashboard --svg` read them. Both apply to the next panel only.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Slot {
    pub span: Option<u32>,
    pub height: Option<crate::height::PanelHeight>,
}

impl Slot {
    /// Is this statement a layout one the slot reads: `::SPAN`, `::HEIGHT`
    /// or both (`SELECT 12 ::SPAN, 680 ::HEIGHT;`, #1286) and nothing else?
    pub fn is_layout(roles: &[(usize, crate::Role)]) -> bool {
        !roles.is_empty()
            && roles
                .iter()
                .all(|(_, r)| matches!(r, crate::Role::Span | crate::Role::Height))
    }

    /// Take a `::SPAN` / `::HEIGHT` statement into the slot for the next
    /// panel, each from its own column (`cell(i)`: the first row's cell of
    /// column `i`); `false` when the statement is no such one.
    pub fn read(
        &mut self,
        roles: &[(usize, crate::Role)],
        cell: impl Fn(usize) -> Option<String>,
    ) -> bool {
        if !Self::is_layout(roles) {
            return false;
        }
        for (i, r) in roles {
            match r {
                crate::Role::Span => {
                    self.span = cell(*i)
                        .and_then(|t| t.trim().parse::<f64>().ok())
                        .filter(|n| n.is_finite() && *n >= 1.0)
                        .map(|n| (n as u32).min(12));
                }
                _ => self.height = cell(*i).and_then(|t| crate::height::PanelHeight::parse(&t)),
            }
        }
        true
    }

    /// The SQL that reads a layout statement's cells by position: its
    /// columns renamed `c0`, `c1`, … so two equal values (`6::SPAN,
    /// 6::HEIGHT`) don't share a name.
    pub fn query(roles: &[(usize, crate::Role)], sql: &str) -> String {
        let n = roles.iter().map(|(i, _)| i + 1).max().unwrap_or(1);
        let cols: Vec<String> = (0..n).map(|i| format!("c{i}")).collect();
        format!(
            "SELECT * FROM (\n{}\n) AS dp_layout({}) LIMIT 1",
            sql.trim().trim_end_matches(';'),
            cols.join(", ")
        )
    }

    /// The drawing size: 460 × 300 as before for a panel without either;
    /// a span widens it in steps of a twelfth of a two-panel row (6 = 460,
    /// 12 = 928), a height sets the height (a share of the window as of a
    /// 900 px high one).
    pub fn size(&self) -> (u32, u32) {
        let w = self.span.map_or(460, |s| 78 * s.clamp(1, 12) - 8);
        let h = self.height.map_or(300, |h| {
            h.px(w as f64, 900.0)
                .clamp(crate::height::MIN_PX, crate::height::MAX_PX)
        });
        (w, h)
    }

    /// The figure's extra class: `dp-span-N` places it on the 12-column grid
    /// a page with any span switches to (see [`render`]).
    pub fn class(&self) -> String {
        self.span
            .map(|s| format!(" dp-span-{}", s.clamp(1, 12)))
            .unwrap_or_default()
    }
}

/// Does a static page fit its panels to the window (#400)? Only for a
/// dashboard-wide `::FIT`: one outside any `::VIEW` in a dashboard with no
/// `::TAB` / `::SUBTAB` — a static page flattens tabs (every tab's panels on
/// one page) and shows a `::VIEW` group's first alternative inline, so a
/// tab's, sub-tab's or view's `::FIT` has no section of its own to fit there.
pub fn static_fit(plan: &[crate::sql::Panel]) -> bool {
    let live = |p: &&crate::sql::Panel| !p.setup;
    !plan.iter().filter(live).any(|p| p.tab_kind().is_some())
        && plan
            .iter()
            .filter(live)
            .any(|p| p.view.is_none() && p.roles.iter().any(|(_, r)| *r == crate::Role::Fit))
}

/// The static page's fit-to-screen, CSS only (no script measures anything):
/// the page is the window tall, the header keeps its height and the panels'
/// grid takes the rest in equal rows of at least 120 px — more rows than fit
/// overflow it and the page scrolls, as the live app falls back. Each chart
/// (its pre-rendered SVG, under its title) fills its panel's height, scaled uniformly by its
/// viewBox (`preserveAspectRatio` meet: crisp, not distorted).
const FIT_CSS: &str = "html{height:100%}\
body.dp-static.dp-fit{height:100vh;min-height:0;box-sizing:border-box;display:flex;flex-direction:column;margin:0}\
body.dp-fit>.grid{flex:1 1 auto;min-height:0;display:flex;flex-direction:column}\
body.dp-fit>.grid>*{flex:none}\
body.dp-fit>.grid>.dp-fit-grid{flex:1 1 auto;min-height:0;display:grid;grid-template-columns:repeat(auto-fit,minmax(440px,1fr));grid-auto-rows:minmax(120px,1fr);gap:inherit}\
body.dp-fit .dp-fit-grid>.panel{min-height:0;overflow:hidden;display:flex;flex-direction:column}\
body.dp-fit .dp-fit-grid>.panel>svg{flex:none}\
body.dp-fit .dp-fit-grid>.panel>svg:last-child{flex:1 1 0;min-height:0;width:100%;height:100%}\
body.dp-fit .dp-fit-grid>h2.section{grid-column:1/-1;align-self:end}";

/// #1248: the Step / Play controls of a chart with steps (`data-story`: a
/// `::STORY` chart, a `::DIAGRAM` or a factor graph's message schedule).
/// The app builds them with `web/anim.js`'s kit; a static or served page
/// embeds the same source (one implementation, as the app's own HTML export
/// does) and starts each stepper at its overview. The kit puts its controls
/// after the chart's holder, which on these pages is wrapped in the app's
/// `.panel-svg` first. Only pages with such a chart carry it.
fn stepper_script() -> String {
    let kit =
        include_str!("../web/anim.js").replacen("export function animKit", "function animKit", 1);
    format!(
        "(function(){{{kit}\nvar K=animKit(window);\
document.querySelectorAll('.panel svg[data-story]').forEach(function(s){{\
var p=s.closest('.panel');if(!p)return;\
if(s.parentElement===p){{var h=document.createElement('div');h.className='panel-svg';s.before(h);h.append(s);}}\
K.story(p,s);}});}})();"
    )
}

/// #1240: uppercased labels keep their Greek letters as written (μ, not M).
/// The page embeds `web/nocase.js` (the app's own function) and runs it
/// once; only pages with Greek letters carry it.
fn nocase_script() -> String {
    let f = include_str!("../web/nocase.js").replacen(
        "export function keepMathCase",
        "function keepMathCase",
        1,
    );
    format!("(function(){{{f}\nkeepMathCase(document.body);}})();")
}

/// Whether `s` has a Greek letter (the Greek and Coptic, and Greek Extended
/// blocks): the letters CSS `text-transform: uppercase` turns into Latin-looking
/// capitals.
pub(crate) fn has_greek(s: &str) -> bool {
    s.chars().any(is_greek)
}

fn is_greek(c: char) -> bool {
    matches!(c, '\u{0370}'..='\u{03ff}' | '\u{1f00}'..='\u{1fff}')
}

/// `s` in capitals, its Greek letters as written (#1240): for labels the
/// renderer uppercases itself (an SVG has no `text-transform`).
pub(crate) fn upper_keep_greek(s: &str) -> String {
    s.chars()
        .flat_map(|c| -> Box<dyn Iterator<Item = char>> {
            if is_greek(c) {
                Box::new(std::iter::once(c))
            } else {
                Box::new(c.to_uppercase())
            }
        })
        .collect()
}

/// #1345: a static page keeps the dashboard's `::TAB`s. Each tab's panels
/// sit in a `<section class="dp-tab">` under the tab's name
/// ([`tab_open`]); the section is `display: contents`, so its panels stay on
/// the page's grid. [`TABS_SCRIPT`] turns the sections into a tab bar and
/// shows one at a time; without scripts, and in print, every tab shows under
/// its heading.
pub const TAB_CLASS: &str = "class=\"dp-tab\"";

/// The opening of a tab's section, closing the one before it (`open`).
pub fn tab_open(label: &str, open: bool) -> String {
    format!(
        "{}<section {TAB_CLASS} data-tab=\"{}\"><h2 class=\"section dp-tab-title\">{}</h2>",
        if open { "</section>" } else { "" },
        esc(label),
        esc(label)
    )
}

/// An input's place on a static page (#1345): the page can't change it, so a
/// note says it shows the dashboard's default choice.
pub fn input_note(label: &str, shown: &str) -> String {
    let (label, shown) = (label.trim(), shown.trim());
    let what = if label.is_empty() {
        String::new()
    } else {
        format!("<b>{}</b>: ", esc(label))
    };
    let choice = if shown.is_empty() || shown.eq_ignore_ascii_case("null") {
        "this page shows the default choice".to_string()
    } else {
        format!("this page shows <b>{}</b>, the default", esc(shown))
    };
    format!("<p class=\"dp-input-note\">{what}{choice} — change it in the live dashboard</p>")
}

/// The tab bar uses the app's own `.tabbar` / `.tab-btn` styles (embedded with
/// the theme's CSS); only the sections' behaviour and the print view are here.
const TABS_CSS: &str = "section.dp-tab{display:contents}section.dp-tab[hidden]{display:none}\
.dp-tabs-on h2.dp-tab-title{display:none}\
@media print{.tabbar{display:none}section.dp-tab[hidden]{display:contents}.dp-tabs-on h2.dp-tab-title{display:flex}}";

const TABS_SCRIPT: &str = "(function(){var s=[].slice.call(document.querySelectorAll('section.dp-tab'));\
if(s.length<2)return;var nav=document.createElement('div');\
nav.className='tabbar';nav.setAttribute('role','tablist');\
var bs=s.map(function(sec,i){var b=document.createElement('button');b.type='button';b.className='tab-btn';\
b.setAttribute('role','tab');b.textContent=sec.getAttribute('data-tab');\
b.addEventListener('click',function(){show(i);});nav.append(b);return b;});\
function show(k){s.forEach(function(sec,i){sec.hidden=i!==k;bs[i].classList.toggle('active',i===k);\
bs[i].setAttribute('aria-selected',i===k?'true':'false');});window.dispatchEvent(new Event('resize'));}\
s[0].parentElement.insertBefore(nav,s[0]);document.body.classList.add('dp-tabs-on');show(0);})();";

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
    // the dashboard's description (#1345), then the source line
    let mut sub = p
        .description
        .filter(|d| !d.trim().is_empty())
        .map(|d| format!("<p class=\"dp-desc\">{}</p>", esc(d.trim())))
        .unwrap_or_default();
    if !p.subtitle_html.is_empty() {
        sub.push_str(&format!("<div class=\"dp-src\">{}</div>", p.subtitle_html));
    }
    format!(
        "<!doctype html><html lang=\"{lang}\" data-theme=\"{name}\" data-variant=\"{variant}\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">{head_extra}{favicon}<title>{title}</title>\
<style>{css}{fit_css}{tabs_css}</style></head>\
<body class=\"dp-static{fit_class}\"><div class=\"grid{spans}\">{header}{sub}{controls}{panels}</div>\
<div id=\"dp-tip\" class=\"dp-tip\"></div><script>{HOVER_SCRIPT}{ASOF_SCRIPT}{CONTROLS_SCRIPT}{stepper}{nocase}{tabs}</script></body></html>",
        lang = esc(t.locale.as_deref().unwrap_or("en")),
        name = esc(&t.name),
        variant = t.variant.as_str(),
        head_extra = p.head_extra,
        favicon = theme::favicon_html(t),
        title = esc(&title),
        css = theme::page_css(t),
        controls = p.controls_html,
        fit_css = if p.fit { FIT_CSS } else { "" },
        tabs_css = if p.panels_html.contains(TAB_CLASS) { TABS_CSS } else { "" },
        tabs = if p.panels_html.contains(TAB_CLASS) { TABS_SCRIPT } else { "" },
        fit_class = if p.fit { " dp-fit" } else { "" },
        spans = if p.panels_html.contains(" dp-span-") { " dp-spans" } else { "" },
        stepper = if p.panels_html.contains("data-story=") { stepper_script() } else { String::new() },
        nocase = if [&header, p.controls_html.as_str(), p.panels_html.as_str()].iter().any(|h| has_greek(h)) {
            nocase_script()
        } else {
            String::new()
        },
        panels = if p.fit {
            format!("<div class=\"dp-fit-grid\">{}</div>", p.panels_html)
        } else {
            p.panels_html.clone()
        },
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
        description: None,
        subtitle_html: String::new(),
        head_extra: String::new(),
        controls_html: String::new(),
        panels_html: format!("<ul class=\"dash-list\">{lis}</ul>"),
        fit: false,
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
/// `-- @nav` label if set (else `-- @title`, with the full title as the
/// card's tooltip when `@nav` shortens it), description, tags, owner, ✓
/// certified, updated), and — from [`LIST_SEARCH_MIN`] dashboards — a search
/// field, tag chips and "certified only". Server-rendered and escaped; the
/// filters are a small inline script.
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
    let ok = |u: &&str| crate::host::app_brand::image_url_ok(u);
    let light_logo = app.and_then(|a| a.logo.as_deref()).filter(ok);
    let dark_logo = app.and_then(|a| a.logo_dark.as_deref()).filter(ok);
    let logo = match (light_logo, dark_logo, l.theme.mode) {
        (None, _, _) => String::new(),
        (Some(u), Some(d), theme::Mode::Auto) => format!(
            "<picture><source media=\"(prefers-color-scheme: dark)\" srcset=\"{}\"><img class=\"lp-logo\" src=\"{}\" alt=\"\"></picture>",
            esc(d), esc(u)),
        (Some(_), Some(d), theme::Mode::Dark) => format!("<img class=\"lp-logo\" src=\"{}\" alt=\"\">", esc(d)),
        (Some(u), _, _) => format!("<img class=\"lp-logo\" src=\"{}\" alt=\"\">", esc(u)),
    };
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
            m.nav.as_deref().unwrap_or(""),
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
        // the compact `-- @nav` label when set (else `-- @title`); the full
        // title stays reachable as the link's tooltip so a shortened label
        // doesn't bury the business question it stands for.
        let label = m.nav_label();
        let card = format!(
            "<li class=\"lp-card\" data-q=\"{q}\" data-tags=\"{tg}\"{cert}>\
             <a class=\"lp-link\" href=\"{href}\"{nav_title}>{label}</a>{desc}\
             {meta_html}{tags}</li>",
            q = esc(&q),
            tg = esc(&m.tags.join("|")),
            cert = if m.certified.is_some() {
                " data-cert"
            } else {
                ""
            },
            href = esc(&it.href),
            nav_title = if m.nav.is_some() {
                format!(" title=\"{}\"", esc(&m.title))
            } else {
                String::new()
            },
            label = esc(label),
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
</div>{script}</body></html>",
        tname = esc(&l.theme.name),
        scheme = match l.theme.mode {
            theme::Mode::Auto => "light dark",
            theme::Mode::Light => "light",
            theme::Mode::Dark => "dark",
        },
        title = esc(name),
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

    fn static_page(panels: &str) -> String {
        let set = theme::resolve("").unwrap();
        render(&StaticPage {
            theme: set.pick(theme::Variant::Light),
            fallback_title: "t",
            description: None,
            subtitle_html: String::new(),
            head_extra: String::new(),
            controls_html: String::new(),
            panels_html: panels.into(),
            fit: false,
        })
    }

    #[test]
    fn a_chart_with_steps_gets_the_stepper_1248() {
        let html = static_page(
            r#"<div class="panel"><svg data-story="[{&quot;k&quot;:&quot;a&quot;}]"></svg></div>"#,
        );
        let script = &html[html.rfind("<script>").unwrap()..];
        assert!(
            script.contains("function animKit") && script.contains("K.story(p,s)"),
            "{script}"
        );
        // a classic script: the module's `export` would be a syntax error
        assert!(!script.contains("export function"));
        assert!(!script.contains("</script>") || script.matches("</script>").count() == 1);
        // without steps, the page stays small
        let plain = static_page(r#"<div class="panel"><svg></svg></div>"#);
        assert!(!plain.contains("animKit"));
    }

    #[test]
    fn greek_letters_keep_their_case_1240() {
        assert_eq!(
            upper_keep_greek("Average cycle μ, min"),
            "AVERAGE CYCLE μ, MIN"
        );
        assert_eq!(
            upper_keep_greek("How much a job varies (τ)"),
            "HOW MUCH A JOB VARIES (τ)"
        );
        assert_eq!(upper_keep_greek("straße"), "STRASSE");
        let set = theme::resolve("").unwrap();
        let page = |panels: &str| {
            render(&StaticPage {
                theme: set.pick(theme::Variant::Light),
                fallback_title: "t",
                description: None,
                subtitle_html: String::new(),
                head_extra: String::new(),
                controls_html: String::new(),
                panels_html: panels.into(),
                fit: false,
            })
        };
        let greek =
            page(r#"<div class="panel"><div class="metric-cap">Average cycle μ</div></div>"#);
        let script = &greek[greek.rfind("<script>").unwrap()..];
        assert!(script.contains("function keepMathCase") && !script.contains("export function"));
        assert!(!page(r#"<div class="panel">Plain</div>"#).contains("keepMathCase"));
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
        assert!(!html.contains("Powered by"));
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
        // logo_dark (#198): auto mode uses <picture>, dark mode uses the dark logo
        app.set("logo_dark", "https://example.com/d.png", false)
            .unwrap();
        let html = page(&metas[..1], Some(&app));
        assert!(html.contains("srcset=\"https://example.com/d.png\""));
        assert!(html.contains("<img class=\"lp-logo\" src=\"https://example.com/l.png\""));
        let dark_set = theme::resolve("dark").unwrap();
        assert!(matches!(dark_set.mode, theme::Mode::Dark));
        let html = landing(&Landing {
            theme: &dark_set,
            app: Some(&app),
            items: vec![],
        });
        assert!(html.contains("src=\"https://example.com/d.png\""));
        assert!(!html.contains("<picture>"));
        assert!(page(&[], None).contains("No dashboards to show."));
    }

    /// #355: a card shows `-- @nav` in place of `-- @title` when set, with
    /// the full title kept as the link's tooltip; without `-- @nav` the
    /// card still shows `-- @title` and carries no such tooltip.
    #[test]
    fn landing_card_prefers_nav_label() {
        let metas = [
            parse_dashboard_meta(
                "s",
                "-- @title Which products depend on this supplier, and how badly?\n\
                 -- @nav Supplier risk\nSELECT 1;",
            ),
            parse_dashboard_meta("o", "-- @title Ops board\nSELECT 1;"),
        ];
        let html = page(&metas, None);
        assert!(
            html.contains(">Supplier risk<"),
            "shows the short @nav label"
        );
        assert!(
            !html.contains(">Which products depend"),
            "the long @title is not the visible label"
        );
        assert!(
            html.contains("title=\"Which products depend on this supplier, and how badly?\""),
            "the full @title is kept as the card's tooltip"
        );
        // without @nav, the title is the label and gets no such tooltip
        assert!(html.contains(">Ops board<"));
        assert!(!html.contains("title=\"Ops board\""));
    }
}

#[cfg(test)]
mod slot_tests {
    use super::*;
    use crate::height::PanelHeight;
    use crate::Role;

    /// #1292: a ::SPAN / ::HEIGHT statement sizes the next panel; nothing
    /// set keeps the old 460 × 300.
    #[test]
    fn span_and_height_size_the_panel() {
        let mut s = Slot::default();
        assert_eq!(s.size(), (460, 300));
        assert_eq!(s.class(), "");
        assert!(s.read(&[(0, Role::Span)], one("12")));
        assert!(s.read(&[(0, Role::Height)], one("680")));
        assert_eq!(s.size(), (928, 680));
        assert_eq!(s.class(), " dp-span-12");
        let mut s = Slot::default();
        s.read(&[(0, Role::Span)], one("6"));
        assert_eq!(s.size(), (460, 300));
        // a ratio of the width; out-of-range spans clamp; junk is ignored
        let mut s = Slot::default();
        s.read(&[(0, Role::Span)], one("40"));
        s.height = Some(PanelHeight::Ratio(0.5));
        assert_eq!(s.size(), (928, 464));
        let mut s = Slot::default();
        s.read(&[(0, Role::Span)], one("wide"));
        assert_eq!(s, Slot::default());
        assert!(!s.read(&[(0, Role::Title)], one("x")));
    }

    fn one(v: &str) -> impl Fn(usize) -> Option<String> + '_ {
        move |i| (i == 0).then(|| v.to_string())
    }

    /// #1286: one statement may set both, each from its own column.
    #[test]
    fn one_statement_sets_span_and_height() {
        let mut s = Slot::default();
        let roles = [(0, Role::Span), (1, Role::Height)];
        assert!(Slot::is_layout(&roles));
        assert!(s.read(&roles, |i| Some(["12", "680"][i].to_string())));
        assert_eq!(s.size(), (928, 680));
        assert!(!Slot::is_layout(&[(0, Role::Span), (1, Role::Title)]));
        assert_eq!(
            Slot::query(&roles, "SELECT 6, 6;"),
            "SELECT * FROM (\nSELECT 6, 6\n) AS dp_layout(c0, c1) LIMIT 1"
        );
    }

    #[test]
    fn a_page_with_a_span_gets_the_twelve_column_grid() {
        let t = theme::default_light();
        let page = |panels: &str| {
            render(&StaticPage {
                theme: &t,
                fallback_title: "t",
                description: None,
                subtitle_html: String::new(),
                head_extra: String::new(),
                controls_html: String::new(),
                panels_html: panels.into(),
                fit: false,
            })
        };
        assert!(page("<figure class=\"panel dp-span-12\"></figure>")
            .contains("class=\"grid dp-spans\""));
        assert!(page("<figure class=\"panel\"></figure>").contains("class=\"grid\""));
    }
}

#[cfg(test)]
mod fit_tests {
    use super::*;
    use crate::sql::plan;

    /// #400: a static page fits only for a dashboard-wide ::FIT (no tabs,
    /// not inside a ::VIEW), with CSS alone.
    #[test]
    fn static_fit_only_for_a_dashboard_wide_fit() {
        let chart = "SELECT 1 AS x ::XAXIS, 2 AS y ::BARCHART;";
        assert!(static_fit(&plan(&format!("SELECT 'fit'::FIT; {chart}"))));
        assert!(!static_fit(&plan(chart)));
        assert!(!static_fit(&plan(&format!(
            "SELECT 'A'::TAB; SELECT 'fit'::FIT; {chart}"
        ))));
        assert!(!static_fit(&plan(&format!(
            "SELECT 'L' AS v ::VIEW; SELECT 'fit'::FIT; {chart} SELECT 'B' AS v ::VIEW; {chart}"
        ))));
    }

    #[test]
    fn a_fitted_static_page_sizes_rows_with_css_only() {
        let t = theme::default_light();
        let page = |fit| {
            render(&StaticPage {
                theme: &t,
                fallback_title: "x",
                description: None,
                subtitle_html: String::new(),
                head_extra: String::new(),
                controls_html: String::new(),
                panels_html: "<figure class=\"panel\"><svg viewBox=\"0 0 460 300\"></svg></figure>"
                    .into(),
                fit,
            })
        };
        let on = page(true);
        assert!(on.contains("<body class=\"dp-static dp-fit\">"));
        assert!(on.contains("<div class=\"dp-fit-grid\"><figure class=\"panel\">"));
        assert!(on.contains("height:100vh") && on.contains("grid-auto-rows:minmax(120px,1fr)"));
        assert!(
            !FIT_CSS.contains("transform"),
            "scaled by viewBox, not a CSS transform"
        );
        let off = page(false);
        assert!(
            off.contains("<body class=\"dp-static\">")
                && !off.contains("<div class=\"dp-fit-grid\">")
        );
    }
}
