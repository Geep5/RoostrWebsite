# Roostr

Roostr's app and website, in Rust on [GPUI](https://gpui-kit.com). One
workspace (`gpui/`) builds the desktop app and, compiled to WebAssembly, all of
roostr.space: the landing (`/`), the guide (`/guide`), the privacy policy
(`/privacy`), the space join page (`/j#r2.<blob>`) and the workspace
(`/app`, `/app/object/<id>`, `/app/graph`, `/app/chat/<id>`).

| Path | What |
|---|---|
| `gpui/` | The Cargo workspace - crates, `scripts/build-web.sh`, and its own [README](gpui/README.md) (modes, crates, development) |
| `static/` | The shared Odin engine (`engine.wasm` + `engine-core.json`), the files the site's pages link (`favicon.png`, `logo.png`, `manifest.json`, `robots.txt`), and fonts, images and spelling dictionaries the crates embed |
| `Dockerfile`, `Caddyfile`, `fly.toml` | The roostr.space image: builds the web app, serves it with Caddy on Fly |

## Run

The desktop app, against the glonOdin daemon or as its own replica:

```sh
cd gpui
cargo run --release -p roostr                 # the workspace
cargo run --release -p roostr -- --replica    # no daemon: this device's own replica
cargo run --release -p roostr -- /guide       # any site path
```

The browser build needs the nightly the `Dockerfile` pins (on wasm, gpui builds
`parking_lot` with its `nightly` feature; stable fails in `wasm_thread` with
E0554). Pin it for this folder only, so your default toolchain stays as it is:

```sh
rustup toolchain install nightly-2026-10-24 --profile minimal --target wasm32-unknown-unknown
rustup override set nightly-2026-10-24 --path gpui
gpui/scripts/build-web.sh     # → gpui/web-dist/ (first build a few minutes)
```

Keep the date in step with `RUST_TOOLCHAIN` in the `Dockerfile`. A Roostr
service started with `--web <this repo>` runs this build itself whenever
`web-dist/` is older than its sources, so the override must be set there too.

`web-dist/` is the whole site. Serve it at the root with every unknown path
answered by `200.html` (`/guide` by `guide.html`, and so on) - the Caddyfile
does exactly that, so the quickest local server is the production image:

```sh
docker build -t roostr-web .
docker run --rm -p 8080:8080 roostr-web     # http://localhost:8080
```

By default a tab is its own replica: the engine, IndexedDB `roostr-replica`,
and the identity in `localStorage` `roostr-key`. The website pages show on
their own; "Open Roostr" starts the vault. A tab can pair with the daemon and
harness on its own computer instead (see the gpui README, "Browser (roostr.space)").

## Shared Odin engine

Protobuf and hashing, replay, queries, mutation planning, the relay receive
session and shared-space authority run in the same Odin sources as the
glonOdin daemon and the iOS app. The desktop links them natively (built from
the sibling `glonOdin` checkout by `gpui/crates/roostr-engine/build.rs`); the
browser loads `static/engine.wasm`, checked against the sha256 in
`static/engine-core.json` both at build time (`build-web.sh`) and when the
tab loads it.

After changing the shared Odin sources, regenerate and commit both files (needs
Odin and Node):

```sh
node ../glonOdin/build-wasm.mjs .
```

Never hand-edit the manifest to get past the check.

Sync follows glonOdin `docs/state-sync.md`: NIP-77 negentropy against each
relay per stream (personal, each space), bases first, then legacy
checkpoints, then deltas, with a per-stream floor. Local changes enter a
durable outbox before they are published; logging out refuses while any are
unpublished.

## Check

```sh
cd gpui
cargo test --workspace
cargo clippy --workspace --all-targets
cargo clippy -p roostr-web --target wasm32-unknown-unknown
cargo fmt --all --check
```

## Deploy

```sh
fly deploy --remote-only
```

The image (`Dockerfile`) builds `gpui/web-dist/` with a pinned nightly (on
wasm, gpui builds `parking_lot` with its `nightly` feature), checks the engine,
precompresses the large files (brotli and gzip) and serves them with Caddy:
no-cache everywhere (nothing is content-hashed), 301s from the old `/gpui/`
preview paths and from `getroostr.fly.dev` / `www.roostr.space` to
`roostr.space` - except the workspace and its files, since a browser vault's
key and outbox live in that origin's storage.
