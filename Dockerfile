# syntax=docker/dockerfile:1
# roostr.space - static files only. The app is the Rust/GPUI workspace in
# gpui/ compiled to wasm: a serverless browser replica (or a client of the
# visitor's own daemon). This image builds it and serves the result with an
# SPA fallback.
#
# Nightly: on wasm, gpui-pre-web builds parking_lot with its `nightly` feature.
FROM rust:1-bookworm AS build
ARG RUST_TOOLCHAIN=nightly-2026-10-24
RUN rustup toolchain install "$RUST_TOOLCHAIN" --profile minimal --target wasm32-unknown-unknown \
	&& rustup default "$RUST_TOOLCHAIN"
# brotli for the precompressed copies Caddy serves.
RUN apt-get update && apt-get install -y --no-install-recommends brotli && rm -rf /var/lib/apt/lists/*
WORKDIR /src
# The wasm-bindgen CLI the lockfile pins, in a layer of its own: rebuilt only
# when Cargo.lock changes.
COPY gpui/Cargo.lock gpui/Cargo.lock
RUN cargo install --locked wasm-bindgen-cli --version \
	"$(awk '/^name = "wasm-bindgen"$/ { getline; gsub(/version = |"/, ""); print; exit }' gpui/Cargo.lock)"
# The crates embed fonts, images and dictionaries from static/, which also
# holds the engine and the files the pages link.
COPY static ./static
COPY gpui ./gpui
# Crates and compiled dependencies persist between builds of this builder.
# build-web.sh checks engine.wasm against engine-core.json first.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
	--mount=type=cache,target=/usr/local/cargo/git \
	--mount=type=cache,target=/src/gpui/target \
	gpui/scripts/build-web.sh \
	&& find gpui/web-dist -type f \( -name '*.wasm' -o -name '*.js' -o -name '*.html' -o -name '*.json' -o -name '*.ttf' \) \
		-exec gzip -9 -k {} \; -exec brotli -q 11 {} \;

FROM caddy:2-alpine
COPY --from=build /src/gpui/web-dist /srv
COPY Caddyfile /etc/caddy/Caddyfile
