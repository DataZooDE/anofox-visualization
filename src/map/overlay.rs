//! The flow and hexbin overlays: drawn by ggplot-rs instead of hand-written
//! SVG — see issue #318 (visualization logic belongs in ggplot-rs).
//!
//! - **Flows** (`::ORIGIN`/`::DEST`, M2b): `geom_curve` + `arrow()` + the
//!   `linewidth` aesthetic + an identity colour scale + interactive
//!   aesthetics — upstream ggplot-rs issues #125 (arrow), #126 (linewidth),
//!   #128 (identity scales), #129 (interactive aesthetics).
//! - **Hexbin** (`::HEXBIN`, M2c): `geom_hex` drawing cells that
//!   `hexbin::of`'s `StatBinHex::compute_group` already binned, through
//!   `StatIdentity` (the consumer pre-bins; `geom_hex`'s own default stat
//!   would re-bin) and `scale_fill_identity` for the pre-resolved per-cell
//!   colour — upstream ggplot-rs issues #127 (the `StatBinHex` rewrite onto
//!   a fixed lattice) and #128 (identity scales).
//!
//! The consumer still does the data prep (`flows.rs`/`hexbin.rs`:
//! unprojected endpoint/point parsing, matching codes, width/colour
//! resolution before drawing) and the legend box math (`legend.rs`); this
//! module only builds a `DataFrame` from already-projected rows and
//! composes the rendered fragment.
//!
//! Positions are fed through **already-projected, pre-pixel** coordinates
//! (`Flow::o`/`Flow::d`, hexbin cell centres): the plot's `CoordSf`, built
//! from the same `Frame::window`, performs the identical data→pixel affine
//! transform `Frame::at` used to do by hand, so feeding it pixel
//! coordinates too would double-transform every position.

use ggplot_rs::prelude::*;

use crate::theme::{hex, mix};
use crate::{find_role, value_str, Column, RenderOptions, Role};

use super::flows::{Flows, FLOW_CAP};
use super::hexbin::{self, Hexbin};
use super::{Fill, Frame, Model, Rgb, ValueScale};

