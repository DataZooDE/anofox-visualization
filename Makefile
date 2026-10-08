PROJ_DIR := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

EXT_NAME=anofox_visualization
EXT_CONFIG=${PROJ_DIR}extension_config.cmake

# Build tooling from extension-ci-tools (submodule). `make` / `make test` build
# and test the render-only C++ extension (the community binary).
include extension-ci-tools/makefiles/duckdb_extension.Makefile

.PHONY: rust_release rust_debug rust_test native
rust_release:
	cargo build --locked -p anofox_viz_ffi --release
rust_debug:
	cargo build --locked -p anofox_viz_ffi
rust_test:
	cargo test --locked --features serve
	cargo test --locked -p anofox_viz_ffi -p anofox-visualization-duckdb

# The full-toolkit C-API build (rendering + anofox_serve*); see BUILD.md.
native:
	duckext/scripts/build-native.sh --release
