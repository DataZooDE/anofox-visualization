PROJ_DIR := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))

EXT_NAME=anofox_visualization
EXT_CONFIG=${PROJ_DIR}extension_config.cmake

# Build tooling from extension-ci-tools (submodule)
include extension-ci-tools/makefiles/duckdb_extension.Makefile

.PHONY: rust_release rust_debug rust_test
rust_release:
	cargo build --manifest-path crates/anofox-viz-ffi/Cargo.toml --release
rust_debug:
	cargo build --manifest-path crates/anofox-viz-ffi/Cargo.toml
