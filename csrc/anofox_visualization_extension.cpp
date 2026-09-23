#include "anofox_visualization_extension.hpp"
#include "anofox_viz_ffi.h"
#include "anofox_visualization_banner.hpp"
#include "duckdb.hpp"
#include "duckdb/main/extension/extension_loader.hpp"
#include "duckdb/catalog/default/default_functions.hpp"
#include "duckdb/parser/parsed_data/create_scalar_function_info.hpp"
#include <string>

// The build stamps EXT_VERSION_ANOFOX_VISUALIZATION from the git tag; the
// fallback keeps the banner honest in local builds that do not, and matches
// what AnofoxVisualizationExtension::Version() reports below.
#ifdef EXT_VERSION_ANOFOX_VISUALIZATION
#define ANOFOX_VISUALIZATION_BANNER_VERSION EXT_VERSION_ANOFOX_VISUALIZATION
#else
#define ANOFOX_VISUALIZATION_BANNER_VERSION "0.1.0"
#endif

// Deliberately outside namespace duckdb: the banner library is DuckDB-agnostic
// and the guard macro refers to this object from every guarded source file.
const datazoo::BannerInfo ANOFOX_VISUALIZATION_BANNER {
    "anofox_visualization", ANOFOX_VISUALIZATION_BANNER_VERSION,
    "https://github.com/DataZooDE/anofox-visualization"};