/// Draw the flow overlay and splice it into the map body. Returns the
/// rendered SVG fragment (empty when every flow turned out screen-coincident
/// or there were none to draw), the number of marks drawn (for the
/// "aggregate in SQL" budget warning), the count of screen-coincident
/// ("self") flows skipped (pixel distance < 2px, checked here since the
/// drawn coordinates themselves stay in projected, not pixel, space), and
/// the over-cap warning text (`fl.total > FLOW_CAP`), if any.
#[allow(clippy::too_many_arguments)]
pub(super) fn render(
    o: &RenderOptions,
    cols: &[Column],
    m: &Model,
    frame: &Frame,
    fl: &Flows,
    t: &crate::theme::ResolvedTheme,
    card: Rgb,
    hatch_bg: Rgb,
    w: f64,
    h: f64,
) -> (String, usize, usize, Option<String>) {
    let linestyle_col = find_role(cols, Role::LineStyle);
    let fmt_full = |s: &ValueScale, v: f64| s.fmt.full(v, &o.locale);

    let (mut xs, mut ys, mut xends, mut yends) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut colors = Vec::new();
    let mut sizes = Vec::new();
    let mut tooltips = Vec::new();
    let mut rows = Vec::new();
    let mut seriess = Vec::new();
    let mut values = Vec::new();
    let mut labels = Vec::new();
    let mut origs = Vec::new();
    let mut dests = Vec::new();
    let mut raw_sizes = Vec::new();
    let mut fsizes = Vec::new();
    let mut linestyles = Vec::new();
    let mut nds = Vec::new();
    let mut keys = Vec::new();

    let mut end_pts: std::collections::HashMap<&str, [f64; 2]> = std::collections::HashMap::new();
    let mut extra_self = 0usize;

    for fl_ in &fl.flows {
        let i = fl_.row;
        // Screen-coincident ("self") check: pixel distance, computed only
        // to decide whether this row is drawable — the drawn x/y/xend/yend
        // below stay in projected (pre-pixel) space, see the module doc.
        let (px0, py0) = frame.at(fl_.o);
        let (px1, py1) = frame.at(fl_.d);
        if (px1 - px0).hypot(py1 - py0) < 2.0 {
            extra_self += 1;
            continue;
        }

        let label = m.label_of(i);
        let cat = m.text(m.category, i);
        let series = if cat.is_empty() {
            fl_.o_name.clone()
        } else {
            cat.clone()
        };
        let (color, value, class, nd) = match &m.fill {
            Fill::Measure(s) => match s.values[i] {
                Some(v) => {
                    let (c, k) = s.color(v);
                    (c, Some(v), k, false)
                }
                None => (hatch_bg, None, None, true),
            },
            Fill::Category { of_row, levels } => (
                of_row[i].map(|k| levels[k].1).unwrap_or(hatch_bg),
                None,
                None,
                false,
            ),
            Fill::Plain(c) => (*c, None, None, false),
        };
        let linestyle = linestyle_col
            .and_then(|lc| lc.values.get(i))
            .map(value_str)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        let shown = match (&m.fill, value) {
            (Fill::Measure(s), Some(v)) => fmt_full(s, v),
            (Fill::Measure(_), None) => "no data".to_string(),
            (Fill::Category { .. }, _) => cat.clone(),
            (Fill::Plain(_), _) if fl.has_size => fl.fmt.full(fl_.raw.unwrap_or(fl_.w), &o.locale),
            _ => String::new(),
        };
        let flow_label = if label.is_empty() {
            format!("{} → {}", fl_.o_name, fl_.d_name)
        } else {
            label.clone()
        };
        let mut title = format!("{flow_label}: {shown}");
        if title.ends_with(": ") {
            title.truncate(title.len() - 2);
        }
        if let (Fill::Measure(s), Some(k)) = (&m.fill, class) {
            if let Some((_, cl, _)) = &s.classes {
                title.push_str(&format!(
                    " (class {} of {}: {})",
                    k + 1,
                    cl.len(),
                    cl.label(k, &|v| s.fmt.short(v, &o.locale))
                ));
            }
        }
        let key = format!("{}→{}", fl_.ok, fl_.dk);
        let data_value = value.unwrap_or(fl_.raw.unwrap_or(fl_.w));
        let fsize = fl_.raw.map(|raw| fl.fmt.full(raw, &o.locale));

        xs.push(Value::Float(fl_.o[0]));
        ys.push(Value::Float(fl_.o[1]));
        xends.push(Value::Float(fl_.d[0]));
        yends.push(Value::Float(fl_.d[1]));
        colors.push(Value::Str(hex(if nd { hatch_bg } else { color })));
        // The raw, unscaled magnitude — the linewidth *scale* does the sqrt
        // scaling (when one applies, see `width_const` below), not this row.
        sizes.push(Value::Float(fl_.raw.unwrap_or(fl_.w)));
        tooltips.push(Value::Str(title));
        rows.push(Value::Integer(i as i64));
        seriess.push(Value::Str(series));
        values.push(Value::Float(data_value));
        labels.push(Value::Str(flow_label));
        origs.push(Value::Str(fl_.o_name.clone()));
        dests.push(Value::Str(fl_.d_name.clone()));
        raw_sizes.push(fl_.raw.map(Value::Float).unwrap_or(Value::Na));
        fsizes.push(fsize.map(Value::Str).unwrap_or(Value::Na));
        linestyles.push(if linestyle.is_empty() {
            Value::Na
        } else {
            Value::Str(linestyle)
        });
        nds.push(if nd {
            Value::Str("1".to_string())
        } else {
            Value::Na
        });
        keys.push(Value::Str(key));

        end_pts.entry(fl_.ok.as_str()).or_insert(fl_.o);
        end_pts.entry(fl_.dk.as_str()).or_insert(fl_.d);
    }

    let cap_warn = (fl.total > FLOW_CAP).then(|| {
        format!(
            "map: {} flows, the {} widest drawn — aggregate in SQL",
            fl.total, FLOW_CAP
        )
    });
    if xs.is_empty() {
        return (String::new(), 0, extra_self, cap_warn);
    }
    let n = xs.len();

    let df: Vec<(String, Vec<Value>)> = vec![
        ("x".into(), xs),
        ("y".into(), ys),
        ("xend".into(), xends),
        ("yend".into(), yends),
        ("color".into(), colors),
        ("size_col".into(), sizes),
        ("tooltip_col".into(), tooltips),
        ("row".into(), rows),
        ("series".into(), seriess),
        ("value".into(), values),
        ("label".into(), labels),
        ("orig".into(), origs),
        ("dest".into(), dests),
        ("size".into(), raw_sizes),
        ("fsize".into(), fsizes),
        ("linestyle".into(), linestyles),
        ("nd".into(), nds),
        ("key".into(), keys),
    ];

    let aes = Aes::new()
        .x("x")
        .y("y")
        .xend("xend")
        .yend("yend")
        .color("color")
        .tooltip("tooltip_col")
        .data_attr("row", "row")
        .data_attr("series", "series")
        .data_attr("value", "value")
        .data_attr("label", "label")
        .data_attr("orig", "orig")
        .data_attr("dest", "dest")
        .data_attr("size", "size")
        .data_attr("fsize", "fsize")
        .data_attr("linestyle", "linestyle")
        .linetype("linestyle")
        .data_attr("nd", "nd")
        // `data-x` by default would be the row's `x` aesthetic (a numeric
        // longitude); this replaces it with the flow's "orig→dest" key, as
        // the old hand-written code did (and web/app.js's hover code wants).
        .data_attr("x", "key");

    // Width resolution mirrors `flows::project`'s three cases: a fixed
    // `::SIZE` constant, no `::SIZE` at all (flat 1.5px), or a varying
    // `::SIZE` sqrt-scaled 0.75..8 by `fl.vmax` — only the last one is a
    // ggplot-rs *scale* (so it can also drive the legend key widths in
    // `legend.rs`); the other two are a plain fixed `GeomCurve::linewidth`,
    // matching every row's identical pre-resolved `Flow::w`.
    let width_const = fl.fixed.is_some() || !fl.has_size;
    let fixed_w = fl.flows.first().map(|f| f.w).unwrap_or(1.5);
    let alpha = if m.alpha < 1.0 { m.alpha } else { 0.85 };

    let mut aes = aes;
    if !width_const {
        aes = aes.linewidth("size_col");
    }

    let mut curve = GeomCurve::default()
        .curvature(0.2)
        .lineend(LineEnd::Round)
        .arrow(
            arrow()
                .closed()
                .angle(26.565)
                .length(5.0)
                .length_per_linewidth(2.2)
                .position(0.85),
        )
        .alpha(alpha);
    if width_const {
        curve = curve.linewidth(fixed_w);
    }

    let mut plot = GGPlot::new(df)
        .aes(aes)
        .geom_curve_with(curve)
        .scale_colour_identity()
        // "flow"/"dashed" ::LINESTYLE rows dash — ggplot-rs's own `Dashed`
        // pattern (6,3), not the old hand-written `stroke-dasharray="6 4"`
        // (an accepted, documented visual delta).
        .scale_linetype_manual(vec![
            ("NA", Linetype::Solid),
            ("flow", Linetype::Dashed),
            ("dashed", Linetype::Dashed),
        ]);
    if !width_const {
        plot = plot.scale_linewidth(
            ScaleLinewidthContinuous::new()
                .with_range(0.75, 8.0)
                .with_transform(ScaleTransform::Sqrt)
                .with_limits(0.0, fl.vmax),
        );
    }

    // Ends: one dot per distinct end point, deduplicated by key — plain
    // filled dots, no stroke (GeomPoint has no stroke parameter at all) and
    // no hover title of their own (an all-NA `tip` column resets it).
    let (mut ex, mut ey, mut etip) = (Vec::new(), Vec::new(), Vec::new());
    for [x, y] in end_pts.values() {
        ex.push(Value::Float(*x));
        ey.push(Value::Float(*y));
        etip.push(Value::Na);
    }
    let ends_df: Vec<(String, Vec<Value>)> =
        vec![("x".into(), ex), ("y".into(), ey), ("tip".into(), etip)];
    let end_color = mix(t.colors.ink, card, 0.25);
    plot = plot
        .geom_point_with(GeomPoint {
            size: 2.5,
            color: end_color,
            alpha: 1.0,
        })
        .layer_data(ends_df)
        .layer_aes(Aes::new().x("x").y("y").tooltip("tip"));

    let [x0, x1, y0, y1] = frame.window(w, h);
    let frag = plot
        .coord_sf_with(CoordSf::new().xlim(x0, x1).ylim(y0, y1).expand(false))
        .theme_void()
        .legend_position(LegendPosition::None)
        .svg_root_data(false)
        .render_svg_native_at(0.0, 0.0, w.max(1.0) as u32, h.max(1.0) as u32)
        .unwrap_or_default();

    (frag, n, extra_self, cap_warn)
}

