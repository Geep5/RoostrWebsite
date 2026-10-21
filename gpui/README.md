# Roostr for desktop (GPUI)

A native port of this repository's Svelte UI — the `/app` workspace and the
website pages (`/`, `/guide`, `/privacy`, `/j`) — built on
[GPUI Kit](https://gpui-kit.com) 0.7.3. It runs in one of two modes, the
Svelte app's two:

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
| `roostr-core` | Wire types, `Backend` trait + daemon/harness clients, the on-device `Account` (replica identity, profile, join requests), the mode choice, the global `Store` (`data.svelte.ts`), navigation and tabs, UI state, theme tokens, icons, motion presets (`motion`), and the shared `lib/*.ts` helpers |
| `roostr-engine` | The shared Odin engine: a static library natively (built from `glonOdin`), the site's `engine.wasm` in the browser |
| `roostr-replica-store` | The replica's change store: SQLite natively, IndexedDB (`roostr-replica`) in the browser, plus the one-time import of the Svelte app's IndexedDB `roostr` (`svelte_import`) |
| `roostr-replica` | The daemon-less backend (`engine/backend.ts`, iOS `Backend.swift`): replay, query cache, mutations, shared spaces, identity and device-local storage; `sync/` is the relay sync (`sync.ts`, `SyncEngine.swift`) |
| `roostr-nostr` | The replica's Nostr host side (`engine/{keys,negentropy,spacekeys,profile}.ts`, sync.ts' relay and gift-wrap code, the iOS `RoostrSync` equivalents): keys and BIP-340 signing, NIP-01 events, NIP-44, NIP-59, space keyring, NIP-77 negentropy, the relay client (native and browser websockets), NIP-11/Blossom discovery. `cargo test -p roostr-nostr -- --ignored` also checks Roostr's live relay |
| `roostr-properties` | Property values and pickers, properties pane, object meta, type and property pages, repeat, credentials, capabilities, emoji picker |
| `roostr-editor` | Block editor, slash/block menus, tables, links and mentions, spellcheck, tool-code lint, file blocks and the File page (harness + Blossom) |
| `roostr-sets` | Query/set/collection/type views: controls, table, kanban, gallery, calendar, row menu |
| `roostr-discussion` | Discussion, agent exchanges, chat & properties drawer, chat page |
| `roostr-graph` | Schema/object graph |
| `roostr-settings` | Settings, space page, install-computer card, identity gate, export |
| `roostr-site` | Landing, guide, privacy and join pages |
| `roostr-shell` | Space rail, sidebar, tab strip, header, search, New menu, pinned widgets; at a window 720pt wide or less (a phone in portrait), the mobile shell instead: Spaces list → space home → object, as the web app showed on phones |
| `roostr-app` | What both front ends run: feature init, the page per route, the object page |
| `roostr` | The desktop app: window, menus |
| `roostr-web` | roostr.space (wasm): entry point, the website pages before any vault, replica or pairing gate, address-bar routing, `www/` page template and loader |
| `roostr-ios` | The iPhone/iPad app (a static library the RoostrIOS Xcode host links): UIKit platform, Keychain/UserDefaults storage, reminders, lifecycle and network resume, deep links |

Data flows one way: views read the `Store` (and re-render on its events);
mutations go through `roostr_core::api`, and the backend's commit stream
(the daemon's SSE, or the replica's own commits, relay imports and sync
status) refreshes what changed.

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

The script builds with the `web` profile (size-optimized, fat LTO,
stripped) and installs the `wasm-bindgen` CLI matching `Cargo.lock` into
`target/tools/` when the one on `PATH` differs; it first checks
`../static/engine.wasm` against `engine-core.json`. `web-dist/` has one page
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

Emoji: the build ships Twemoji (`crates/roostr-core/fonts/TwemojiMozilla.ttf`,
a COLR font, CC-BY 4.0 - see its license file) as `web-dist/fonts/`; the app
fetches it after it starts and draws every emoji, graph labels included, in
it. Until it arrives, and for emoji newer than it, the browser's own emoji
font stands in. The desktop uses Apple Color Emoji.

