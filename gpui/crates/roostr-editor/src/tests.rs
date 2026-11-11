//! Headless tests of the editor in a Rune runtime over a fake vault: typing
//! saves, Enter splits, Backspace merges, the mark shortcuts, links under the
//! pointer, and long pages (ported from roostr-app's GPUI `editor_*` tests).
//! The vault answers at once; the virtual clocks (Rune's frames and
//! roostr-rt's `advance`) stand in for time, so nothing sleeps.

use std::sync::{Arc, Once};
use std::time::Duration;

use parking_lot::Mutex;
use roostr_core::State;
use roostr_core::backend::{Backend, BackendEvent, JsonMap};
use roostr_core::types::{
    Block, BlockContent, Mark, Object, ObjectSummary, QueryResult, RelationDef, Settings, Space,
    TextContent, Value, mark,
};
use roostr_rt::{BoxFuture, BoxStream, boxed, boxed_stream};
use rune::{El, Env, Key, KeyEvent, Mods, Runtime, ScrollSpec, col};
use serde_json::Value as Json;

use crate::doc::{self, Op, Place};

const SPACE: &str = "sp-1";
const PAGE: &str = "page-1";

/// One space with one page; block mutations apply to the page as the engine
/// would, and every mutation is kept.
struct Vault {
    page: Mutex<Object>,
    mutations: Mutex<Vec<(String, JsonMap)>>,
}

impl Vault {
    fn new(blocks: Vec<Block>) -> Arc<Self> {
        let mut fields = roostr_core::types::Fields::default();
        fields.insert("name".into(), Value::string("Plans"));
        fields.insert("channel".into(), Value::string(SPACE));
        Arc::new(Self {
            page: Mutex::new(Object {
                id: PAGE.into(),
                type_key: "page".into(),
                fields,
                blocks,
                ..Object::default()
            }),
            mutations: Mutex::default(),
        })
    }

    /// The text of block `id` as the vault holds it.
    fn text(&self, id: &str) -> Option<String> {
        let page = self.page.lock();
        let b = page.blocks.iter().find(|b| b.id == id)?;
        Some(b.content.text.as_ref()?.text.clone())
    }

    /// The marks `(from, to, type)` of block `id` as the vault holds them.
    fn marks(&self, id: &str) -> Vec<(u32, u32, u32)> {
        let page = self.page.lock();
        page.blocks
            .iter()
            .find(|b| b.id == id)
            .and_then(|b| b.content.text.as_ref())
            .and_then(|t| t.marks.clone())
            .unwrap_or_default()
            .into_iter()
            .map(|m| (m.from, m.to, m.kind))
            .collect()
    }

