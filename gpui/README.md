# Roostr for desktop and the browser (Rune)

A native port of this repository's Svelte UI: the `/app` workspace and the
website pages (`/`, `/guide`, `/privacy`, `/j`). It is built on
[Rune](https://github.com/anomalyco/rune), pinned to the `dev` rev `8e3fc8e`
(`rune` and `rune-kit` in the workspace `Cargo.toml`; Rune is pre-release and
`dev` only moves forward). The directory keeps its old name, `gpui/`. The
iPhone app is not part of this workspace on this branch (see iOS below). It
runs in one of two modes, the Svelte app's two:

- **Daemon** (the desktop default, the web app's local mode): the glon-odin
  daemon owns storage, signing, replay and sync, and this app reads and
  writes through its API; its harness serves the computer features (files,
  credentials, capabilities, agent presence).
- **Replica** (the browser default and the only mode on iOS, the web app's
  browser mode, no daemon): `roostr-replica` keeps the vault on this device
  — the shared Odin engine replays, queries and plans, a change store
  (SQLite natively, IndexedDB in the browser) holds every change, and Nostr
  relays are the only network. Same wire format as the Svelte browser
  replica and the daemon, so one key's vault syncs across all of them.
  Files are encrypted and uploaded to the relays' Blossom server; the other
  computer features show "Roostr isn't running on this computer".

Settings → "Where your vault lives" switches modes (the app relaunches, the
tab reloads); the choice is kept per device.

## Run

### Daemon mode

Start the daemon (and, for computer features, the harness) from the sibling
`glonOdin` repository, as in its README. Then:

```sh
cd gpui
cargo run --release -p roostr            # opens the workspace
cargo run --release -p roostr -- /guide  # or any web path: /app/object/<id>, /app/graph, /
scripts/bundle-mac.sh --install         # Roostr.app with the Roostr icon, copied to ~/Applications
```

The app authenticates as the vault's own user with the daemon's service token
(`$GLON_DATA/api-token`, mode 0600) — there is no browser pairing. For the
harness it mints a UI session from that token, bound to the loopback origin
`http://127.0.0.1:7390`, and keeps it for its 24 hours. A daemon that can't
be reached offers the replica instead.

### Replica mode

```sh
cargo run --release -p roostr -- --replica          # or ROOSTR_BACKEND=replica
ROOSTR_REPLICA_DIR=/tmp/roostr-replica cargo run -p roostr -- --replica  # a throwaway replica
```

No daemon. A replica without a key opens on the sign-in gate (KeyGate.svelte:
paste an `nsec`/hex key, or create an account). The replica lives in
`$ROOSTR_REPLICA_DIR` (default `<data dir>/Roostr/replica`, i.e.
`~/Library/Application Support/Roostr/replica` on macOS): `replica.sqlite`,
and `local.json` (mode 0600) holding what the browser keeps in
`localStorage` under the Svelte app's key names — the secret key
(`roostr-key`), space keys (`roostr-space-keys`), relays (`roostr-relays`),
invites sent, join requests. It is a file rather than the Keychain: an
unsigned development build is a new binary to the Keychain after every
rebuild and would prompt each time. Settings → "Log out on this device"
erases the replica and the key once every change is published; "Reveal
private key" shows the `nsec`. A fresh account's empty vault gets a Personal
space once its first full history walk proves the vault empty.

Building the desktop app compiles the engine from `../../glonOdin` (`odin` on
`PATH`, or `ODIN`; `GLON_ODIN_DIR` for another checkout) - see
`crates/roostr-engine/build.rs`.

| Variable | Default | Meaning |
|---|---|---|
| `ROOSTR_BACKEND` | the saved choice, else `daemon` | `daemon` or `replica`; `--daemon`/`--replica` win over it |
| `ROOSTR_REPLICA_DIR` | `<data dir>/Roostr/replica` | Replica mode's store and device-local file |
| `GLON_DATA` | `~/.glon` | Vault directory holding `api-token` |
| `ROOSTR_API` | `http://127.0.0.1:7333` | Daemon |
| `ROOSTR_API_TOKEN` | — | Token instead of the file |
| `ROOSTR_HARNESS` | `http://127.0.0.1:7334` | Harness |
| `ROOSTR_UI_STATE` | `<config dir>/Roostr/ui-state.json` | Device-local UI state (the web app's `localStorage` keys) |
| `RUST_LOG` | `warn` | Log filter (stderr) |

The mode choice is `<config dir>/Roostr/mode` (`localStorage`
`roostr-gpui.mode` in a browser).

## Crates

| Crate | Ports |
|---|---|
| `roostr-core` | The model, with no UI dependency: wire types, `Backend` trait + daemon/harness clients, the on-device `Account` (replica identity, profile, join requests), the mode choice, the app `State` and its `Store` (`data.svelte.ts`), navigation and tabs, UI state, and the shared `lib/*.ts` helpers. Theme, icons and motion moved to `roostr-ui` |
| `roostr-rt` | The async runtime: `Executor<State>` (`s.rt`) with `spawn`, `background`, `timer`; spawned futures get an `AsyncCx` whose `update`/`read` reach the state |
| `roostr-ui` | The design system on Rune: theme (dark and light), type and bundled fonts, icons (`Glyph`), motion presets, and the shared components (buttons, inputs, selects, menus, dialogs, tooltips, calendar, colour picker, ...). It does not depend on `roostr-core` |
| `roostr-host` | Runs `State` in a Rune window: `spec`, `setup` and `poll` (runs `s.poll()`, fetches remote images, wakes at the next timer), and `run_example`, the runner every feature crate's examples use |
| `roostr-engine` | The shared Odin engine: a static library natively (built from `glonOdin`), the site's `engine.wasm` in the browser |
| `roostr-replica-store` | The replica's change store: SQLite natively, IndexedDB (`roostr-replica`) in the browser, plus the one-time import of the Svelte app's IndexedDB `roostr` (`svelte_import`) |
| `roostr-replica` | The daemon-less backend (`engine/backend.ts`, iOS `Backend.swift`): replay, query cache, mutations, shared spaces, identity and device-local storage; `sync/` is the relay sync (`sync.ts`, `SyncEngine.swift`) |
| `roostr-nostr` | The replica's Nostr host side (`engine/{keys,negentropy,spacekeys,profile}.ts`, sync.ts' relay and gift-wrap code, the iOS `RoostrSync` equivalents): keys and BIP-340 signing, NIP-01 events, NIP-44, NIP-59, space keyring, NIP-77 negentropy, the relay client (native and browser websockets), NIP-11/Blossom discovery. `cargo test -p roostr-nostr -- --ignored` also checks Roostr's live relay |
| `roostr-direct` | Direct lines: an agent's live progress over a WebRTC data channel (`roostr-live`) from the computer running it, introduced by kind-25079 signals. The viewer state machine (hello every minute, answer offers addressed to this device, trickle ICE, bye), str0m natively (one UDP socket, STUN for a server-reflexive candidate, no TURN; DTLS over Windows' own crypto there, RustCrypto elsewhere) and `RTCPeerConnection` in the browser |
| `roostr-properties` | Property values and pickers, properties pane, object meta, type and property pages, repeat, credentials, capabilities, emoji picker |
| `roostr-editor` | Block editor, slash/block menus, tables, links and mentions, spellcheck, tool-code lint, file blocks and the File page (harness + Blossom) |
| `roostr-sets` | Query/set/collection/type views: controls, table, kanban, gallery, calendar, row menu |
| `roostr-discussion` | Discussion, agent exchanges, chat & properties drawer, chat page |
| `roostr-graph` | Schema/object graph |
| `roostr-settings` | Settings, space page, install-computer card, identity gate, export |
| `roostr-site` | Landing, guide, privacy and join pages |
| `roostr-shell` | Space rail, sidebar, tab strip, header, search, New menu, pinned widgets; at a window 720pt wide or less (a phone in portrait), the mobile shell instead: Spaces list → space home → object, as the web app showed on phones |
| `roostr-app` | What both front ends run: feature init, the page per route, the object page with its cover (`object_page/cover.rs`: banner, Add/Change cover, Reposition, the generate panel over `roostr_core::cover`'s `cover` / `cover_y` / `cover_request` / `cover_generation` fields) |
| `roostr` | The desktop app: window, menus |
| `roostr-web` | roostr.space (wasm): entry point, the website pages before any vault, replica or pairing gate, address-bar routing, the WebGL2 colour fix (`gamma_canvas.rs`), `www/` page template and loader |
| `roostr-ios` | The iPhone/iPad app. Excluded from the workspace on this branch: it still uses GPUI and ships from `main` until Rune has iOS text input |

## Architecture

- The whole app is one `roostr_core::State`. Rune hands it to handlers as
  `&mut State` and to views as `&State`.
- Async work runs on the `roostr-rt` executor kept in `s.rt`:
  `s.rt.spawn(async move |cx| { .. cx.update(|s| ..) })`. Dropping a `Task`
  cancels it. Blocking work goes through `s.rt.background(..)`.
- Events are queued and dispatched by `State::poll`: listeners registered
  with `on_store` and `on_nav`, and `on_poll` for work that checks state after
  every poll (for example committing a field that lost focus, since Rune has
  no blur event).
- Each feature crate keeps its UI state in a slot (`s.slot::<T>()`), keyed by
  object id where needed, and registers its listeners in a
  `register(s: &mut State)` the app calls once.
- Views are functions `fn(&State, &mut rune::Cx<State>) -> El`. Rune runs the
  view again after every handler and after any poll that changed something,
  so there is nothing to invalidate by hand.
- Views take their look from `roostr-ui` (`roostr_ui::theme(pref, cx)`),
  never from hard-coded colours.
- `roostr-host` wires `State` into a Rune window (setup and poll hooks) for
  the desktop app and the examples.

Data flows one way: views read the `Store`; mutations go through
`roostr_core::api`, and the backend's commit stream (the daemon's SSE, or the
replica's own commits, relay imports and sync status) refreshes what changed.

## Browser (roostr.space)

The same app is all of roostr.space, at the URLs the Svelte site used: `/`,
`/guide`, `/privacy`, `/j#r2.<blob>`, `/app/**` (the old `/gpui/` preview
paths redirect there). The website pages show on their own, before any vault
opens, each from its own HTML file carrying the Svelte site's title and
link-preview tags; the join page opens in the workspace when the tab already
has an identity (a replica key or a pairing), and otherwise asks to set one up
first. A workspace route starts the vault. The tab title follows the route
with the Svelte pages' strings (`Route::title`: `<name> — glon` on an object
page, tracking its saved name, `Graph — Roostr`, `Roostr` for the home and
chats), and a loaded URL that already names its route keeps its query
(`/j?utm=x#r2.…`, `/app/graph?focus=<id>`). By default the tab is its own
replica, like the Svelte app in browser mode: the engine (`engine.wasm` +
`engine-core.json` at the site root, hash-checked), the IndexedDB database
`roostr-replica`, and the identity in `localStorage` `roostr-key` - the
Svelte app's own entry on the same origin, so a key it saved opens here
without signing in again (its space keys, relays and invite bookkeeping too:
`roostr-space-keys`, `roostr-relays`, …).

Space keys also come from the account's keyring backup (kind 30078,
`d` = `roostr-keyring`, NIP-44 encrypted to the account itself), which the
daemon publishes: read at start and on every catch-up, and followed live.
A space created on the account's computer is therefore shared in a browser
signed in with the same key; without it the tab treated that space as
private, wrote its chat to the personal stream and could not read replies
sealed under the space key. A key the tab holds at the same or a newer
keyId is kept; a vanished or left space stays gone.

