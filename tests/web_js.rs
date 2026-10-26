//! Runs the browser regression tests under Node: `tests/web/sanitize_test.mjs`
//! (escapeHtml / safeUrl / renderMarkdown from `web/app.js`) and
//! `tests/web/svg_attrs_test.mjs` (the readers of ggplot-rs SVG metadata).
//! Skipped (with a note) when `node` is not installed — except on CI, where
//! that fails (`common::skip_without`).
mod common;

fn run_node(file: &str) {
    let script = format!("{}/tests/web/{file}", env!("CARGO_MANIFEST_DIR"));
    let out = match std::process::Command::new("node").arg(&script).output() {
        Ok(o) => o,
        Err(_) => return common::skip_without("node", file),
    };
    assert!(
        out.status.success(),
        "{file} failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn web_html_sinks_are_inert() {
    run_node("sanitize_test.mjs");
}

/// #514: imageSrc() must classify every src exactly like Rust's
/// src/image.rs::src_ok (tests/fixtures/image_src_vectors.json).
#[test]
fn web_image_src_matches_the_rust_classifier() {
    run_node("image_src_test.mjs");
}

#[test]
fn web_reads_svg_metadata() {
    run_node("svg_attrs_test.mjs");
}

/// #368: a ggplot-rs 0.23.0 truncated legend label's <title> must not make
/// attachHover treat it as a hoverable/selectable/drillable data mark.
#[test]
fn web_legend_label_titles_are_not_marks() {
    run_node("legend_hover_test.mjs");
}

#[test]
fn web_contract_plot_glue() {
    run_node("contract_test.mjs");
}

/// Loads the fetched anofox wasm extensions in DuckDB-Wasm (Node); skips
/// itself unless `scripts/fetch-wasm-extensions.sh` and
/// `npm install --prefix tests/web` have been run.
#[test]
fn web_wasm_extensions_load() {
    run_node("wasm_extensions_test.mjs");
}

#[test]
fn web_locale_formatting_matches_the_cli() {
    run_node("locale_test.mjs");
}

#[test]
fn web_url_state_is_validated() {
    run_node("urlstate_test.mjs");
}

#[test]
fn web_chart_interactions() {
    run_node("interaction_test.mjs");
}

#[test]
fn web_period_control_matches_the_rust_vectors() {
    run_node("period_test.mjs");
}

#[test]
fn web_kpi_tile_parts() {
    run_node("kpi_test.mjs");
}

#[test]
fn web_freshness_chip_matches_the_cli() {
    run_node("asof_test.mjs");
}

#[test]
fn web_markdown_values_are_inert() {
    run_node("md_values_test.mjs");
}

#[test]
fn web_markdown_math() {
    run_node("math_test.mjs");
}

#[test]
fn web_diagram_text_alternative() {
    run_node("diagram_test.mjs");
}

#[test]
fn web_story_keys_match_marks() {
    run_node("anim_test.mjs");
}

/// M2f-A (#149): the ::FRAME player's pure colour helpers and its stepping
/// control flow (clamping, coalescing, Play, reduced motion, tab-hidden).
#[test]
fn web_frame_player() {
    run_node("frames_test.mjs");
}

#[test]
fn web_examples_library_keeps_old_links() {
    run_node("examples_lib_test.mjs");
}

#[test]
fn web_my_dashboards_store() {
    run_node("dash_store_test.mjs");
}

#[test]
fn web_app_bar_settings_are_merged_and_checked() {
    run_node("app_brand_test.mjs");
}

#[test]
fn web_option_labels() {
    run_node("option_label_test.mjs");
}

#[test]
fn web_sidebar_search_and_zip() {
    run_node("library_test.mjs");
}

#[test]
fn web_served_nav_groups() {
    run_node("served_nav_test.mjs");
}

/// #407 part 2: a served-nav link carries the current value of every
/// server-declared `pathVars` variable (servedNavHref), and only those.
#[test]
fn web_served_nav_keeps_path_vars() {
    run_node("nav_path_vars_test.mjs");
}

/// The builder assistant's core (#145): the loop with scripted model turns,
/// the classifier, masking, the cache prefix, transport (a) against a local
/// fake API and the tools in DuckDB-Wasm; and web/assist/guide.md is current.
#[test]
fn web_assistant_core() {
    run_node("assist_test.mjs");
}

/// ::VIEW / ::ENDVIEW deep-linking (#214): readViewSeed's seed validation,
/// setViewParam's default-key deletion, and the "view." / "v." prefix
/// non-collision.
#[test]
fn web_view_switcher_url_state() {
    run_node("views_test.mjs");
}

/// The exported standalone HTML's tab/sub-tab switcher (#381): each
/// `.tabbar`/`.subtabbar` wires up scoped to its own wrap's panes, so a
/// sub-tab click never indexes into an unrelated bar's panes.
#[test]
fn web_export_wires_each_tab_group_independently() {
    run_node("subtabs_export_test.mjs");
}

/// `-- @name` (#518 PR A): wasm `plan()` carries `Panel::name` through to
/// the browser, and `tagName` only sets `data-dp-name` once (a re-tag never
/// overwrites it).
#[test]
fn web_panel_names() {
    run_node("names_test.mjs");
}