    fn apply(&self, action: &str, params: &JsonMap) {
        let str_of = |k: &str| {
            params
                .get(k)
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let pos_of = || params.get("position").and_then(Json::as_u64).unwrap_or(0) as u32;
        let op = match action {
            "block_update" => {
                let Some(content) = params
                    .get("content")
                    .and_then(|c| serde_json::from_value::<BlockContent>(c.clone()).ok())
                else {
                    return;
                };
                Op::Content {
                    id: str_of("block_id"),
                    content,
                }
            }
            "block_add" => {
                let Some(block) = params
                    .get("block")
                    .and_then(|b| serde_json::from_value::<Block>(b.clone()).ok())
                else {
                    return;
                };
                Op::Insert {
                    block,
                    at: Place::new(&str_of("target_id"), pos_of()),
                }
            }
            "block_remove" => Op::Remove {
                id: str_of("block_id"),
            },
            "block_move" => Op::Move {
                id: str_of("block_id"),
                to: Place::new(&str_of("target_id"), pos_of()),
            },
            _ => return,
        };
        doc::apply_op(&mut self.page.lock().blocks, &op);
    }
}

struct Handle(Arc<Vault>);

impl Backend for Handle {
    fn describe(&self) -> String {
        "one page".into()
    }
    fn settings(&self) -> BoxFuture<anyhow::Result<Settings>> {
        boxed(async {
            Ok(Settings {
                has_key: true,
                ..Settings::default()
            })
        })
    }
    fn objects(&self) -> BoxFuture<anyhow::Result<Vec<ObjectSummary>>> {
        boxed(async {
            Ok(vec![ObjectSummary {
                id: PAGE.into(),
                type_key: "page".into(),
                name: "Plans".into(),
                channel_id: SPACE.into(),
                updated_at: 1_000,
                ..ObjectSummary::default()
            }])
        })
    }
    fn object(&self, id: &str) -> BoxFuture<anyhow::Result<Object>> {
        let found = (id == PAGE).then(|| self.0.page.lock().clone());
        let id = id.to_string();
        boxed(async move { found.ok_or_else(|| anyhow::anyhow!("unknown object {id}")) })
    }
    fn channels(&self) -> BoxFuture<anyhow::Result<Vec<Space>>> {
        boxed(async {
            Ok(vec![Space {
                id: SPACE.into(),
                name: "Personal".into(),
                created_at: 1,
                ..Space::default()
            }])
        })
    }
    fn relations(&self) -> BoxFuture<anyhow::Result<Vec<RelationDef>>> {
        boxed(async { Ok(Vec::new()) })
    }
    fn query(&self, _: &Json) -> BoxFuture<anyhow::Result<QueryResult>> {
        boxed(async { Ok(QueryResult::default()) })
    }
    fn mutate(&self, action: &str, params: JsonMap) -> BoxFuture<anyhow::Result<JsonMap>> {
        self.0.apply(action, &params);
        self.0.mutations.lock().push((action.to_string(), params));
        boxed(async { Ok(JsonMap::new()) })
    }
    fn serving(&self, _: &[String]) -> BoxFuture<anyhow::Result<JsonMap>> {
        boxed(async { Ok(JsonMap::new()) })
    }
    fn sync_digest(&self) -> BoxFuture<anyhow::Result<JsonMap>> {
        boxed(async { Ok(JsonMap::new()) })
    }
    fn events(&self) -> BoxStream<anyhow::Result<BackendEvent>> {
        boxed_stream(futures::stream::pending())
    }
}

fn text_block(id: &str, text: &str, marks: Vec<Mark>) -> Block {
    Block {
        id: id.into(),
        content: BlockContent {
            text: Some(TextContent {
                text: text.into(),
                marks: Some(marks),
                ..TextContent::default()
            }),
            ..BlockContent::default()
        },
        ..Block::default()
    }
}

fn view(s: &State, cx: &mut rune::Cx<State>) -> El {
    let t = roostr_ui::theme(roostr_ui::ThemePref::Dark, cx);
    let mut layer = Vec::new();
    let body = crate::view(s, cx, t, &mut layer, PAGE);
    let page = col()
        .key("page-scroll")
        .w_full()
        .h_full()
        .min_h(0.)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(col().w_full().px(32.).pt(24.).child(body));
    roostr_ui::root(page, layer)
}

/// The page open in a 1400×700 window, loaded.
fn open(vault: &Arc<Vault>) -> Runtime<State> {
    static UI_STATE: Once = Once::new();
    UI_STATE.call_once(|| {
        let path =
            std::env::temp_dir().join(format!("roostr-editor-tests-{}.json", std::process::id()));
        // SAFETY: set once, before any State (the only reader) exists in this process.
        unsafe { std::env::set_var("ROOSTR_UI_STATE", path) };
    });
    let mut s = State::new(Arc::new(Handle(vault.clone())), None);
    crate::register(&mut s);
    crate::open(&mut s, PAGE, false);
    let mut rt = Runtime::new(
        s,
        view,
        (1400., 700.),
        Env {
            system_dark: true,
            scale: 2.0,
        },
    );
    settle(&mut rt);
    assert!(
        crate::editor::editor(&rt.state, PAGE).is_some_and(|e| e.object.is_some()),
        "the page loaded"
    );
    rt
}

/// Run the app's futures until nothing moves, then draw. The vault answers at
/// once, but the store reads it on roostr-rt's worker threads: when a poll
/// changed nothing, a fetch may still be on its way, so wait for its wake (a
/// worker finishes in microseconds; no wake within a few ms means none is out).
fn settle(rt: &mut Runtime<State>) {
    let (woke, wakes) = std::sync::mpsc::channel::<()>();
    let woke = Mutex::new(woke);
    rt.state.rt.set_waker(Arc::new(move || {
        let _ = woke.lock().send(());
    }));
    for _ in 0..500 {
        let ran = rt.state.poll();
        rt.frame(1. / 60.);
        if !ran.updated && wakes.recv_timeout(Duration::from_millis(15)).is_err() {
            break;
        }
        while wakes.try_recv().is_ok() {}
    }
    rt.state.poll();
    rt.frame(1. / 60.);
}

/// Let the debounced saves fire.
fn after_saves(rt: &mut Runtime<State>) {
    settle(rt);
    for _ in 0..3 {
        rt.state.rt.advance(Duration::from_secs(1));
        settle(rt);
    }
}

fn key(rt: &mut Runtime<State>, key: Key, mods: Mods) {
    rt.key(KeyEvent { key, mods });
    rt.state.poll();
    rt.frame(1. / 60.);
}

fn cmd() -> Mods {
    Mods {
        cmd: true,
        logo: cfg!(target_os = "macos"),
        ctrl: !cfg!(target_os = "macos"),
        ..Mods::default()
    }
}

fn type_text(rt: &mut Runtime<State>, text: &str) {
    for c in text.chars() {
        key(rt, Key::Text(c.to_string()), Mods::default());
    }
}

fn click(rt: &mut Runtime<State>, el_key: &str) {
    let (x, y) = rt
        .find(el_key)
        .unwrap_or_else(|| panic!("{el_key} is on screen"));
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.state.poll();
    rt.frame(1. / 60.);
}

fn live(rt: &Runtime<State>, id: &str) -> String {
    crate::editor::editor(&rt.state, PAGE)
        .and_then(|e| e.texts.get(id))
        .map(|t| t.text().to_string())
        .unwrap_or_default()
}

fn roots(rt: &Runtime<State>) -> Vec<String> {
    crate::editor::editor(&rt.state, PAGE)
        .map(|e| e.root_ids())
        .unwrap_or_default()
}

#[test]
fn typing_saves_enter_splits_and_backspace_merges() {
    let vault = Vault::new(vec![text_block("p0", "Hello world", Vec::new())]);
    let mut rt = open(&vault);
    click(&mut rt, "text-p0");
    key(&mut rt, Key::Right, cmd());
    type_text(&mut rt, " again");
    assert_eq!(live(&rt, "p0"), "Hello world again");
    after_saves(&mut rt);
    assert_eq!(
        vault.text("p0").as_deref(),
        Some("Hello world again"),
        "typing was saved"
    );

    // Enter at the end starts a new line below with the caret in it.
    key(&mut rt, Key::Enter, Mods::default());
    type_text(&mut rt, "Next");
    let ids = roots(&rt);
    assert_eq!(ids.len(), 2, "a new block: {ids:?}");
    assert_eq!(live(&rt, &ids[1]), "Next");
    after_saves(&mut rt);
    assert_eq!(vault.text(&ids[1]).as_deref(), Some("Next"));

    // Enter mid-line splits it there.
    for _ in 0.."xt".len() {
        key(&mut rt, Key::Left, Mods::default());
    }
    key(&mut rt, Key::Enter, Mods::default());
    let ids = roots(&rt);
    assert_eq!(ids.len(), 3);
    assert_eq!(
        (live(&rt, &ids[1]), live(&rt, &ids[2])),
        ("Ne".to_string(), "xt".to_string())
    );

    // Backspace at a line's start merges it into the line above, caret at the seam.
    key(&mut rt, Key::Backspace, Mods::default());
    let ids = roots(&rt);
    assert_eq!(ids.len(), 2);
    assert_eq!(live(&rt, &ids[1]), "Next");
    type_text(&mut rt, "_");
    assert_eq!(live(&rt, &ids[1]), "Ne_xt");
    after_saves(&mut rt);
    assert_eq!(
        vault.page.lock().blocks.len(),
        2,
        "the merged block is gone from the vault"
    );

    // Undo puts the typing back out; the editor's own history, not the field's.
    key(&mut rt, Key::Command('z'), cmd());
    assert_eq!(live(&rt, &ids[1]), "Next");
}

#[test]
fn mark_shortcuts_toggle_marks_and_save_them() {
    const TEXT: &str = "Strike me through";
    let vault = Vault::new(vec![text_block("p0", TEXT, Vec::new())]);
    let mut rt = open(&vault);
    click(&mut rt, "text-p0");
    // The whole line selected.
    key(&mut rt, Key::Right, cmd());
    key(
        &mut rt,
        Key::Left,
        Mods {
            shift: true,
            ..cmd()
        },
    );
    key(
        &mut rt,
        Key::Command('s'),
        Mods {
            shift: true,
            ..cmd()
        },
    );
    after_saves(&mut rt);
    let len = TEXT.len() as u32;
    assert_eq!(
        vault.marks("p0"),
        [(0, len, mark::STRIKETHROUGH)],
        "⌘⇧S struck it through"
    );
    // Applying a mark leaves the caret at its end (as ⌘B does); selected
    // again, ⌘⇧S takes it off.
    key(
        &mut rt,
        Key::Left,
        Mods {
            shift: true,
            ..cmd()
        },
    );
    key(
        &mut rt,
        Key::Command('s'),
        Mods {
            shift: true,
            ..cmd()
        },
    );
    after_saves(&mut rt);
    assert_eq!(vault.marks("p0"), [], "a second ⌘⇧S took it off");

    // ⌘B bolds "Strike", and typing at its end extends it.
    key(&mut rt, Key::Left, cmd());
    for _ in 0.."Strike".len() {
        key(
            &mut rt,
            Key::Right,
            Mods {
                shift: true,
                ..Mods::default()
            },
        );
    }
    key(&mut rt, Key::Command('b'), cmd());
    type_text(&mut rt, "s");
    after_saves(&mut rt);
    assert_eq!(vault.text("p0").as_deref(), Some("Strikes me through"));
    assert_eq!(vault.marks("p0"), [(0, 7, mark::BOLD)]);
    // The field strikes nothing through and draws the bold through its paints.
    let e = crate::editor::editor(&rt.state, PAGE).unwrap();
    assert!(e.texts["p0"].edit.marks().is_empty());
}

#[test]
fn a_long_page_builds_only_the_blocks_near_the_window() {
    let blocks = (0..300)
        .map(|i| text_block(&format!("p{i}"), &format!("p{i}"), Vec::new()))
        .collect();
    let vault = Vault::new(blocks);
    let mut rt = open(&vault);
    // Heights are learnt from the first layout; the next frames window the page.
    settle(&mut rt);
    assert!(rt.find("block-p0").is_some(), "the first block");
    assert!(
        rt.find("block-p299").is_none(),
        "the last block, far below, isn't built"
    );
    assert!(rt.find("block-p50").is_none(), "nor is block 50");

    // Typing in a block saves it.
    click(&mut rt, "text-p0");
    key(&mut rt, Key::Right, cmd());
    type_text(&mut rt, " ZZ");
    after_saves(&mut rt);
    assert_eq!(
        vault.text("p0").as_deref(),
        Some("p0 ZZ"),
        "the typing was saved on p0"
    );

    // Down past every block that was built, then type: it lands further on.
    // Each one-line block takes two: to the line's end, then on to the next block.
    for _ in 0..120 {
        key(&mut rt, Key::Down, Mods::default());
        settle(&mut rt);
    }
    type_text(&mut rt, " YY");
    after_saves(&mut rt);
    let landed: Vec<String> = vault
        .page
        .lock()
        .blocks
        .iter()
        .filter_map(|b| b.content.text.as_ref().map(|t| t.text.clone()))
        .filter(|t| t.ends_with(" YY"))
        .collect();
    assert_eq!(landed.len(), 1, "typed once: {landed:?}");
    let n: usize = landed[0]
        .trim_start_matches('p')
        .trim_end_matches(" YY")
        .parse()
        .unwrap();
    assert!(
        n >= 50,
        "the caret moved past the blocks first built: typed in p{n}"
    );
    assert!(
        rt.find(&format!("block-p{n}")).is_some(),
        "and that block is on screen now"
    );
}

#[test]
fn slash_opens_the_menu_and_picks_a_style() {
    let vault = Vault::new(vec![text_block("p0", "", Vec::new())]);
    let mut rt = open(&vault);
    click(&mut rt, "text-p0");
    type_text(&mut rt, "/");
    assert!(
        crate::editor::editor(&rt.state, PAGE)
            .unwrap()
            .slash
            .is_some(),
        "\"/\" opened the menu"
    );
    rt.frame(1. / 60.);
    assert!(rt.find("slash-menu").is_some(), "the menu is drawn");
    type_text(&mut rt, "head");
    key(&mut rt, Key::Enter, Mods::default());
    settle(&mut rt);
    let page = vault.page.lock().clone();
    let b = page.blocks.iter().find(|b| b.id == "p0").unwrap();
    let t = b.content.text.as_ref().unwrap();
    assert_eq!(t.text, "", "the /filter is gone");
    assert_eq!(
        t.style,
        roostr_core::types::style::HEADER1,
        "the line is a heading"
    );
}

#[test]
fn three_dashes_and_a_space_make_a_divider() {
    let vault = Vault::new(vec![text_block("p0", "", Vec::new())]);
    let mut rt = open(&vault);
    click(&mut rt, "text-p0");
    type_text(&mut rt, "--- ");
    settle(&mut rt);
    let page = vault.page.lock().clone();
    let b = page.blocks.iter().find(|b| b.id == "p0").unwrap();
    assert_eq!(
        b.content.custom.as_ref().map(|c| c.content_type.as_str()),
        Some("divider")
    );
    assert_eq!(page.blocks.len(), 2, "a fresh line below takes the caret");
}

#[test]
fn the_paste_as_menu_opens_under_the_block_pasted_into() {
    let blocks = (0..6)
        .map(|n| text_block(&format!("p{n}"), &format!("Line {n}"), Vec::new()))
        .chain([text_block("p6", "", Vec::new())])
        .collect();
    let vault = Vault::new(blocks);
    let mut rt = open(&vault);
    click(&mut rt, "text-p6");
    rt.paste_content(rune::ClipboardContent::new("https://www.twingate.com/"));
    settle(&mut rt);
    rt.frame(1. / 60.);
    let block = rt.presented("block-p6").map(|p| p.0).expect("the block pasted into");
    let item = rt.presented("paste-link").map(|p| p.0).expect("the Paste as menu is open");
    assert!(
        item[1] > block[1] && item[1] < block[1] + block[3] + 140.,
        "the menu sits under its block: menu item at {item:?}, block at {block:?}"
    );
    assert!(item[0] >= block[0] - 1., "and not left of the page: {item:?} vs {block:?}");
}

/// A web link in a page's text opens in the browser when clicked. Field
/// report: links in a page body did nothing.
#[test]
fn a_link_in_page_text_opens_on_click() {
    const URL: &str = "https://example.com/page-link";
    let link = Mark { from: 6, to: 13, kind: mark::LINK, param: Some(URL.into()) };
    let vault = Vault::new(vec![text_block("p0", "Visit example now", vec![link])]);
    let mut rt = open(&vault);
    let (x, y) = rt.find(URL).expect("the link is drawn");
    let (x, y) = (x + 2., y + 2.);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.state.poll();
    rt.frame(1. / 60.);
    assert_eq!(rt.take_url_requests(), vec![URL.to_string()], "the click opened nothing");
}