/// Draw the hexbin overlay and splice it into the map body. `hb` is already
/// binned (`hexbin::of`'s `StatBinHex::compute_group`, data prep only);
/// this only resolves each cell's colour (`fill_of`'s `Fill::Measure` scale,
/// the same choropleth colour-class logic — "hexbin colour classes stay in
/// the consumer for now") and draws. Returns the rendered SVG fragment
/// (empty when there are no occupied cells) and the number of hexagons
/// drawn (for the "aggregate in SQL" mark-budget warning in `mod.rs`).
pub(super) fn render_hexbin(
    o: &RenderOptions,
    hb: &Hexbin,
    scale: &ValueScale,
    frame: &Frame,
    card: Rgb,
    w: f64,
    h: f64,
) -> (String, usize) {
    if hb.cells.is_empty() {
        return (String::new(), 0);
    }
    let lattice = hexbin::lattice_of(hb);
    let n = hb.cells.len();

    let (mut xs, mut ys, mut widths, mut heights) = (
        Vec::with_capacity(n),
        Vec::with_capacity(n),
        Vec::with_capacity(n),
        Vec::with_capacity(n),
    );
    let mut fills = Vec::with_capacity(n);
    let mut rows = Vec::with_capacity(n);
    let mut keys = Vec::with_capacity(n);
    let mut values = Vec::with_capacity(n);
    let mut classes = Vec::with_capacity(n);
    let mut counts = Vec::with_capacity(n);
    let mut labels = Vec::with_capacity(n);
    let mut fvalues = Vec::with_capacity(n);
    let mut tooltips = Vec::with_capacity(n);

    for (i, ((q, r), cell)) in hb.cells.iter().enumerate() {
        let (cx, cy) = lattice.centre(*q as i64, *r as i64);
        let value = if hb.has_measure {
            cell.sum
        } else {
            cell.n as f64
        };
        let (color, class) = scale.color(value);
        let fvalue = scale.fmt.full(value, &o.locale);
        let label = hexbin::label_of(cell, hb.planar);
        let n_points = format!("point{}", if cell.n == 1 { "" } else { "s" });
        let title = if hb.has_measure {
            format!("{label}: {fvalue} ({} {n_points})", cell.n)
        } else {
            format!("{label}: {} {n_points}", cell.n)
        };

        xs.push(Value::Float(cx));
        ys.push(Value::Float(cy));
        widths.push(Value::Float(hb.width));
        heights.push(Value::Float(hb.width));
        fills.push(Value::Str(hex(color)));
        rows.push(Value::Integer(i as i64));
        // `data-x` by default would be the row's `x` aesthetic (a numeric
        // projected coordinate); this replaces it with the cell's "q,r"
        // key, as the old hand-written code did.
        keys.push(Value::Str(format!("{q},{r}")));
        values.push(Value::Float(value));
        classes.push(
            class
                .map(|k| Value::Integer((k + 1) as i64))
                .unwrap_or(Value::Na),
        );
        counts.push(Value::Integer(cell.n as i64));
        labels.push(Value::Str(label));
        fvalues.push(if hb.has_measure {
            Value::Na
        } else {
            Value::Str(fvalue)
        });
        tooltips.push(Value::Str(title));
    }

    let df: Vec<(String, Vec<Value>)> = vec![
        ("x".into(), xs),
        ("y".into(), ys),
        ("width".into(), widths),
        ("height".into(), heights),
        ("fill_color".into(), fills),
        ("row".into(), rows),
        ("key".into(), keys),
        ("value".into(), values),
        ("class".into(), classes),
        ("count".into(), counts),
        ("label".into(), labels),
        ("fvalue".into(), fvalues),
        ("tooltip_col".into(), tooltips),
    ];

    let aes = Aes::new()
        .x("x")
        .y("y")
        .fill("fill_color")
        .tooltip("tooltip_col")
        .data_attr("row", "row")
        .data_attr("value", "value")
        .data_attr("class", "class")
        .data_attr("count", "count")
        .data_attr("label", "label")
        // `data-fvalue` is a ggplot-rs-reserved `data_attr` name (0.21):
        // only `svg_formatted_values` may write it, and a count-based hex's
        // own "N points" text isn't that mechanism's per-scale
        // `format_value`. `data-vtext` carries it instead; `web/app.js`'s
        // `mapMarkInfo` reads either. Omitted when `hb.has_measure` (the
        // `fvalues` column is then all-`Na`): a summed measure gets its
        // `data-fvalue` from the whole-map `with_formatted_values` pass.
        .data_attr("vtext", "fvalue")
        .data_attr("x", "key");

    let [x0, x1, y0, y1] = frame.window(w, h);
    let frag = GGPlot::new(df)
        .aes(aes)
        .geom_hex_with(GeomHex {
            color: card,
            line_width: 0.4,
            alpha: 1.0,
        })
        .stat(StatIdentity)
        .scale_fill_identity()
        .coord_sf_with(CoordSf::new().xlim(x0, x1).ylim(y0, y1).expand(false))
        .theme_void()
        .legend_position(LegendPosition::None)
        .svg_root_data(false)
        .render_svg_native_at(0.0, 0.0, w.max(1.0) as u32, h.max(1.0) as u32)
        .unwrap_or_default();

    (frag, n)
}