namespace duckdb {

// anofox_render(spec VARCHAR) -> VARCHAR (SVG). Delegates to the Rust FFI.
static void AnofoxRenderFunction(DataChunk &args, ExpressionState &state, Vector &result) {
	UnaryExecutor::Execute<string_t, string_t>(args.data[0], result, args.size(), [&](string_t spec) {
		char *svg = anofox_viz_render(spec.GetString().c_str());
		std::string out = svg ? std::string(svg) : std::string();
		if (svg) {
			anofox_viz_free(svg);
		}
		return StringVector::AddString(result, out);
	});
}

// SQL sugar: anofox_bar/_line/_scatter/_area/_xy/_xyc — build a spec + call anofox_render.
//
// Registered as DefaultMacros rather than by executing "CREATE OR REPLACE MACRO ..."
// against a connection. Two reasons:
//
//   1. A macro created by running SQL cannot carry documentation. CREATE MACRO has no
//      syntax for a description, and COMMENT ON MACRO populates
//      duckdb_functions().comment, which is a different column from .description --
//      so these six were invisible to any agent reading the catalog. CreateMacroInfo
//      derives from CreateFunctionInfo, so this path can carry the metadata.
//   2. Executing CREATE MACRO wrote them into whatever database the user happened to
//      have open, persisting extension-owned definitions into their file. Registering
//      them as internal catalog entries is what DuckDB's own json extension does.
struct AnofoxMacro {
	DefaultMacro macro;
	const char *description;
	const char *example;
};

static const AnofoxMacro ANOFOX_MACROS[] = {
    {{DEFAULT_SCHEMA,
      "anofox_xy",
      {"x", "y", nullptr},
      {{"kind", "'BARCHART'"}, {"width", "640"}, {"height", "400"}, {nullptr, nullptr}},
      "anofox_render(json_object('rows', to_json(list({c0: x, c1: y})), "
      "'roles', ('[[0,\"XAXIS\"],[1,\"' || kind || '\"]]')::JSON, 'width', width, 'height', height))"},
     "Aggregate two columns into a single-series chart and render it as SVG. 'kind' selects the mark "
     "(BARCHART, LINECHART, SCATTER, AREACHART); x becomes the axis and y the value.",
     "SELECT anofox_xy(x, y, kind := 'LINECHART') FROM (VALUES ('Jan', 10), ('Feb', 20)) t(x, y)"},
    {{DEFAULT_SCHEMA,
      "anofox_xyc",
      {"x", "y", "series", nullptr},
      {{"kind", "'BARCHART_STACKED'"}, {"width", "640"}, {"height", "400"}, {nullptr, nullptr}},
      "anofox_render(json_object('rows', to_json(list({c0: x, c1: y, c2: series})), "
      "'roles', ('[[0,\"XAXIS\"],[1,\"' || kind || '\"],[2,\"CATEGORY\"]]')::JSON, 'width', width, 'height', "
      "height))"},
     "Aggregate three columns into a multi-series chart and render it as SVG, with 'series' splitting the "
     "data into categories.",
     "SELECT anofox_xyc(x, y, s) FROM (VALUES ('Jan', 10, 'EU'), ('Jan', 8, 'US')) t(x, y, s)"},
    {{DEFAULT_SCHEMA, "anofox_bar", {"x", "y", nullptr}, {{nullptr, nullptr}},
      "anofox_xy(x, y, kind := 'BARCHART')"},
     "Render a bar chart of two columns as SVG. Shorthand for anofox_xy(x, y, kind := 'BARCHART').",
     "SELECT anofox_bar(x, y) FROM (VALUES ('Jan', 10), ('Feb', 20)) t(x, y)"},
    {{DEFAULT_SCHEMA, "anofox_line", {"x", "y", nullptr}, {{nullptr, nullptr}},
      "anofox_xy(x, y, kind := 'LINECHART')"},
     "Render a line chart of two columns as SVG. Shorthand for anofox_xy(x, y, kind := 'LINECHART').",
     "SELECT anofox_line(x, y) FROM (VALUES ('Jan', 10), ('Feb', 20)) t(x, y)"},
    {{DEFAULT_SCHEMA, "anofox_scatter", {"x", "y", nullptr}, {{nullptr, nullptr}},
      "anofox_xy(x, y, kind := 'SCATTER')"},
     "Render a scatter plot of two columns as SVG. Shorthand for anofox_xy(x, y, kind := 'SCATTER').",
     "SELECT anofox_scatter(x, y) FROM (VALUES (1.5, 10), (2.5, 20)) t(x, y)"},
    {{DEFAULT_SCHEMA, "anofox_area", {"x", "y", nullptr}, {{nullptr, nullptr}},
      "anofox_xy(x, y, kind := 'AREACHART')"},
     "Render an area chart of two columns as SVG. Shorthand for anofox_xy(x, y, kind := 'AREACHART').",
     "SELECT anofox_area(x, y) FROM (VALUES ('Jan', 10), ('Feb', 20)) t(x, y)"},
};

void LoadInternal(ExtensionLoader &loader) {
	// Guarded: anofox_render is the single user-facing entry point. Every
	// anofox_bar/_line/_scatter/_area/_xy/_xyc macro below expands to a call to
	// it, so one guard puts the issue link on a failure from any of them.
	ScalarFunction render("anofox_render", {LogicalType::VARCHAR}, LogicalType::VARCHAR,
	                      DATAZOO_GUARD(ANOFOX_VISUALIZATION_BANNER, AnofoxRenderFunction));
	{
		CreateScalarFunctionInfo info(std::move(render));
		FunctionDescription desc;
		desc.description =
		    "Render a chart specification to an SVG string. The spec is JSON carrying 'rows' (the data), "
		    "'roles' (which column plays which part: XAXIS, BARCHART, CATEGORY, ...) and optional 'width' "
		    "and 'height'. The anofox_bar/_line/_scatter/_area/_xy/_xyc macros build this spec for you.";
		desc.parameter_names = {"spec"};
		desc.parameter_types = {LogicalType::VARCHAR};
		desc.examples = {"anofox_render(json_object('rows', to_json([{c0: 'Jan', c1: 10}]), "
		                 "'roles', '[[0,\"XAXIS\"],[1,\"BARCHART\"]]'::JSON))"};
		desc.categories = {"visualization"};
		info.descriptions.push_back(std::move(desc));
		loader.RegisterFunction(std::move(info));
	}

	for (auto &entry : ANOFOX_MACROS) {
		auto info = DefaultFunctionGenerator::CreateInternalMacroInfo(entry.macro);
		FunctionDescription desc;
		desc.description = entry.description;
		desc.examples = {entry.example};
		desc.categories = {"visualization"};
		// parameter_names is deliberately unset: a macro already reports its real
		// parameter names, and a non-empty parameter_names would replace the whole list.
		info->descriptions.push_back(std::move(desc));
		loader.RegisterFunction(*info);
	}

	datazoo::RegisterBannerOption(loader);
	// Last, so a load that fails earlier never advertises itself. Silent unless
	// stderr is a terminal and the ~/.duckdb stamp is over a day old.
	datazoo::ShowBanner(ANOFOX_VISUALIZATION_BANNER);
}

void AnofoxVisualizationExtension::Load(ExtensionLoader &loader) {
	LoadInternal(loader);
}
std::string AnofoxVisualizationExtension::Name() {
	return "anofox_visualization";
}
std::string AnofoxVisualizationExtension::Version() const {
#ifdef EXT_VERSION_ANOFOX_VISUALIZATION
	return EXT_VERSION_ANOFOX_VISUALIZATION;
#else
	return "0.1.0";
#endif
}

} // namespace duckdb

extern "C" {
DUCKDB_CPP_EXTENSION_ENTRY(anofox_visualization, loader) {
	duckdb::LoadInternal(loader);
}
DUCKDB_EXTENSION_API const char *anofox_visualization_version() {
	return duckdb::DuckDB::LibraryVersion();
}
}