Symbols: the browser lays text out only in its bundled fonts, so UI symbols
Inter, IBM Plex Sans and JetBrains Mono lack (`▨` `◍` `⌕` `☐` `＋` …) come
from `crates/roostr-core/fonts/RoostrSymbols.ttf`, a 4 KB subset of Noto Sans
Math / Symbols / Symbols 2 (OFL; fullwidth `＋` `＠` are its `+` `@` centred
on a full em). `roostr-core`'s `glyph_coverage` test fails on any non-emoji
character in a source literal no bundled font draws: add it to the subset
(fontTools) or draw a `Glyph` instead.

Not in the browser yet: anything that opens a file dialog (adding files and
images, a custom icon or avatar) - GPUI's web platform has no file picker;
dropping files from the desktop. Exports and file downloads are browser
downloads; "open" shows a file in a new tab.

GPUI's web platform is `vendor/gpui-pre-web`, gpui-pre-web 0.3.9 patched in
through `[patch.crates-io]`, its changes marked `roostr:`. Once the last
finger lifts, the pointer moves off the canvas, so hover (and a `[data-tip]`
tooltip) ends with the touch instead of staying on the control last tapped.
In a Mac browser (an iPad's too) Command is the `secondary` key, as on the
desktop: GPUI parses `secondary-c` as Control on wasm, so Command and Control
trade places in keyboard and pointer events there (`keystroke_modifiers`;
not the wheel, where Control is a trackpad pinch). Command-C, -X, -A, -Z,
-B... act as on the desktop; Command-V stays the browser's paste, whose
`paste` event hands the text to the focused field without a permission
prompt. A browser keymap binds `secondary-`, never `cmd-`
(`cargo test --target wasm32-unknown-unknown -p roostr-web` with
`wasm-bindgen-test-runner` checks the mapping).

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
downloads, how long, storage used now, offline afterwards, only logging out
undoes it) before it switches; the sync status line then shows the download.

## iOS

`crates/roostr-ios` is the iOS app; the sibling `RoostrIOS` repository is
its Xcode host (XcodeGen spec, icon, a C `main`) and builds it from this
workspace - see its README for building, running and what lives where (the
Swift app's Keychain items, `UserDefaults` relay list and
`Application Support/Roostr/roostr.sqlite`, so an update keeps the vault).

```sh
rustup target add aarch64-apple-ios-sim aarch64-apple-ios
CARGO_PROFILE_DEV_DEBUG=line-tables-only cargo build -p roostr-ios --target aarch64-apple-ios-sim
cargo clippy -p roostr-ios --target aarch64-apple-ios-sim
```

GPUI's UIKit platform is `vendor/gpui-pre-ios`, a vendored gpui-pre-ios
0.3.9: that snapshot is newer than the gpui-pre 0.3.9 core it pins and
does not build against it, and it gains a document picker for
`prompt_for_paths`. It names its window as the active one (the published
crate's `active_window` was always none, so what acts on the active
window - an alert, closing a dialog - did nothing). It also takes over a
window scene UIKit restores with another app's scene delegate: an update
over the SwiftUI app restores that app's scene session, whose SwiftUI
`AppSceneDelegate` would show a black screen and crash on backgrounding.
`vendor/gpui-pre-apple` (patched in for every target through
`[patch.crates-io]`) changes iOS only: every font
falls back to the bundled Twemoji (`crates/roostr-core/fonts/TwemojiMozilla.ttf`,
the web build's emoji), drawn in color. Both mark their changes `roostr:`.

Layout follows the web view the Svelte app ran in: the window content sits
inside the safe area and above the on-screen keyboard; ≤720pt wide (iPhone
portrait) is the mobile shell, wider (landscape, iPad) the desktop layout;
touch shows hover-only controls at rest and long presses lift blocks and
spaces (`roostr_core::layout`).

Touch scrolling follows WebKit's directional lock, on the phone and in a
mobile browser alike: a pan keeps, with its fling, to the axis it first moved
further along, so a swipe down a query scrolls the page and leaves the
table's columns where they are, and the table (a board's columns, the tab
strip) scrolls sideways only under a mostly sideways swipe. The wheel and
the trackpad scroll as in a browser, on the web and the desktop alike: a
scroller takes only the axes it scrolls on, so a wheel down over the table
scrolls the page (Shift+wheel scrolls the table sideways), and a trackpad
gesture runs on a browser's scroll rails - one that moves clearly more along
one axis (twice as far) scrolls on that axis alone, so a swipe down that
drifts sideways leaves the table where it is, while a diagonal one moves
both. The graph canvas alone pans on both axes (`Window::pan_touch_freely`;
its wheel listener gets the wheel unrailed). This lives in GPUI's core,
`vendor/gpui-pre` (gpui-pre 0.3.9 patched in through `[patch.crates-io]`,
changes marked `roostr:`): its recognizer already locked each pan, but a
scroller that scrolls on one axis only took the other axis as its own, under
a swipe, a wheel and a trackpad alike.