The Svelte app's own replica, IndexedDB `roostr`, is imported into
`roostr-replica` once, on the replica's first start for the key, before its
first sync (`roostr_replica_store::svelte_import`): every change (bytes,
JSON, base tag, rebased mark), checkpoint, base, held relay event and
publication record, the walk's cursor, floors and replay groups when this
store has none yet, and every unpublished change, which the outbox then
publishes. The replay memo (`states`) is not carried: its rows name no
engine (only a change count, checkpoint and base), so nothing proves they
are what this engine replays the inputs to. The first open prepares the
vault instead (below). It imports only the stored key's vault - the Svelte
app deleted the database on every identity change, so it belongs to that
key; it must also hold a change authored by the key - and stamps it
(`gpui-import` in its `meta`) so no later identity, nor this one after a
logout, imports it again.
The Svelte database is otherwise left as it was. A visitor without one never
gets one (`indexedDB.databases()`, else an open that aborts its upgrade). A
failed import logs a warning and retries on the next start; it never keeps
the replica from starting. It reads the Svelte stores 500 rows a request
and writes 1,000 rows a transaction, every lookup of a batch issued at once
(publication marks, obligations, checkpoints, bases), so a slow disk costs a
round trip per batch, not per row; the boot screen meanwhile says "Importing
· 4,000 of 16,323 changes".

