#include "anofox_visualization_extension.hpp"
#include "anofox_viz_ffi.h"
#include "anofox_visualization_banner.hpp"
#include "duckdb.hpp"
#include "duckdb/main/extension/extension_loader.hpp"
#include "duckdb/catalog/default/default_functions.hpp"
#include "duckdb/parser/parsed_data/create_scalar_function_info.hpp"
#include "duckdb/parser/parsed_data/create_macro_info.hpp"
#include "duckdb/parser/parser.hpp"
#include "duckdb/parser/expression/columnref_expression.hpp"
#include "duckdb/function/scalar_macro_function.hpp"
#include <string>

// The build stamps EXT_VERSION_ANOFOX_VISUALIZATION from the git tag; the
// fallback keeps the banner honest in local builds that do not, and matches
// what AnofoxVisualizationExtension::Version() reports below.
// ANOFOX_VIZ_CARGO_VERSION is injected by CMakeLists.txt from the workspace
// Cargo.toml — the single version source of truth.
#ifndef ANOFOX_VIZ_CARGO_VERSION
#define ANOFOX_VIZ_CARGO_VERSION "unknown"
#endif
#ifdef EXT_VERSION_ANOFOX_VISUALIZATION
#define ANOFOX_VISUALIZATION_BANNER_VERSION EXT_VERSION_ANOFOX_VISUALIZATION
#else
#define ANOFOX_VISUALIZATION_BANNER_VERSION ANOFOX_VIZ_CARGO_VERSION
#endif

// Deliberately outside namespace duckdb: the banner library is DuckDB-agnostic
// and the guard macro refers to this object from every guarded source file.
const datazoo::BannerInfo ANOFOX_VISUALIZATION_BANNER {
    "anofox_visualization", ANOFOX_VISUALIZATION_BANNER_VERSION,
    "https://github.com/DataZooDE/anofox-visualization"};

namespace duckdb {

// anofox_render(spec VARCHAR) -> VARCHAR (SVG). Delegates to the Rust FFI.
// NULL in -> NULL out (UnaryExecutor skips invalid rows). A bad spec, an
// oversize spec (rows / width / height caps) or an internal renderer failure is
// a real SQL error, not an error string disguised as a result.
static void AnofoxRenderFunction(DataChunk &args, ExpressionState &state, Vector &result) {
	UnaryExecutor::Execute<string_t, string_t>(args.data[0], result, args.size(), [&](string_t spec) {
		char *out = nullptr;
		int rc = anofox_viz_render(spec.GetData(), spec.GetSize(), &out);
		std::string text = out ? std::string(out) : std::string("anofox_render: out of memory");
		if (out) {
			anofox_viz_free(out);
		}
		if (rc != ANOFOX_VIZ_OK) {
			throw InvalidInputException(text);
		}
		return StringVector::AddString(result, text);
	});
}

// The bundled SQL macros (anofox_bar/_line/_xy/…, anofox_plot_*): the table
// lives in the Rust core (src/macros.rs) and is read through the FFI, so the
// C-API build (duckext/, CREATE MACRO) and this build share byte-identical
// bodies.
//
// Registered as internal catalog entries rather than by executing "CREATE OR
// REPLACE MACRO ..." against a connection. Two reasons:
//
//   1. A macro created by running SQL cannot carry documentation. CREATE MACRO has no
//      syntax for a description, and COMMENT ON MACRO populates
//      duckdb_functions().comment, which is a different column from .description --
//      so they were invisible to any agent reading the catalog. CreateMacroInfo
//      derives from CreateFunctionInfo, so this path can carry the metadata (and tags).
//   2. Executing CREATE MACRO wrote them into whatever database the user happened to
//      have open, persisting extension-owned definitions into their file. Registering
//      them as internal catalog entries is what DuckDB's own json extension does.
//
// Built like DefaultFunctionGenerator::CreateInternalMacroInfo, without its
// fixed-size parameter arrays.
static unique_ptr<CreateMacroInfo> AnofoxMacroInfo(idx_t i) {
	auto field = [&](int f) {
		const char *p = anofox_viz_macro_field(i, f);
		return std::string(p ? p : "");
	};
	auto expressions = Parser::ParseExpressionList(field(1));
	if (expressions.size() != 1) {
		throw InternalException("anofox_visualization: macro %s must be one expression", field(0));
	}
	auto function = make_uniq<ScalarMacroFunction>(std::move(expressions[0]));
	const char *second = nullptr;
	for (size_t j = 0; const char *p = anofox_viz_macro_item(i, 0, j, &second); j++) {
		function->parameters.push_back(make_uniq<ColumnRefExpression>(p));
	}
	for (size_t j = 0; const char *p = anofox_viz_macro_item(i, 1, j, &second); j++) {
		auto defaults = Parser::ParseExpressionList(second ? second : "NULL");
		if (defaults.size() != 1) {
			throw InternalException("anofox_visualization: bad default for %s", std::string(p));
		}
		function->parameters.push_back(make_uniq<ColumnRefExpression>(p));
		function->default_parameters.insert(make_pair(std::string(p), std::move(defaults[0])));
	}
	auto info = make_uniq<CreateMacroInfo>(CatalogType::MACRO_ENTRY);
	info->macros.push_back(std::move(function));
	info->schema = DEFAULT_SCHEMA;
	info->name = field(0);
	info->temporary = true;
	info->internal = true;
	for (size_t j = 0; const char *k = anofox_viz_macro_item(i, 2, j, &second); j++) {
		info->tags[k] = second ? second : "";
	}
	FunctionDescription desc;
	desc.description = field(2);
	desc.examples = {field(3)};
	desc.categories = {"visualization"};
	// parameter_names is deliberately unset: a macro already reports its real
	// parameter names, and a non-empty parameter_names would replace the whole list.
	info->descriptions.push_back(std::move(desc));
	return info;
}

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
		    "and 'height' -- or 'plot' (terms, prediction, curve, summary, obs, diagnostics, auto) with rows "
		    "named by the anofox contract. The anofox_bar/_xy/... and anofox_plot_* macros build this spec for you.";
		desc.parameter_names = {"spec"};
		desc.parameter_types = {LogicalType::VARCHAR};
		desc.examples = {"anofox_render(json_object('rows', to_json([{c0: 'Jan', c1: 10}]), "
		                 "'roles', '[[0,\"XAXIS\"],[1,\"BARCHART\"]]'::JSON))"};
		desc.categories = {"visualization"};
		info.descriptions.push_back(std::move(desc));
		loader.RegisterFunction(std::move(info));
	}

	for (idx_t i = 0; i < anofox_viz_macro_count(); i++) {
		auto info = AnofoxMacroInfo(i);
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
	return ANOFOX_VIZ_CARGO_VERSION;
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
