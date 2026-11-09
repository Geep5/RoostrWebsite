//! The data view of an object id (query/set, collection or type) under a bar
//! of the vault's set objects that switches between them, on the daemon named
//! by the environment.
//!
//! `cargo run -p roostr-sets --example sets -- [object-id] [--snap out.png --size WxH]`

use roostr_core::{State, api, field_str};
use roostr_sets::DataViewKind;
use roostr_ui::{ThemePref, ty};
use rune::{Cursor, Cx, Drag, El, ScrollSpec, Tx, col, el, row, text};
use serde_json::json;

/// The picker's state: the vault's sets and the one shown.
#[derive(Default)]
struct Picker {
    sets: Vec<(String, String, String)>,
    current: Option<(String, DataViewKind)>,
}

fn kind_of(type_key: &str) -> DataViewKind {
    match type_key {
        "collection" => DataViewKind::Collection,
        "type" => DataViewKind::Type,
        _ => DataViewKind::Query,
    }
}

fn open(s: &mut State, id: String, type_key: &str) {
    let kind = kind_of(type_key);
    if let Some((old, _)) = s.slot::<Picker>().current.take() {
        roostr_sets::close(s, &old);
    }
    roostr_sets::open(s, &id, kind);
    s.slot::<Picker>().current = Some((id, kind));
}

fn register(s: &mut State) {
    roostr_sets::register(s);
    roostr_properties::register(s);
    roostr_host::wait_loaded(s, std::time::Duration::from_secs(20));
    if let Some(id) = rune::extra_args()
        .iter()
        .find(|a| !a.starts_with('-'))
        .cloned()
    {
        // The kind comes from the object once it is fetched.
        let fetch = s.store.fetch_object(&id);
        s.rt.spawn(async move |cx| {
            let type_key = fetch
                .await
                .map(|o| o.type_key)
                .unwrap_or_else(|_| "query".into());
            cx.update(|s| open(s, id, &type_key));
        })
        .detach();
    }
    let task = api::query(
        json!({"filters": [{"key": "typeKey", "condition": "in", "value": ["query", "set", "collection"]}], "fields": ["name", "channel"]}),
        s,
    );
    s.rt.spawn(async move |cx| {
        let Ok(res) = task.await else { return };
        let mut sets: Vec<(String, String, String)> = res
            .records
            .iter()
            .map(|r| {
                (
                    r.id.clone(),
                    r.type_key.clone(),
                    field_str(&r.fields, "name").to_string(),
                )
            })
            .collect();
        sets.sort_by(|a, b| a.2.cmp(&b.2).then(a.0.cmp(&b.0)));
        cx.update(|s| {
            let first = sets.first().cloned();
            let p = s.slot::<Picker>();
            p.sets = sets;
            if p.current.is_none()
                && rune::extra_args().iter().all(|a| a.starts_with('-'))
                && let Some((id, kind, _)) = first
            {
                open(s, id, &kind);
            }
        });
    })
    .detach();
    // A headless `--snap` shows the records: run the loads the view starts to quiet.
    roostr_host::settle(s, std::time::Duration::from_secs(20));
}

fn view(s: &State, cx: &mut Cx<State>, layer: &mut Vec<El>) -> El {
    let t = roostr_ui::theme(ThemePref::Dark, cx);
    let Some(p) = s.get::<Picker>() else {
        return el();
    };
    let current = p.current.as_ref().map(|c| c.0.as_str()).unwrap_or_default();
    let mut bar = row()
        .flex_wrap()
        .gap(6.0)
        .px(32.0)
        .pt(12.0)
        .pb(8.0)
        .border_sides([0., 0., 1., 0.], t.border)
        .border_layout();
    for (id, kind, name) in &p.sets {
        let on = id == current;
        let (pick, k) = (id.clone(), kind.clone());
        bar = bar.child(
            row()
                .key(format!("pick-{id}"))
                .px(10.0)
                .py(4.0)
                .radius(999.0)
                .border(1.0, if on { t.accent } else { t.border })
                .cursor(Cursor::Pointer)
                .child(
                    text(format!("{name} · {kind}"))
                        .font(ty::ui(12.0))
                        .color(t.fg),
                )
                .on_press(move |s: &mut State| open(s, pick.clone(), &k)),
        );
    }
    let mut page = col()
        .key("page")
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .px(32.0)
        .pt(24.0)
        .pb(80.0);
    if let Some((id, kind)) = p.current.clone() {
        let title = s
            .store
            .object(&id)
            .map(|o| o.name().to_string())
            .unwrap_or_default();
        let drag = id.clone();
        page =
            page
                // The page around the data view arms its rubber band too (as the object page should).
                .on_drag(move |s: &mut State, d: &mut Drag, tx: &mut Tx| {
                    roostr_sets::page_drag(s, &drag, d, tx)
                })
                .child(el().pl(48.0).pb(16.0).child(text(title).color(t.fg).font(
                    rune::TextStyle {
                        weight: 750.0,
                        ..ty::ui(34.0)
                    },
                )))
                .child(
                    el().w_full()
                        .pl(48.0)
                        .child(roostr_sets::view(s, cx, t, layer, &id, kind)),
                );
    }
    col().w_full().h_full().child(bar).child(page)
}

fn main() {
    roostr_host::run_example_layered("Roostr sets", register, view);
}