Every IndexedDB operation runs under a watchdog (`IDB_TIMEOUT_MS`, 12 s):
it reports "local storage stopped responding" only once the database has
answered nothing for that long while the page was responsive - a busy main
thread, a slow disk, or an operation queued behind another's long
transaction never counts. Whole-store reads go 500 rows per request, and
per-object lookups are issued together rather than one round trip each, so
no operation's length grows with the vault (a wall-clock limit on one
operation used to fail every start of a ~2,000-object vault on slower
disks, at `cached_replays`). `cargo test --release -p roostr-replica --test
large_vault -- --ignored` (with `SEED_RELAY`, `SEED_OUT`) seeds a throwaway
~18k-change vault with bases and shared spaces onto a relay, to load against.

A history page (up to 128 events, or 100 fetched ids) is written as one
transaction - its changes, their publication marks and its held relay
events (`ChangeStore::import`; the SQLite store does the same for the iOS
app and the desktop) - instead of two per change, so a first sync of a large
vault is not thousands of transactions. A shared payload's authorization
reads what the store holds for its space and object; a page with buffered
changes of either writes them first, so every verdict sees what it did when
each change was written on its own. A page cut off mid-write leaves none of
its changes and none of its events held: the next walk asks for it again
and stores each change once. Each shared-space refresh (every start, and
on space changes) finds the space's unacknowledged changes in a few
requests (`unpublished_changes`), not two reads per change.

