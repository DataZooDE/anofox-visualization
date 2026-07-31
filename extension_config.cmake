# Included by DuckDB's build system; declares which extension to load and links
# the Rust FFI static archive (the -static target that corrosion imports).
duckdb_extension_load(anofox_visualization
    SOURCE_DIR ${CMAKE_CURRENT_LIST_DIR}
    LOAD_TESTS
    LINKED_LIBS "$<TARGET_FILE:anofox_viz_ffi-static>"
)
