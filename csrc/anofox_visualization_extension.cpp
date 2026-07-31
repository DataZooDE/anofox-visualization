#include "anofox_visualization_extension.hpp"
#include "anofox_viz_ffi.h"
#include "duckdb.hpp"
#include "duckdb/main/extension/extension_loader.hpp"
#include <string>

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
static const char *MACROS[] = {
	"CREATE OR REPLACE MACRO anofox_xy(x, y, kind := 'BARCHART', width := 640, height := 400) AS "
	"anofox_render(json_object('rows', to_json(list({c0: x, c1: y})), "
	"'roles', ('[[0,\"XAXIS\"],[1,\"' || kind || '\"]]')::JSON, 'width', width, 'height', height))",
	"CREATE OR REPLACE MACRO anofox_xyc(x, y, series, kind := 'BARCHART_STACKED', width := 640, height := 400) AS "
	"anofox_render(json_object('rows', to_json(list({c0: x, c1: y, c2: series})), "
	"'roles', ('[[0,\"XAXIS\"],[1,\"' || kind || '\"],[2,\"CATEGORY\"]]')::JSON, 'width', width, 'height', height))",
	"CREATE OR REPLACE MACRO anofox_bar(x, y) AS anofox_xy(x, y, kind := 'BARCHART')",
	"CREATE OR REPLACE MACRO anofox_line(x, y) AS anofox_xy(x, y, kind := 'LINECHART')",
	"CREATE OR REPLACE MACRO anofox_scatter(x, y) AS anofox_xy(x, y, kind := 'SCATTER')",
	"CREATE OR REPLACE MACRO anofox_area(x, y) AS anofox_xy(x, y, kind := 'AREACHART')",
};

void LoadInternal(ExtensionLoader &loader) {
	ScalarFunction render("anofox_render", {LogicalType::VARCHAR}, LogicalType::VARCHAR, AnofoxRenderFunction);
	loader.RegisterFunction(render);

	Connection con(loader.GetDatabaseInstance());
	con.BeginTransaction();
	for (auto sql : MACROS) {
		con.Query(sql);
	}
	con.Commit();
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