Objects no replay memo row serves (the first open after the Svelte import,
or after an engine update) do not hold the workspace back: a full pass
replays them for a second at most (the vanish ledger and the keyring's
spaces first, then the objects with the fewest changes) and prepares the
rest in the background in 50 ms steps, the page rendering in between and
the memo written every 50 objects, so a reload resumes where it stopped.
Meanwhile the status carries `preparing` (shown as "Preparing · 400 of 1,900
objects" on the boot screen and the sync dot, whose ring fills with it),
reads serve what is prepared, anything asked for by id (an open object, a
mutation's missing ids, the shared spaces) is prepared first, and
`bootstrapped` is withheld - nothing seeds a Personal space or migrates
the space order against a partial vault; `BackendEvent::Prepared` then
reloads every view. The sync digest, serving, reminders and the catalog
seeding wait for the whole vault. The boot screen also shows the history
pass's count ("Syncing · 3,200 of 15,000 (21%)") while it loads.

The alternative is the daemon and harness on the visitor's own computer, like
the hosted Svelte app in paired mode ("Pair with Roostr on this computer
instead" on the sign-in gate, or Settings → "Where your vault lives"): the
tab pairs with the daemon's one-use code (`POST /api/pair`) or - "Sign in
with a Nostr key instead" - by signing the harness's one-use challenge with
the vault owner key (`GET /pair/challenge`, a NIP-98 kind-27235 event to
`POST /pair/owner`, signed by `roostr-nostr`). It keeps the session in
`localStorage` (`roostr-local-pairing`, the Svelte app's key and shape) until
it expires, and sends it as a bearer token to both; a 401 drops it. A key
that paired is kept as the hosted Svelte app keeps its key (`localStorage`
`roostr-key`, hex) when the origin holds none yet, and the gate offers it
again in one click. The commit stream is a streamed `fetch`. A tab that
paired before keeps pairing until a mode is chosen. A page served with
`<meta name="roostr-mode" content="daemon">` defaults a tab that has neither
chosen nor paired to this mode, as the Svelte app's `dev:local` build did.

```sh
rustup target add wasm32-unknown-unknown
scripts/build-web.sh          # → web-dist/, the whole site
```

Rune's web target draws the app into one canvas over WebGPU, falling back to
WebGL2 where a browser has no WebGPU. On WebGL2, Rune leaves the canvas in an
sRGB format while its runtime still blends in gamma space, so colours come
out washed out. `roostr-web`'s `GammaCanvas` (`gamma_canvas.rs`) draws such a
frame into a plain texture and copies it to the canvas decoded to linear
light, so both paths show the same colours.

The script builds with the `web` profile (size-optimized, fat LTO,
stripped) and installs the `wasm-bindgen` CLI matching `Cargo.lock` into
`target/tools/` when the one on `PATH` differs; it first checks
`../static/engine.wasm` against `engine-core.json`. `wasm-bindgen` also
writes `snippets/` (Rune's page bridge, JS modules `roostr_web.js` loads),
which must be served beside it. `scripts/build-web.sh --debug` builds with
the dev profile, faster to build. `web-dist/` has one page
per website route (`index.html`, `guide.html`, `privacy.html`, `j.html`) and
`200.html` for every other path; serve it at the root with `/<path>` answered
by `<path>`, `<path>.html`, else `200.html` (the Dockerfile and Caddyfile at
the repository root do, from brotli/gzip copies made at image build). Any
loopback origin (`http://localhost:<port>`, `http://127.0.0.1:<port>`) may
pair, so a local static server works for development. glonOdin's
`bun run service install --web <this repo>` (its `harness/src/web.ts`)
builds `web-dist/` when it is missing or stale and serves it that way on
`http://127.0.0.1:5190`, with the daemon-mode tag in every page. UI state
lives in `localStorage` under `roostr-gpui.ui-state`.

Emoji: the build ships Twemoji (`crates/roostr-ui/fonts/TwemojiMozilla.ttf`,
a COLR font, CC-BY 4.0 - see its license file) as `web-dist/fonts/`; the app
fetches it with the GPU's start-up, before the first frame
(`roostr_ui::fonts::EMOJI_FONT_PATH`), and draws every emoji, graph labels
included, in it. The desktop uses Apple Color Emoji.

Text: the UI is Intern everywhere (`crates/roostr-ui/fonts/Intern.ttf`, a
rounded Inter, OFL), with Inter after it for Greek and Cyrillic.

Symbols: the browser lays text out only in its bundled fonts (Intern, Inter and
JetBrains Mono), so UI symbols they lack (`▨` `◍` `⌕` `☐` `＋` …) come
from `crates/roostr-ui/fonts/RoostrSymbols.ttf`, a 4 KB subset of Noto Sans
Math / Symbols / Symbols 2 (OFL; fullwidth `＋` `＠` are its `+` `@` centred
on a full em). `roostr-ui`'s `glyph_coverage` test fails on any non-ASCII
character in a source literal no bundled web font draws: add it to the subset
(fontTools) or draw a `Glyph` instead.

Not in the browser yet: anything that opens a file dialog (adding files and
images, a custom icon or avatar), since Rune's web target has no file picker;
dropping files from the desktop. Exports and file downloads are browser
downloads; "open" shows a file in a new tab.

Keymaps use Rune's `mod` chord (`rune::Chord::parse("mod+k")`): Command on
macOS, Control elsewhere.

### Lite loading in the UI

A replica decides on its first start whether it is lite or full (device key
`roostr-holding`): the browser and the iPhone start lite when they hold
nothing yet (a Svelte or Swift vault already there stays full); the desktop
replica is full. How a lite replica syncs - directories first, objects
fetched by id and followed by their `h` tag, spaces loaded on demand - is in
glonOdin's `docs/state-sync.md` ("Directories");
`crates/roostr-replica/tests/lite.rs` runs it end to end against a relay.

A lite replica (roostr.space and the iPhone by default) holds only what was
opened, so the views that need a whole space say so (`roostr_core::lite_ui`,
the same notices on desktop and phone widths). An opened query/set or type
page - pinned widget or page - over a space not held whole downloads that
space by itself, showing "Loading this space · N of M" until its rows are
complete; a pinned collection or link widget fetches just the objects it
shows. Search, backlinks and the @-agent list show what the device holds plus
"Load this space fully". The graph draws loaded objects as nodes and, per
type, the directory's remaining count as faint placeholder dots labelled
"N not loaded"; selecting the type (or the totals) offers the download. A
space whose directory is not built yet says "This space isn't ready for lite
yet — load it fully once", a vault without a built personal directory "This
vault isn't ready for lite yet" (button: fully sync). Settings → "Where your
vault lives" shows "This device holds about N% of your vault" on any replica
and, when lite, "Fully sync this device": a details sheet (what it
downloads, how long, storage used now, offline afterwards) before it
switches; the sync status line then shows the download. A browser holding
the whole vault shows "Switch to Lite" instead: after a confirm it drops its
local copy and reloads lite, still signed in with its space keys
(`LiteControl::switch_to_lite`); it refuses while changes are not sent yet.

### Your devices

Every device of the account keeps a device record (glonOdin
`core/device.odin`, `docs/state-sync.md` "Device records"): a replica writes
its own (`roostr-replica` `write_device_record`, named by `DeviceInfo`:
"Chrome at roostr.space", the iPhone's model, "Desktop app on <host>") once
its first sync settles, when what it holds changes and on a 5-minute check
(`last_seen` alone at most every 30 minutes); the daemon writes its own. A
lite replica fetches the records the personal directory lists. Settings →
"Where your vault lives" lists them under the mode line in every mode
(`roostr_core::devices`): name ("(this device)" by `Settings.device_id`),
Full/Lite, "% held" when lite, last seen; Rename keeps a person's name over
the device's own, Remove deletes the record until the device next runs;
devices unseen for 30 days fold under "Older devices".
`crates/roostr-replica/tests/devices.rs` runs two replicas of one key end to
end.

### Direct lines

An agent's live progress (kind 25078, `roostr_core::live`) also travels
straight from the computer running the agent: the store runs one viewer per
open vault (`roostr_core::direct`, `roostr-direct`) under this install's id
(`direct-device` in the UI state: `localStorage` in the browser,
`ui-state.json` on the desktop). It says hello on start and every minute; a
harness without a line to this device offers one, the viewer answers, both
trickle ICE (STUN `stun.l.google.com:19302`, no TURN), and each live event
arrives on the `roostr-live` data channel, one payload per message. Channel
messages enter the store as the same `BackendEvent::Live` their relay copy
would; the chat drops duplicates by (run, seq), so the harness still sends
the relay copy whenever a viewer has no line. A line introduced in a shared
space may only speak of that space's objects.

The signals are kind 25079, sealed like the live events: the account's own
with its self conversation key and no tags, a shared space's with the space
key and `h` = the space tag; a hello goes out in every scope, everything else
in the scope its line came in. The replica subscribes and publishes them
itself (`sync/signal.rs`; signed by this account = its own scope, otherwise
only from the space's writers); a daemon-backed app hears them as
`/api/events` `signal` frames and sends them with `signal_publish`.
`crates/roostr-direct/tests/loopback.rs` runs a line between two native peers
in one process (`-- --ignored` also over the network with real STUN).

### Sync tracing

When one browser misses what other devices see, switch on tracing in that
tab's console and reload:

```js
localStorage.setItem("roostr.syncTrace", "1"); location.reload()
```

`[trace]` lines then show each relay subscription (its filters) and its
up/down, every live event (id, kind, signer, `h` tags), what the engine made
of it (change, base, a part still waiting, decrypt or replay failures),
imports and authority refusals, each object replayed (changes, bases, heads,
its latest chat message), each open chat's message count and latest message,
and what this tab published. `localStorage.removeItem("roostr.syncTrace")`
turns it off (`roostr_core::sync_trace`).

## iOS

The iPhone/iPad app (`crates/roostr-ios`) is excluded from this workspace on
this branch. It still uses GPUI and keeps shipping from `main` until Rune has
iOS text input. Its sibling `RoostrIOS` repository (the Xcode host) builds it
from `main`; see that branch's README for its build steps.

## Open tabs

Which tabs each space has open, in order, is shared by all of a person's
devices: the private preferences object (`__user_prefs__`) holds them in
`open_tabs`, a map of space id to the tabs' route paths
(`{"sp-1": ["/app/object/<id>", "/app/graph"]}`). A change is written 2 s
after the last one, only when it differs from what is stored; another
device's change replaces the strip in place, keeping this device's selected
tab (else its nearest neighbour). The selected tab stays per device, with a
local copy of the strips, in the UI state (`roostr.tabs.spaces`); the first
read merges this device's older local tabs in once. Tabs of a binned or
vanished object close everywhere. On a phone, moving around leaves the tabs
alone; the space home's tab button opens the Open tabs list (tap opens, ×
closes). See `roostr_core::nav`.

## Selectable text

Chat messages are selectable text, as the web view's DOM text was: a mouse
drag selects across lines and messages and ⌘C copies. Each line is Rune
selectable text in the conversation's one selection group
(`roostr-discussion/src/md.rs`).

## Motion

Transitions use Rune's motion through `roostr_ui::motion`: presets with the
Svelte CSS timings (`HOVER_REVEAL` 100ms, `FADE` 120, `CONTROL` 150,
`HANDLE` 150 on cubic-bezier(.22,1,.36,1), `COLUMNS` 160, `SHEET_SNAP` 180,
`DISCLOSURE` 200, all CSS `ease` otherwise), springs only to settle a
released gesture, and `THEME_SWITCH` for the colour glide when the theme
changes. Motion never holds input: menus, dialogs and sheets take keys and
clicks from their first frame and close at once; nothing animates on load or
on changes arriving from sync, except a new chat message.

On a touch screen the phone shell's screens slide as a navigation stack
(`roostr-shell/src/back_swipe.rs`): opening a space, an object or a chat
slides it in from the right over the screen it came from, and a back button
slides it off again, the screen beneath riding in from 30% left and
brightening, both on a 300ms release spring. The web shell leaves the left
edge to the browser's own back gesture.

Reduced motion is Rune's (`roostr_ui::motion::reduced(cx)`). It follows the
OS setting (`prefers-reduced-motion` in a browser): travel snaps and fades
are short.

## Develop

Every feature crate has an example that hosts its views against the daemon in
the environment, through `roostr_host::run_example`:

```sh
cargo run -p <crate> --example <name> -- <args> [--snap out.png --size WxH]
```

The examples are `roostr-editor` `editor`, `roostr-properties` `properties`,
`roostr-sets` `sets`, `roostr-discussion` `discussion`, `roostr-graph`
`graph`, `roostr-settings` `settings`, `roostr-shell` `shell`, `roostr-site`
`site`, and `roostr-ui` `gallery` (every component; `--theme dark` or
`--theme light`). The window opens once the store's first load lands, so
`--snap` writes a PNG of real data without showing a window. Rune's
`--script` drives an example headlessly (clicks, typing, keys) on a virtual
clock.

Develop against a throwaway vault on its own port, never your own:
`scripts/seed-test-vault.sh` fills it with sample data covering every view
(it refuses to run unless `GLON_DATA` is set and is not `~/.glon`, and reruns
add nothing).

```sh
mkdir -p /tmp/roostr-test
GLON_DATA=/tmp/roostr-test ../../glonOdin/glon-odin serve 7399 &
export GLON_DATA=/tmp/roostr-test ROOSTR_API=http://127.0.0.1:7399
scripts/seed-test-vault.sh
cargo run -p roostr-editor --example editor -- seed-page-showcase
cargo run -p roostr-editor --example editor -- seed-page-showcase --snap /tmp/editor.png --size 1280x860
cargo run -p roostr                      # the whole app on the test vault
```

Views are tested headlessly with `rune::Runtime`: a test builds a `State`,
renders the view, presses and types through Rune's virtual pointer and
keyboard, and moves time with the virtual clock and `Executor::advance` (no
wall-clock sleeps). See `tests/` in `roostr-properties`, `roostr-sets`,
`roostr-settings` and `roostr-app`.

The dev profile keeps only line tables for the workspace's own crates
(`debug = "line-tables-only"`) and no debug info for dependencies, so builds
and the target dir stay small; backtraces still have file and line.

Checks: `cargo test --workspace`, `cargo clippy --workspace --examples`,
`cargo fmt --all --check`.

Browser tests (IndexedDB, the Svelte vault import) run in a real browser:
serve `../static` (`engine.wasm`, `engine-core.json`) with CORS at
`http://127.0.0.1:8787/` (or set `ROOSTR_ENGINE_TEST_BASE` at build time), then
`CHROMEDRIVER=<chromedriver> CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner
cargo test --target wasm32-unknown-unknown -p roostr-replica-store` (and
`-p roostr-replica --test svelte_import_web`), with the `wasm-bindgen-test-runner`
matching `Cargo.lock` (`target/tools/` after `scripts/build-web.sh`); a
`webdriver.json` in the crate directory picks the Chrome binary when it differs
from the driver's.

## Differences from the web app

Platform-driven only; behavior and visuals otherwise follow the Svelte source.

- Embedded players (YouTube, Vimeo, Spotify) show a poster card that opens the
  link in the browser; PDFs, video and audio open in the system viewer.
- Settings → "Disconnect this app" quits instead of unpairing a browser;
  exports are written to `~/Downloads`; `prompt()`/`confirm()` are in-app or
  native dialogs.
- Graph: CSS blend modes and backdrop blur have no equivalent; the radial
  background is stepped.

## Differences from the GPUI app

- A light theme exists (Roostr was dark only); dark is value for value.
- The spelling menu opens on a right-click inside a field with the caret in a
  misspelled word. Misspellings and lint marks are straight underlines, not
  squiggles.
- A link inside block text shows no pointing-hand cursor of its own (Rune
  has no per-glyph cursor).
- Rune has no blur event: a field commits what was typed once it loses focus,
  checked on the next poll; Enter still commits at once.
- Hover is an overlay over a control's fill: border-colour changes on hover
  and press fades on text buttons are not reproduced.
- Tooltips use Rune's help: 500 ms delay, 150 ms fade, no slide; on macOS
  Rune draws them, not AppKit.
- Dialogs have no exit animation; kit submenus stay open when another row is
  hovered; Select and Command lists are not virtualized.
- Scrollbars are rune-kit's (2 pt edge gap, 24 pt minimum thumb) and follow
  the system "show scroll bars" setting.
- Spinners rotate linearly, Progress does not animate value changes, and
  avatar initials pick a different hue.
