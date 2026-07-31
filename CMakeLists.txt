cmake_minimum_required(VERSION 3.20)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

# --- Rust target detection (propagate arch to the cargo build in CI) ---
execute_process(COMMAND rustup target list --installed OUTPUT_VARIABLE RUST_TARGETS ERROR_QUIET)
set(Rust_CARGO_TARGET "")
if("${OS_NAME}" STREQUAL "linux")
    if("${OS_ARCH}" STREQUAL "arm64")
        set(Rust_CARGO_TARGET "aarch64-unknown-linux-gnu")
    elseif("${CMAKE_CXX_COMPILER}" MATCHES "aarch64")
        set(Rust_CARGO_TARGET "aarch64-unknown-linux-gnu")
    else()
        string(FIND "${RUST_TARGETS}" "musl" MUSL_TARGET_FOUND)
        if(NOT MUSL_TARGET_FOUND EQUAL -1)
            set(Rust_CARGO_TARGET "x86_64-unknown-linux-musl")
        else()
            set(Rust_CARGO_TARGET "x86_64-unknown-linux-gnu")
        endif()
    endif()
elseif("${OS_NAME}" STREQUAL "osx")
    if("${OSX_BUILD_ARCH}" STREQUAL "arm64")
        set(Rust_CARGO_TARGET "aarch64-apple-darwin")
    elseif("${OSX_BUILD_ARCH}" STREQUAL "x86_64")
        set(Rust_CARGO_TARGET "x86_64-apple-darwin")
    elseif("${OS_ARCH}" STREQUAL "arm64")
        set(Rust_CARGO_TARGET "aarch64-apple-darwin")
    endif()
endif()
message(STATUS "Rust_CARGO_TARGET: ${Rust_CARGO_TARGET}")

# --- Corrosion: build the Rust FFI staticlib and expose it as a CMake target ---
include(FetchContent)
FetchContent_Declare(Corrosion GIT_REPOSITORY https://github.com/corrosion-rs/corrosion.git GIT_TAG v0.5.2)
FetchContent_MakeAvailable(Corrosion)
corrosion_import_crate(
    MANIFEST_PATH "${CMAKE_CURRENT_SOURCE_DIR}/crates/anofox-viz-ffi/Cargo.toml"
    CRATES "anofox_viz_ffi"
)

set(TARGET_NAME anofox_visualization)
set(EXTENSION_NAME ${TARGET_NAME}_extension)
set(LOADABLE_EXTENSION_NAME ${TARGET_NAME}_loadable_extension)
project(${TARGET_NAME})

include_directories(src/include)
set(EXTENSION_SOURCES csrc/anofox_visualization_extension.cpp)

build_static_extension(${TARGET_NAME} ${EXTENSION_SOURCES})
build_loadable_extension(${TARGET_NAME} " " ${EXTENSION_SOURCES})

# Link the Rust FFI static archive (corrosion registers `<crate>-static`).
target_link_libraries(${EXTENSION_NAME} anofox_viz_ffi-static)
target_link_libraries(${LOADABLE_EXTENSION_NAME} anofox_viz_ffi-static)

if(WIN32)
    # bcrypt: required by Rust's getrandom crate
    target_link_libraries(${EXTENSION_NAME} bcrypt)
    target_link_libraries(${LOADABLE_EXTENSION_NAME} bcrypt)
endif()

install(
    TARGETS ${EXTENSION_NAME}
    EXPORT "${DUCKDB_EXPORT_SET}"
    LIBRARY DESTINATION "${INSTALL_LIB_DIR}"
    ARCHIVE DESTINATION "${INSTALL_LIB_DIR}"
)