Text input follows iOS WebKit too. Focus raises the on-screen keyboard only
from the person: a tap on a field, focus the tap itself moves (Reply, a
menu's search box), a field the app focuses under a tap just made (the line
a tap on an empty page creates), or one input handing focus to the next
while the keyboard is up (Enter splitting a block). A field the app focuses
on its own - the chat composer when a page opens - keeps focus without the
keyboard until it is tapped; with a hardware keyboard attached it takes
typing straight away. Return is the Enter key: bindings see it first (split
a block, send a message, confirm a field) and only an unhandled Return types
a newline. Delete with the caret at the start of a field is the Backspace
key, as the web view's `deleteContentBackward` was: a to-do, list item or
heading turns into a paragraph, a paragraph joins the block above. Holding
delete stops at a block's start, as it did in the web view (UIKit stops
repeating with no text before the caret); the next press goes on.
Dismissing the keyboard (swipe down, the iPad's hide key) ends
editing, as it blurred the field in the web view: the editor then shows a
remote edit to the block the caret was in, which it leaves alone while that
block is being edited (BlockNode.svelte's rule, `sync_live_texts`). Once the
last finger lifts, the pointer leaves the window, so nothing stays hovered
and no tooltip lingers after a tap. A `[data-tip]` also goes with its
control, as CSS's `:hover::after` did: a tap that takes the control off
screen (the back button to the Spaces list) hears no hover end, and the tip
is dropped once a frame no longer draws the control. The Settings sheet's
key import field stays above the keyboard as it rises, as WebKit scrolled a
focused input into view.

The edit menu is UIKit's (`UIEditMenuInteraction`), as it was in the web
view: a finger held still for half a second on the focused
field puts the caret under it (or keeps a selection it lands on) and the
menu opens when it lifts; a tap that leaves the caret where it was, or a
tap on the selection, toggles it. GPUI never sees that hold, so an input's
own long press does not run there; holds elsewhere (a block not being
edited, the edited block's gutter) still lift. Cut, Copy, Paste and Select
All - and a hardware keyboard's ⌘X ⌘C ⌘V ⌘A - send the key binding the
desktop gives them (`secondary-v`...), so a paste lands where ⌘V's does: a
link opens the editor's Paste as menu, copied blocks paste as blocks, and
an image - every image, when several photos were copied - becomes a File
block (uploaded to Blossom), ahead of the link Safari copies with it. The
pasteboard carries a string's metadata, a copied link and images. PNG,
JPEG, GIF, WebP, BMP, ICO and SVG paste as they are; an image GPUI cannot
draw - Safari's Copy of a photo puts its HEIC on the pasteboard, which
UIKit's `hasImages` does not even count - is redrawn upright and pasted as
JPEG, or PNG when it has transparency (`gpui-pre-ios` `pasted_image`). An
image's layout follows its EXIF orientation, as GPUI draws it, so the
sideways-stored JPEG of a portrait photo no longer spills over the blocks
below. In the browser the editor takes the browser's own paste event the
same way (images, Paste as, copied blocks), not only its text. The chat
composer is text only, as the Svelte textarea was. There is no Select
(word) item, no selection handles and no loupe.

Chat messages are selectable text, as the web view's DOM text was, on every
platform: a mouse drag selects across lines and messages and ⌘C copies; on a
touch screen a long press selects a word with gpui-kit's window text
selection (`gpui_kit::base::TextSelection`), whose handles extend it and
whose Copy / Select All menu sits over it (drawn by gpui-kit, not UIKit).

## Motion

Transitions run on [gpui-motion](https://github.com/1bye/gpui-motion)
(`gpui-motion-kit` 0.5.0 over `gpui-ce-motion` 0.4.0) through
`roostr_core::motion`: presets with the Svelte CSS timings (`HOVER_REVEAL`
100ms, `FADE` 120, `CONTROL` 150, `HANDLE` 150 on cubic-bezier(.22,1,.36,1),
`COLUMNS` 160, `SHEET_SNAP` 180, `DISCLOSURE` 200, all CSS `ease` otherwise),
springs only to settle a released gesture, and keyed element state
(`use_motion`, `animate`, `progress`, `hover`) for stateless components. Looping
indicators (sync pulse, typing dots, spinners) stay on gpui's
`Animation::repeat`. Motion never holds input: menus, dialogs and sheets take
keys and clicks from their first frame and close at once; nothing animates on
load or on changes arriving from sync, except a new chat message.

On a touch screen the phone shell's screens slide as a navigation stack
(`roostr-shell/src/back_swipe.rs`): opening a space, an object or a chat
slides it in from the right over the screen it came from, and a back button
slides it off again, the screen beneath riding in from 30% left and
brightening, both on a 300ms release spring. In the iOS app a pan from the
left 20pt drags the screen back with the finger (UIKit's interactive pop):
let go past a third of the width, or flicked right, and it goes back (an
object to its space home, a space home to the Spaces list), otherwise it
springs back. The pan is taken in the capture phase ahead of the page's
scrollers, so a pan starting anywhere else still scrolls tables and moves
through the editor. The web shell leaves the left edge to the browser's
own back gesture. Under reduced motion the screens swap at once and a swipe
goes back on release without following the finger.

Each front end follows the OS "reduce motion" setting live
(`NSWorkspace` on macOS, `UIAccessibility` on iOS, `prefers-reduced-motion`
in a browser); with it on every transition snaps to its end and the loops
hold still.

Both crates are vendored (`vendor/gpui-motion-kit`, `vendor/gpui-ce-motion`,
patched in through `[patch.crates-io]`, changes marked `roostr:`): as
published their clocks are `std::time::Instant`, which is not gpui's
`web_time::Instant` on wasm32, so the browser build did not compile.
`vendor/gpui-motion-kit/UPSTREAM.patch` is the fix for upstream.

## Develop

Every feature crate has an example that hosts its views against the daemon in
the environment. Develop against a throwaway vault on its own port, never your
own: `scripts/seed-test-vault.sh` fills it with sample data covering every view
(it refuses to run unless `GLON_DATA` is set and is not `~/.glon`, and reruns
add nothing).

```sh
mkdir -p /tmp/roostr-test
GLON_DATA=/tmp/roostr-test ../../glonOdin/glon-odin serve 7399 &
export GLON_DATA=/tmp/roostr-test ROOSTR_API=http://127.0.0.1:7399
scripts/seed-test-vault.sh
cargo run -p roostr-editor --example editor -- seed-page-showcase
cargo run -p roostr                      # the whole app on the test vault
```

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
- Graph: CSS blend modes and backdrop blur have no GPUI equivalent; the radial
  background is stepped.
- Inline code marks keep the block's font size and mentions use a solid
  underline (GPUI text runs have no per-run size or dotted underline).
