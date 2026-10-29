#!/usr/bin/env bash
# Build roostr.space - the app compiled for the browser, plus the site's
# static files - into gpui/web-dist/, to be served at the site root.
#
#   scripts/build-web.sh            # optimized (the `web` profile)
#   scripts/build-web.sh --debug    # unoptimized, faster to build
#
# Needs the wasm32-unknown-unknown target. wasm-bindgen's CLI must be the
# version Cargo.lock pins for the `wasm-bindgen` crate: a matching one on PATH
# is used, else it is installed once into target/tools/. No wasm-opt: on top
# of the `web` profile, `-Os` made the module 9% smaller raw but 1.5% larger
# gzipped and 3% larger brotli'd, which is what a visitor downloads.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT=$(pwd)
OUT="$ROOT/web-dist"
STATIC="$ROOT/../static"

PROFILE=web
TARGET_DIR=web
if [[ "${1:-}" == "--debug" ]]; then
  PROFILE=dev
  TARGET_DIR=debug
fi

sha256() {
  if command -v sha256sum >/dev/null; then
    sha256sum | cut -d' ' -f1
  else
    shasum -a 256 | cut -d' ' -f1
  fi
}

# The shared Odin engine (glonOdin build-wasm.mjs) must match its manifest:
# the tab refuses a module whose sha256 differs, so a stale pair fails here.
MANIFEST="$STATIC/engine-core.json"
field() {
  sed -n "s/^  \"$1\": \"\{0,1\}\([^\",]*\)\"\{0,1\},\{0,1\}$/\1/p" "$MANIFEST"
}
regenerate="Regenerate static/engine.wasm + static/engine-core.json with glonOdin's build-wasm.mjs (see README)."
if [[ "$(field abiVersion)" != 2 || "$(field target)" != js_wasm32 || "$(field artifact)" != engine.wasm ]]; then
  echo "Shared core manifest ABI/target mismatch. $regenerate" >&2
  exit 1
fi
if [[ "$(sha256 <"$STATIC/engine.wasm")" != "$(field sha256)" ]]; then
  echo "Shared core WASM fingerprint mismatch. $regenerate" >&2
  exit 1
fi
# sourceFingerprint is the sha256 of JSON.stringify(sourceFiles).
SOURCES=$(awk '/^  "sourceFiles": \{$/ { on = 1; next }
  on && /^  \}/ { exit }
  on { gsub(/^ +|,$/, ""); sub(/": "/, "\":\""); printf "%s%s", (n++ ? "," : ""), $0 }' "$MANIFEST")
if [[ "$(printf '{%s}' "$SOURCES" | sha256)" != "$(field sourceFingerprint)" ]]; then
  echo "Invalid shared core source fingerprint. $regenerate" >&2
  exit 1
fi
echo "Verified shared Odin core $(field sourceFingerprint)"

# The stack size for the wasm link comes from .cargo/config.toml.
cargo build -p roostr-web --target wasm32-unknown-unknown --profile "$PROFILE"
WASM="$ROOT/target/wasm32-unknown-unknown/$TARGET_DIR/roostr_web.wasm"

VERSION=$(awk '/^name = "wasm-bindgen"$/ { getline; gsub(/version = |"/, ""); print; exit }' Cargo.lock)
if [[ -z "$VERSION" ]]; then
  echo "wasm-bindgen is not in Cargo.lock" >&2
  exit 1
fi
BINDGEN=wasm-bindgen
if [[ "$(wasm-bindgen --version 2>/dev/null | awk '{print $2}')" != "$VERSION" ]]; then
  TOOLS="$ROOT/target/tools/wasm-bindgen-$VERSION"
  BINDGEN="$TOOLS/bin/wasm-bindgen"
  if [[ ! -x "$BINDGEN" ]]; then
    cargo install --locked --root "$TOOLS" wasm-bindgen-cli --version "$VERSION"
  fi
fi

rm -rf "$OUT"
mkdir -p "$OUT"
"$BINDGEN" "$WASM" --out-dir "$OUT" --target web --no-typescript

# One page per website route, with the Svelte site's title and link-preview
# tags; 200.html answers every other path (the workspace, /app/**).
TEMPLATE=crates/roostr-web/www/index.html
page() {
  PAGE_HEAD=$2 awk '/<!-- page head/ { print ENVIRON["PAGE_HEAD"]; next } { print }' "$TEMPLATE" >"$OUT/$1"
}
page index.html '    <meta property="og:image" content="/logo.png" />
    <meta name="description" content="Roostr is a workspace where people and AI agents run things together. Each space is one part of your life, with its own people, records, logins and agents that work on a schedule on your own computers. End-to-end encrypted, open source." />
    <title>Roostr — your team, human and not</title>'
page guide.html '    <meta name="description" content="How Roostr works: objects, spaces, agents, repeating work, scores and logins - with best practices and step-by-step setups." />
    <title>Guide — Roostr</title>'
page privacy.html '    <meta name="description" content="What Roostr stores, where it goes, and who can see it." />
    <title>Privacy — Roostr</title>'
page j.html '    <meta name="robots" content="noindex" />
    <title>Join a space — Roostr</title>'
page 200.html '    <title>Roostr</title>'
cp crates/roostr-web/www/main.js "$OUT/"

# The replica's engine (its sha256 is checked against engine-core.json again
# when the tab loads it), and the files the pages and the manifest name.
cp "$STATIC"/{engine.wasm,engine-core.json,favicon.png,logo.png,manifest.json,robots.txt} "$OUT/"
# The color emoji font, fetched after the app starts (theme::EMOJI_FONT_PATH).
mkdir -p "$OUT/fonts"
cp crates/roostr-core/fonts/TwemojiMozilla.ttf crates/roostr-core/fonts/TwemojiMozilla-LICENSE.md "$OUT/fonts/"

for FILE in roostr_web_bg.wasm roostr_web.js engine.wasm fonts/TwemojiMozilla.ttf; do
  RAW=$(wc -c <"$OUT/$FILE" | tr -d ' ')
  GZ=$(gzip -9 -c "$OUT/$FILE" | wc -c | tr -d ' ')
  echo "web-dist/$FILE: $RAW bytes ($GZ gzipped)"
done
