# Container image for a locked dashboard server (docs/deployment.md).
#   docker build -f deploy/Dockerfile -t anofox-dashboards .
#   docker run --rm -p 127.0.0.1:8080:8080 --read-only -v /tmp \
#       -v $PWD/dash:/etc/anofox/dashboards:ro -v $PWD/serve.sql:/etc/anofox/serve.sql:ro \
#       -v $PWD/data:/data:ro anofox-dashboards
# The image holds the DuckDB CLI and the native extension; the dashboards, the
# SERVE_SQL and the data are mounted. In a container the server must bind
# 0.0.0.0 ("bind" in the options) — publish the port on the host's loopback
# (-p 127.0.0.1:8080:8080) and put the TLS proxy in front.
ARG DUCKDB_VERSION=1.5.6
# Must match the `wasm-bindgen` crate pinned in Cargo.lock — build-native.sh
# checks this itself and fails loudly on a mismatch (#552).
ARG WASM_BINDGEN_VERSION=0.2.129
ARG BINARYEN_VERSION=version_116

FROM rust:1-bookworm AS build
ARG DUCKDB_VERSION
ARG WASM_BINDGEN_VERSION
ARG BINARYEN_VERSION
# build-native.sh builds web/pkg itself (#552: no wasm-pack, which doesn't
# work on every box — cargo to wasm32-unknown-unknown + wasm-bindgen +
# wasm-opt instead, see that script's comments), so this installs the two
# CLIs it needs rather than wasm-pack.
RUN apt-get update && apt-get install -y --no-install-recommends clang libclang-dev unzip python3 && rm -rf /var/lib/apt/lists/* \
 && rustup target add wasm32-unknown-unknown \
 && curl -sSfL "https://github.com/rustwasm/wasm-bindgen/releases/download/${WASM_BINDGEN_VERSION}/wasm-bindgen-${WASM_BINDGEN_VERSION}-x86_64-unknown-linux-musl.tar.gz" \
    | tar -xz --strip-components=1 -C /usr/local/bin "wasm-bindgen-${WASM_BINDGEN_VERSION}-x86_64-unknown-linux-musl/wasm-bindgen" \
 && curl -sSfL "https://github.com/WebAssembly/binaryen/releases/download/${BINARYEN_VERSION}/binaryen-${BINARYEN_VERSION}-x86_64-linux.tar.gz" \
    | tar -xz --strip-components=2 -C /usr/local/bin "binaryen-${BINARYEN_VERSION}/bin/wasm-opt" \
 && curl -sSfL -o /tmp/duckdb.zip https://github.com/duckdb/duckdb/releases/download/v${DUCKDB_VERSION}/duckdb_cli-linux-amd64.zip \
 && unzip -d /usr/local/bin /tmp/duckdb.zip && rm /tmp/duckdb.zip
WORKDIR /src
COPY . .
RUN mkdir -p /out && DUCKDB=/usr/local/bin/duckdb OUT=/out/anofox_visualization.duckdb_extension duckext/scripts/build-native.sh --release

FROM debian:bookworm-slim
# tini as PID 1: the kernel ignores SIGTERM for a PID 1 without a handler, so
# without it `docker stop` would wait 10 s and kill.
RUN apt-get update && apt-get install -y --no-install-recommends tini && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --home-dir /nonexistent --shell /usr/sbin/nologin anofox
COPY --from=build /usr/local/bin/duckdb /opt/anofox/duckdb
COPY --from=build /out/anofox_visualization.duckdb_extension /opt/anofox/
COPY deploy/run-locked.sh /opt/anofox/run-locked.sh
ENV DUCKDB=/opt/anofox/duckdb HOME=/tmp
USER anofox
EXPOSE 8080
ENTRYPOINT ["/usr/bin/tini", "--", "/opt/anofox/run-locked.sh"]
CMD ["/etc/anofox/serve.sql"]
