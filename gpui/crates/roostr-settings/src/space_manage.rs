//! Space settings on a space (`channel`) object's page (`SpaceManage.svelte`):
//! default type, members, join link + requests, key rotation, the bin, and
//! leaving / deleting the space.

use std::collections::HashMap;
use std::time::Duration;

use roostr_core::create::{FALLBACK_TYPE, NOT_DEFAULTABLE};
use roostr_core::dates::show_date;
use roostr_core::invite::{SpaceJoinLink, join_url};
use roostr_core::objects::{object_icon, type_glyph};
use roostr_core::store::Store;
use roostr_core::types::field_str;
use roostr_core::{Route, State, StoreEvent, Value, api, nav};
use roostr_ui::ty::Weights as _;
use roostr_ui::{
    Avatar, Dialog, DialogEntrance, Images, InputState, KitSize, SelectState, Theme,
    checkbox_display, motion, ty,
};
use rune::{Effect, El, Spring, SpringPair, Transition, col, el, row, text};
use serde_json::json;

use crate::local::{JoinRequest, clear_join_request, list_join_requests};
use crate::widgets::{Btn, Field, FieldStyle, SelectOption, Span, rich, select, sync_select};

/// `binBusy` while the whole bin is being emptied.
const ALL: &str = "__all__";

struct BinRow {
    id: String,
    name: String,
    type_key: String,
    icon: String,
    done: bool,
}

/// Every opened space's settings section, by space id.
#[derive(Default)]
pub(crate) struct SpaceManages {
    by_id: HashMap<String, SpaceManageState>,
}

/// The settings section of one space object's page.
#[derive(Default)]
struct SpaceManageState {
    /// `None` until the first read of the bin.
    bin: Option<Vec<BinRow>>,
    /// The bin row being restored/deleted, [`ALL`] while emptying, else "".
    bin_busy: String,
    confirm_delete: bool,
    deleting: bool,
    confirm_empty: bool,
    /// The bin row whose "Delete forever" asks first: (id, name).
    confirm_vanish: Option<(String, String)>,
    empty_draft: InputState,
    delete_draft: InputState,
    /// The delete field's placeholder: the phrase, kept in step with the name.
    delete_placeholder: String,
    npub_draft: InputState,
    /// First click on Remove arms it, the second rotates the key.
    confirm_remove: String,
    copied_join: bool,
    /// The npub this view last added: its row rises in when it arrives
    /// (rows from sync just appear).
    added_member: Option<String>,
    join_requests: Vec<JoinRequest>,
    identity_error: String,
    default_type: SelectState,
    /// The window width as last laid out (root's right edge plus its margin).
    width: f32,
}

fn entry<'a>(s: &'a mut State, id: &str) -> &'a mut SpaceManageState {
    s.slot::<SpaceManages>()
        .by_id
        .entry(id.to_owned())
        .or_default()
}

/// Runs `f` on the space's entry if it is still open.
fn with<R>(s: &mut State, id: &str, f: impl FnOnce(&mut SpaceManageState) -> R) -> Option<R> {
    s.slot::<SpaceManages>().by_id.get_mut(id).map(f)
}

/// Opens the section for `space_id` (idempotent): creates its state and
/// starts reading the bin and the join requests.
pub fn open_space_manage(s: &mut State, space_id: &str) {
    if s.slot::<SpaceManages>().by_id.contains_key(space_id) {
        return;
    }
    s.store.ensure_object(space_id);
    entry(s, space_id);
    sync_type_select(s, space_id);
    load_bin(s, space_id);
    load_identity(s, space_id);
}

/// Store listener: the space's name feeds the delete phrase, and the
/// defaultable types follow the store.
pub(crate) fn on_store(s: &mut State, event: &StoreEvent) {
    let ids: Vec<String> = match s.get::<SpaceManages>() {
        Some(m) => m.by_id.keys().cloned().collect(),
        None => return,
    };
    for id in ids {
        if matches!(event, StoreEvent::Committed(c) if *c == id) {
            sync_delete_placeholder(s, &id);
        }
        sync_type_select(s, &id);
    }
}

// ------------------------------------------------------------- reads

/// `SpaceJSON.owner` is "" when this identity administers the space.
fn is_owner(store: &Store, id: &str) -> bool {
    store.space(id).is_none_or(|sp| sp.owner.is_empty())
}

fn space_name(store: &Store, id: &str) -> String {
    store
        .object(id)
        .map(|o| o.field_str("name").trim().to_string())
        .unwrap_or_default()
}

/// The space's own name, not the word "delete": the gate should cost a
/// look at WHICH space this is. A space with no name keeps the action's word.
fn phrase_for(name: &str, owner: bool) -> String {
    if !name.is_empty() {
        name.to_string()
    } else if owner {
        "delete".into()
    } else {
        "leave".into()
    }
}

fn delete_phrase(store: &Store, id: &str) -> String {
    phrase_for(&space_name(store, id), is_owner(store, id))
}

/// The typed draft matches the phrase (trimmed, case-insensitive).
fn phrase_matches(draft: &str, phrase: &str) -> bool {
    draft.trim().to_lowercase() == phrase.to_lowercase()
}

fn delete_armed(s: &State, m: &SpaceManageState, id: &str) -> bool {
    phrase_matches(m.delete_draft.value(), &delete_phrase(&s.store, id))
}

fn empty_armed(m: &SpaceManageState) -> bool {
    phrase_matches(m.empty_draft.value(), "delete")
}

fn owner_npub(store: &Store) -> String {
    store
        .settings
        .as_ref()
        .and_then(|s| s.npub.clone())
        .unwrap_or_default()
}

fn sync_delete_placeholder(s: &mut State, id: &str) {
    let phrase = delete_phrase(&s.store, id);
    with(s, id, |m| {
        if m.confirm_delete {
            m.delete_placeholder = phrase;
        }
    });
}

/// The default type's options: the space's defaultable types by name, each
/// as "<icon> <name>", and the current choice.
fn type_options(store: &Store, id: &str) -> (Vec<SelectOption>, String) {
    let default_type = store
        .object(id)
        .map(|o| o.field_str("default_type").to_string())
        .filter(|k| !k.is_empty())
        .unwrap_or_else(|| FALLBACK_TYPE.to_string());
    let mut defaultable: Vec<_> = store
        .types
        .iter()
        .filter(|t| t.space == id && !NOT_DEFAULTABLE.contains(&t.key.as_str()))
        .collect();
    defaultable.sort_by(|a, b| {
        let an = if a.name.is_empty() { &a.key } else { &a.name };
        let bn = if b.name.is_empty() { &b.key } else { &b.name };
        an.to_lowercase().cmp(&bn.to_lowercase())
    });
    let options = defaultable
        .iter()
        .map(|t| {
            let icon = if t.icon.is_empty() {
                type_glyph(store, &t.key, id)
            } else {
                t.icon.clone()
            };
            let name = if t.name.is_empty() { &t.key } else { &t.name };
            (t.key.clone(), format!("{icon} {name}"))
        })
        .collect();
    (options, default_type)
}

fn sync_type_select(s: &mut State, id: &str) {
    let (options, selected) = type_options(&s.store, id);
    with(s, id, |m| {
        sync_select(&mut m.default_type, &options, &selected)
    });
}

// ------------------------------------------------------------- bin

/// Deleted objects of this space, read to exhaustion: "Empty bin" vanishes
/// exactly this list, so a capped read would delete a page and then
/// report the bin emptied.
fn load_bin(s: &mut State, id: &str) {
    let task = api::query_all(
        json!({ "includeDeleted": true, "filters": [{ "key": "channel", "condition": "equal", "value": id }] }),
        s,
    );
    let id = id.to_owned();
    s.rt.spawn(async move |cx| {
        let result = task.await;
        cx.update(|s| match result {
            Ok(records) => {
                with(s, &id, |m| {
                    m.bin = Some(
                        records
                            .into_iter()
                            .filter(|r| r.deleted == Some(true))
                            .map(|r| BinRow {
                                name: Some(field_str(&r.fields, "name"))
                                    .filter(|s| !s.is_empty())
                                    .unwrap_or("Untitled")
                                    .to_string(),
                                icon: field_str(&r.fields, "iconEmoji").to_string(),
                                done: r.fields.get("done").and_then(|v| v.bool_value) == Some(true),
                                type_key: r.type_key,
                                id: r.id,
                            })
                            .collect(),
                    );
                });
            }
            Err(error) => log::warn!("bin read failed: {error:#}"),
        });
    })
    .detach();
}

fn restore_object(s: &mut State, space: &str, id: String) {
    let Some(()) = with(s, space, |m| m.bin_busy = id.clone()) else {
        return;
    };
    let task = api::note::restore(&id, s);
    let space = space.to_owned();
    s.rt.spawn(async move |cx| {
        let result = task.await;
        cx.update(|s| {
            if let Err(error) = result {
                log::warn!("restore failed: {error:#}");
            }
            if with(s, &space, |m| m.bin_busy.clear()).is_some() {
                load_bin(s, &space);
                s.store.refresh_all();
            }
        });
    })
    .detach();
}

/// Empty bin: vanish every deleted object in this space at once.
fn empty_bin(s: &mut State, space: &str) {
    let Some(ids) = with(s, space, |m| {
        m.bin
            .as_ref()
            .map(|b| b.iter().map(|r| r.id.clone()).collect::<Vec<_>>())
    })
    .flatten() else {
        return;
    };
    if ids.is_empty() {
        return;
    }
    with(s, space, |m| m.bin_busy = ALL.into());
    let task = api::note::vanish(&ids, s);
    let space = space.to_owned();
    s.rt.spawn(async move |cx| {
        let result = task.await;
        cx.update(|s| {
            if let Err(error) = result {
                log::warn!("empty bin failed: {error:#}");
            }
            // Escape may already have closed the dialog while the vanish ran.
            with(s, &space, |m| {
                m.bin = Some(Vec::new());
                m.confirm_empty = false;
                m.bin_busy.clear();
            });
        });
    })
    .detach();
}

/// The "Permanently delete …?" question was answered OK.
fn vanish_object(s: &mut State, space: &str) {
    let Some(Some((id, _))) = with(s, space, |m| m.confirm_vanish.take()) else {
        return;
    };
    with(s, space, |m| m.bin_busy = id.clone());
    let task = api::note::vanish(std::slice::from_ref(&id), s);
    let space = space.to_owned();
    s.rt.spawn(async move |cx| {
        if let Err(error) = task.await {
            log::warn!("vanish failed: {error:#}");
        }
        cx.update(|s| {
            if with(s, &space, |m| m.bin_busy.clear()).is_some() {
                load_bin(s, &space);
            }
        });
    })
    .detach();
}

// ------------------------------------------------------------- members

fn load_identity(s: &mut State, space: &str) {
    load_join_requests(s, space);
}

fn load_join_requests(s: &mut State, space: &str) {
    let task = list_join_requests(s);
    let space = space.to_owned();
    s.rt.spawn(async move |cx| {
        let result = task.await;
        cx.update(|s| {
            with(s, &space, |m| match result {
                Ok(requests) => {
                    m.join_requests = requests.into_iter().filter(|r| r.space == space).collect();
                    m.identity_error.clear();
                }
                Err(error) => {
                    m.join_requests.clear();
                    m.identity_error = error.to_string();
                }
            });
        });
    })
    .detach();
}

fn copy_join_link(s: &mut State, tx: &mut rune::Tx, space: &str) {
    let owner = owner_npub(&s.store);
    if owner.is_empty() {
        return;
    }
    let name = s
        .store
        .object(space)
        .map(|o| o.field_str("name").to_string())
        .filter(|n| !n.is_empty());
    let relays = s
        .store
        .settings
        .as_ref()
        .map(|s| s.relays.clone())
        .unwrap_or_default();
    let url = join_url(&SpaceJoinLink {
        space: space.to_owned(),
        name,
        owner,
        relays,
    });
    tx.copy(url);
    with(s, space, |m| m.copied_join = true);
    let space = space.to_owned();
    s.rt.spawn(async move |cx| {
        cx.timer(Duration::from_millis(1500)).await;
        cx.update(|s| with(s, &space, |m| m.copied_join = false));
    })
    .detach();
}

fn approve_request(s: &mut State, space: &str, request: JoinRequest) {
    let add = api::space::member_add(space, &request.requester_npub, None, s);
    let space = space.to_owned();
    s.rt.spawn(async move |cx| {
        let result = async {
            add.await?;
            cx.update(|s| clear_join_request(&request.key, s)).await
        }
        .await;
        cx.update(|s| match result {
            Ok(()) => {
                if s.slot::<SpaceManages>().by_id.contains_key(&space) {
                    load_join_requests(s, &space);
                    s.store.refresh_all();
                }
            }
            Err(error) => {
                with(s, &space, |m| m.identity_error = error.to_string());
            }
        });
    })
    .detach();
}

fn deny_request(s: &mut State, space: &str, request: JoinRequest) {
    let task = clear_join_request(&request.key, s);
    let space = space.to_owned();
    s.rt.spawn(async move |cx| {
        let result = task.await;
        cx.update(|s| match result {
            Ok(()) => {
                if s.slot::<SpaceManages>().by_id.contains_key(&space) {
                    load_join_requests(s, &space);
                }
            }
            Err(error) => {
                with(s, &space, |m| m.identity_error = error.to_string());
            }
        });
    })
    .detach();
}

fn add_member(s: &mut State, space: &str) {
    let npub = entry(s, space).npub_draft.value().trim().to_string();
    if npub.is_empty() {
        return;
    }
    entry(s, space).added_member = Some(npub.clone());
    let task = api::space::member_add(space, &npub, None, s);
    let space = space.to_owned();
    s.rt.spawn(async move |cx| match task.await {
        Ok(_) => cx.update(|s| {
            with(s, &space, |m| m.npub_draft.set_value(""));
            s.store.refresh_all();
        }),
        Err(error) => log::warn!("add member failed: {error:#}"),
    })
    .detach();
}

fn remove_member(s: &mut State, space: &str, npub: String) {
    let m = entry(s, space);
    if m.confirm_remove != npub {
        // First click arms, second executes: the key rotates and the member loses future access.
        m.confirm_remove = npub;
        return;
    }
    m.confirm_remove.clear();
    let task = api::space::member_remove(space, &npub, s);
    s.rt.spawn(async move |cx| match task.await {
        Ok(_) => cx.update(|s| s.store.refresh_all()),
        Err(error) => log::warn!("remove member failed: {error:#}"),
    })
    .detach();
}

fn rotate_key(s: &mut State, space: &str) {
    let task = api::space::key_rotate(space, s);
    s.rt.spawn(async move |cx| match task.await {
        Ok(_) => cx.update(|s| s.store.refresh_all()),
        Err(error) => log::warn!("key rotate failed: {error:#}"),
    })
    .detach();
}

fn set_default_type(s: &mut State, space: &str, key: &str) {
    let task = api::note::set_field(space, "default_type", Value::string(key), s);
    s.rt.spawn(async move |_| {
        if let Err(error) = task.await {
            log::error!("{error:#}");
        }
    })
    .detach();
}

// ------------------------------------------------------------- delete / leave

fn open_delete(s: &mut State, space: &str) {
    let phrase = delete_phrase(&s.store, space);
    let m = entry(s, space);
    m.confirm_delete = true;
    m.delete_draft.set_value("");
    m.delete_placeholder = phrase;
    m.delete_draft.focus();
}

fn open_empty(s: &mut State, space: &str) {
    let m = entry(s, space);
    m.confirm_empty = true;
    m.empty_draft.set_value("");
}

/// Owner: vanish the space and every object in it (the ledger entry rides
/// sync, so it is gone everywhere and can't resurrect). Member: leave —
/// only this identity's copies go.
fn remove_space(s: &mut State, space: &str) {
    entry(s, space).deleting = true;
    let id = space.to_owned();
    let owner = is_owner(&s.store, space);
    s.rt.spawn(async move |cx| {
        let result: anyhow::Result<()> = async {
            if owner {
                // Every object, not a page of them: anything left behind is unreachable.
                let records = cx
                    .update(|s| {
                        api::query_all(
                            json!({ "filters": [{ "key": "channel", "condition": "equal", "value": id }] }),
                            s,
                        )
                    })
                    .await?;
                let mut ids = vec![id.clone()];
                ids.extend(records.into_iter().map(|r| r.id));
                cx.update(|s| api::note::vanish(&ids, s)).await?;
            } else {
                cx.update(|s| api::space::leave(&id, s)).await?;
            }
            Ok(())
        }
        .await;
        cx.update(|s| {
            if with(s, &id, |m| m.deleting = false).is_none() {
                return;
            }
            match result {
                Ok(()) => {
                    with(s, &id, |m| m.confirm_delete = false);
                    s.store.refresh_all();
                    s.nav.forget(&id, &mut s.ui);
                    // Switching space restores that space's tabs; with none left, home.
                    let next = s
                        .store
                        .channels
                        .iter()
                        .find(|c| c.id != id)
                        .map(|c| c.id.clone());
                    match next {
                        Some(next) => s.nav.use_space(&next, &mut s.ui),
                        None => nav::navigate(Route::Home, s),
                    }
                }
                Err(error) => log::warn!("removing the space failed: {error:#}"),
            }
        });
    })
    .detach();
}

/// Escape, a backdrop press and Cancel all land here: the empty-bin
/// confirmation always closes, the delete flow stays put once the vanish
/// is in flight.
fn dismiss_confirm(s: &mut State, space: &str) {
    with(s, space, |m| {
        if m.confirm_empty {
            m.confirm_empty = false;
        } else if m.confirm_delete {
            m.confirm_delete = m.deleting;
        }
    });
}

fn delete_input_style(t: &Theme) -> FieldStyle {
    FieldStyle {
        bg: t.bg,
        radius: 6.,
        px: 10.,
        py: 7.,
        size: 14.,
        focus: t.red,
        ring: Some(t.red.alpha(0.25)),
        centered: false,
    }
}

// ------------------------------------------------------------- view

fn h3(t: &Theme, s: &str) -> El {
    text(s.to_uppercase())
        .font(ty::ui(11.0).lh(14.0).weight(ty::BOLD))
        .color(t.muted)
        .mt(14.0)
        .mb(2.0)
}

fn manage_hint(t: &Theme, s: impl Into<String>) -> El {
    text(s.into())
        .font(ty::ui(12.0).lh(15.0))
        .color(t.muted)
        .wrap()
        .max_w(560.0)
        .min_w(0.0)
        .shrink()
}

fn short_npub(npub: &str) -> String {
    format!("{}…", npub.chars().take(20).collect::<String>())
}

/// "Type **<phrase>** to confirm."
fn type_to_confirm(t: &Theme, phrase: &str) -> El {
    rich(
        t,
        &[
            Span::Plain("Type "),
            Span::Bold(phrase),
            Span::Plain(" to confirm."),
        ],
        t.muted,
        12.0,
        15.0,
    )
}

/// A member row you just added rises in (opacity, y 6 → 0, 150ms).
fn rise_in() -> Transition {
    Transition {
        insertion: Effect {
            offset: rune::Vec2::new(0.0, 6.0),
            ..Effect::opacity(0.0)
        },
        springs: SpringPair::new(
            motion::CONTROL.spring(),
            Spring::new(motion::CONTROL.duration, 0.0),
        ),
        ..Transition::identity()
    }
}

/// The section for `space_id`: empty until [`open_space_manage`] ran and the
/// object loaded.
pub fn space_manage(
    s: &State,
    _cx: &mut rune::Cx<State>,
    t: &Theme,
    layer: &mut Vec<El>,
    space_id: &str,
) -> El {
    let Some(m) = s.get::<SpaceManages>().and_then(|x| x.by_id.get(space_id)) else {
        return el();
    };
    let store = &s.store;
    let Some(_object) = store.object(space_id) else {
        return el();
    };
    let id = space_id.to_owned();
    let space = store.space(space_id).cloned();
    let owner = space.as_ref().is_none_or(|sp| sp.owner.is_empty());
    let members = space
        .as_ref()
        .map(|sp| sp.members.clone())
        .unwrap_or_default();
    let channel_count = store.channels.len();
    let owner_npub = owner_npub(store);
    let key_id = space
        .as_ref()
        .map_or("?".to_string(), |sp| sp.key_id.to_string());

    // `.manage`: `margin: 8px 24px 24px 48px`, `8px 16px 24px` at ≤720px.
    let mobile = m.width > 0.0 && roostr_core::layout::is_mobile(m.width);
    let right = if mobile { 16.0 } else { 24.0 };
    let mut root = col()
        .key(format!("sm-{id}"))
        .gap(10.0)
        .mt(8.0)
        .mb(24.0)
        .mr(right)
        .ml(if mobile { 16.0 } else { 48.0 })
        .on_layout({
            let id = id.clone();
            move |s: &mut State, r: [f32; 4]| {
                let w = r[0] + r[2] + right;
                with(s, &id, |m| {
                    if (m.width - w).abs() > 0.5 {
                        m.width = w;
                    }
                });
            }
        });

    // ── Default type
    root = root
        .child(h3(t, "Default type"))
        .child(manage_hint(
            t,
            "What a new object becomes when Roostr doesn't ask: search's \"Create object\", a collection's New, and the first choice in the New menu.",
        ))
        .child(row().child(select(
            t,
            layer,
            &format!("sm-default-type-{id}"),
            &m.default_type,
            {
                let id = id.clone();
                move |s: &mut State| &mut entry(s, &id).default_type
            },
            220.0,
            t.panel,
            {
                let id = id.clone();
                move |s: &mut State, key: &str, _: &mut rune::Tx| set_default_type(s, &id, key)
            },
        )));

    // ── Members
    root = root.child(h3(t, "Members")).child(manage_hint(
        t,
        "Everyone holding this space's key can read and write every object in it. Adding an npub gift-wraps the space key to it over your relays (NIP-59) — nothing to copy, their device just receives the space. Removing a member rotates the key.",
    ));
    for mem in &members {
        let armed = m.confirm_remove == mem.npub;
        let npub = mem.npub.clone();
        let sid = id.clone();
        let mut r = row()
            .key(format!("sm-member-{id}-{}", mem.npub))
            .center_y()
            .gap(10.0)
            .child(roostr_ui::tooltip(
                text(short_npub(&mem.npub)).font(ty::mono(13.0)).color(t.fg),
                mem.npub.clone(),
            ))
            .child(text(mem.role.clone()).font(ty::ui(12.0)).color(t.muted))
            .child(
                Btn::manage(
                    t,
                    format!("sm-remove-{id}-{}", mem.npub),
                    if armed {
                        "Confirm: rotate key"
                    } else {
                        "Remove"
                    },
                )
                .danger_hover()
                .on_press(move |s: &mut State| remove_member(s, &sid, npub.clone()))
                .build(t),
            );
        if m.added_member.as_deref() == Some(mem.npub.as_str()) {
            r = r.transition(rise_in());
        }
        root = root.child(r);
    }
    if members.is_empty() {
        root = root.child(manage_hint(t, "No members yet — it's just you."));
    }
    let npub_field = Field::new(
        format!("sm-npub-{id}"),
        &m.npub_draft,
        FieldStyle {
            bg: t.panel,
            radius: 7.,
            px: 10.,
            py: 5.,
            size: 13.,
            focus: t.accent,
            ring: None,
            centered: false,
        },
    )
    .placeholder("npub1… of the person to invite")
    .on_edit({
        let id = id.clone();
        move |s: &mut State| &mut entry(s, &id).npub_draft
    })
    .on_submit({
        let id = id.clone();
        move |s: &mut State, _| add_member(s, &id)
    })
    .build(t)
    .w(340.0);
    root = root
        .child(
            row()
                .gap(8.0)
                .child(npub_field)
                .child(
                    Btn::manage(t, format!("sm-add-member-{id}"), "Add member")
                        .on_press({
                            let id = id.clone();
                            move |s: &mut State| add_member(s, &id)
                        })
                        .build(t),
                )
                .child(
                    Btn::manage(
                        t,
                        format!("sm-copy-join-{id}"),
                        if m.copied_join { "Copied ✓" } else { "Copy join link" },
                    )
                    .disabled(owner_npub.is_empty())
                    .tooltip(if owner_npub.is_empty() {
                        "No key on this device yet"
                    } else {
                        "Copy a public link that lets anyone REQUEST to join — you approve each request here"
                    })
                    .on_press_tx({
                        let id = id.clone();
                        move |s: &mut State, tx| copy_join_link(s, tx, &id)
                    })
                    .build(t),
                ),
        )
        .child(manage_hint(
            t,
            "The join link carries no key — it only lets someone knock. You approve each request below.",
        ));
    if !m.identity_error.is_empty() {
        root = root.child(manage_hint(t, m.identity_error.clone())).child(
            Btn::manage(t, format!("sm-retry-join-{id}"), "Retry join requests")
                .full_width()
                .on_press({
                    let id = id.clone();
                    move |s: &mut State| load_identity(s, &id)
                })
                .build(t),
        );
    }

    // ── Join requests
    if !m.join_requests.is_empty() {
        root = root.child(h3(t, "Join requests"));
        let images = s.get::<Images>();
        for r in &m.join_requests {
            let mut avatar = Avatar::new().size(KitSize::Px(28.0));
            if let Some(img) = r
                .picture
                .as_deref()
                .filter(|p| !p.is_empty())
                .zip(images)
                .and_then(|(p, images)| images.image(p, 28.0))
            {
                avatar = avatar.src(img.pixels().clone());
            }
            let name = r.name.clone().filter(|n| !n.is_empty());
            let mut who = col().grow(1.0).min_w(0.0).gap(1.0).child(
                text(
                    name.clone()
                        .unwrap_or_else(|| short_npub(&r.requester_npub)),
                )
                .font(ty::ui(13.0).weight(ty::MEDIUM))
                .color(t.fg)
                .truncate(),
            );
            if name.is_some() {
                who = who.child(roostr_ui::tooltip(
                    text(short_npub(&r.requester_npub))
                        .font(ty::mono(11.0))
                        .color(t.muted),
                    r.requester_npub.clone(),
                ));
            }
            let (approve, deny) = (r.clone(), r.clone());
            root = root.child(
                row()
                    .center_y()
                    .gap(10.0)
                    .child(el().child(avatar.build(t)))
                    .child(who)
                    .child(
                        text(show_date(r.at as i64))
                            .font(ty::ui(12.0))
                            .color(t.muted),
                    )
                    .child(
                        Btn::manage(t, format!("sm-approve-{id}-{}", r.key), "Approve")
                            .on_press({
                                let id = id.clone();
                                move |s: &mut State| approve_request(s, &id, approve.clone())
                            })
                            .build(t),
                    )
                    .child(
                        Btn::manage(t, format!("sm-deny-{id}-{}", r.key), "Deny")
                            .danger_hover()
                            .on_press({
                                let id = id.clone();
                                move |s: &mut State| deny_request(s, &id, deny.clone())
                            })
                            .build(t),
                    ),
            );
        }
    }

    // ── Key
    root = root.child(
        row()
            .center_y()
            .gap(10.0)
            .child(manage_hint(t, format!("Space key #{key_id}")))
            .child(
                Btn::manage(t, format!("sm-rotate-key-{id}"), "Rotate key")
                    .on_press({
                        let id = id.clone();
                        move |s: &mut State| rotate_key(s, &id)
                    })
                    .build(t),
            ),
    );

    // ── Bin
    let bin_len = m.bin.as_ref().map_or(0, Vec::len);
    let mut bin_head = row()
        .center_y()
        .justify_between()
        .child(h3(t, "Bin").mb(0.0));
    if bin_len > 0 {
        bin_head = bin_head.child(
            Btn::manage(t, format!("sm-empty-bin-{id}"), "Empty bin…")
                .danger_hover()
                .on_press({
                    let id = id.clone();
                    move |s: &mut State| open_empty(s, &id)
                })
                .build(t),
        );
    }
    root = root.child(bin_head);
    match &m.bin {
        None => root = root.child(manage_hint(t, "Loading…")),
        Some(rows) if rows.is_empty() => root = root.child(manage_hint(t, "Nothing in the bin.")),
        Some(rows) => {
            let mut list = col();
            for b in rows {
                let busy = m.bin_busy == b.id;
                let leading = if store.layout_of(&b.type_key) == "task" {
                    checkbox_display(format!("sm-bin-check-{}", b.id), b.done)
                        .size(KitSize::Medium)
                        .build(t)
                } else {
                    text(object_icon(store, &b.icon, &b.type_key, space_id))
                        .font(ty::ui(13.0))
                        .color(t.fg)
                };
                let (restore_id, vanish) = (b.id.clone(), (b.id.clone(), b.name.clone()));
                list = list.child(
                    row()
                        .center_y()
                        .gap(8.0)
                        .py(4.0)
                        .child(el().child(leading))
                        .child(
                            text(b.name.clone())
                                .font(ty::ui(13.0))
                                .color(t.fg)
                                .truncate()
                                .grow(1.0)
                                .min_w(0.0),
                        )
                        .child(
                            Btn::manage(t, format!("sm-restore-{}", b.id), "Restore")
                                .text_style(11.0, ty::NORMAL)
                                .disabled(busy)
                                .on_press({
                                    let id = id.clone();
                                    move |s: &mut State| restore_object(s, &id, restore_id.clone())
                                })
                                .build(t),
                        )
                        .child(
                            Btn::manage(t, format!("sm-vanish-{}", b.id), "Delete forever")
                                .text_style(11.0, ty::NORMAL)
                                .danger_hover()
                                .disabled(busy)
                                .on_press({
                                    let id = id.clone();
                                    move |s: &mut State| {
                                        entry(s, &id).confirm_vanish = Some(vanish.clone())
                                    }
                                })
                                .build(t),
                        ),
                );
            }
            root = root.child(list);
        }
    }

    // ── Danger zone: an owner keeps at least one space; a member may always leave.
    if !owner || channel_count > 1 {
        root = root.child(h3(t, "Danger zone")).child(
            Btn::manage(
                t,
                format!("sm-remove-space-{id}"),
                if owner {
                    "Delete for everyone…"
                } else {
                    "Leave space…"
                },
            )
            .danger_hover()
            .full_width()
            .on_press({
                let id = id.clone();
                move |s: &mut State| open_delete(s, &id)
            })
            .build(t),
        );
    }

    confirm_dialog(s, m, t, layer, &id, owner);
    vanish_dialog(m, t, layer, &id);
    root
}

/// The empty-bin or delete/leave confirmation (`.del-modal`): title,
/// warning, "Type <phrase> to confirm.", the field, Cancel and the red
/// confirm that stays disabled until the phrase matches.
fn confirm_dialog(
    s: &State,
    m: &SpaceManageState,
    t: &Theme,
    layer: &mut Vec<El>,
    id: &str,
    owner: bool,
) {
    if !m.confirm_empty && !m.confirm_delete {
        return;
    }
    let cancel = |key: String| {
        let id = id.to_owned();
        Btn::manage(t, key, "Cancel").on_press(move |s: &mut State| dismiss_confirm(s, &id))
    };
    let (title, body, phrase, field, footer) = if m.confirm_empty {
        let n = m.bin.as_ref().map_or(0, Vec::len);
        let armed = empty_armed(m) && m.bin_busy.is_empty();
        let confirm = Btn::destructive(
            t,
            format!("sm-empty-confirm-{id}"),
            if m.bin_busy == ALL {
                "Deleting…"
            } else {
                "Delete forever"
            },
        )
        .disabled(!armed)
        .on_press({
            let id = id.to_owned();
            move |s: &mut State| empty_bin(s, &id)
        });
        let field = Field::new(
            format!("sm-empty-draft-{id}"),
            &m.empty_draft,
            delete_input_style(t),
        )
        .placeholder("delete")
        .on_edit({
            let id = id.to_owned();
            move |s: &mut State| &mut entry(s, &id).empty_draft
        })
        .on_submit({
            let id = id.to_owned();
            move |s: &mut State, _| {
                let go = with(s, &id, |m| empty_armed(m) && m.bin_busy.is_empty());
                if go == Some(true) {
                    empty_bin(s, &id);
                }
            }
        });
        (
            "Empty the bin?".to_string(),
            format!(
                "Permanently deletes {n} object{} — on every device, forever. This cannot be undone.",
                if n == 1 { "" } else { "s" }
            ),
            "delete".to_string(),
            field,
            row()
                .justify_end()
                .gap(8.0)
                .child(cancel(format!("sm-empty-cancel-{id}")).build(t))
                .child(confirm.build(t)),
        )
    } else {
        let name = space_name(&s.store, id);
        let armed = delete_armed(s, m, id) && !m.deleting;
        let shown = if name.is_empty() {
            "this space".to_string()
        } else {
            name
        };
        let (title, body) = if owner {
            (
                format!("Delete {shown} for everyone?"),
                "This permanently deletes the space and everything in it — for every member, on every device, forever. This cannot be undone.",
            )
        } else {
            (
                format!("Leave {shown}?"),
                "Only your own copies go: the space and everything in it disappear from your devices. The other members keep the space and everything in it, including what you added. This cannot be undone.",
            )
        };
        let confirm_label = match (owner, m.deleting) {
            (true, true) => "Deleting…",
            (true, false) => "Delete for everyone",
            (false, true) => "Leaving…",
            (false, false) => "Leave space",
        };
        let confirm = Btn::destructive(t, format!("sm-delete-confirm-{id}"), confirm_label)
            .disabled(!armed)
            .on_press({
                let id = id.to_owned();
                move |s: &mut State| remove_space(s, &id)
            });
        let field = Field::new(
            format!("sm-delete-draft-{id}"),
            &m.delete_draft,
            delete_input_style(t),
        )
        .placeholder(m.delete_placeholder.clone())
        .on_edit({
            let id = id.to_owned();
            move |s: &mut State| &mut entry(s, &id).delete_draft
        })
        .on_submit({
            let id = id.to_owned();
            move |s: &mut State, _| {
                let phrase = delete_phrase(&s.store, &id);
                let go = with(s, &id, |m| {
                    phrase_matches(m.delete_draft.value(), &phrase) && !m.deleting
                });
                if go == Some(true) {
                    remove_space(s, &id);
                }
            }
        });
        (
            title,
            body.to_string(),
            delete_phrase(&s.store, id),
            field,
            row()
                .justify_end()
                .gap(8.0)
                .child(
                    cancel(format!("sm-delete-cancel-{id}"))
                        .disabled(m.deleting)
                        .build(t),
                )
                .child(confirm.build(t)),
        )
    };
    let dismiss = id.to_owned();
    Dialog::new(format!("sm-confirm-{id}"))
        .width(380.0)
        .close_button(false)
        .entrance(DialogEntrance::FadeSlide)
        .open(true)
        .on_close(move |s: &mut State| dismiss_confirm(s, &dismiss))
        .padding(18.0)
        .radius(12.0)
        .fill(t.panel)
        .border_color(t.border)
        .child(
            col()
                .gap(14.0)
                .child(
                    text(title)
                        .font(ty::ui(15.0).lh(19.0).weight(ty::BOLD))
                        .color(t.fg)
                        .wrap(),
                )
                .child(
                    col()
                        .gap(10.0)
                        .child(manage_hint(t, body))
                        .child(type_to_confirm(t, &phrase))
                        .child(el().mt(8.0).child(field.build(t).w_full())),
                ),
        )
        .footer(footer)
        .build(t, layer);
}

/// "Permanently delete "<name>"? …" with OK / Cancel (the GPUI window prompt).
fn vanish_dialog(m: &SpaceManageState, t: &Theme, layer: &mut Vec<El>, id: &str) {
    let Some((_, name)) = &m.confirm_vanish else {
        return;
    };
    let cancel = {
        let id = id.to_owned();
        move |s: &mut State| {
            with(s, &id, |m| m.confirm_vanish = None);
        }
    };
    Dialog::new(format!("sm-vanish-ask-{id}"))
        .width(380.0)
        .close_button(false)
        .open(true)
        .on_close(cancel.clone())
        .padding(18.0)
        .radius(12.0)
        .fill(t.panel)
        .border_color(t.border)
        .child(
            text(format!(
                "Permanently delete \"{name}\"? This cannot be undone — on any device."
            ))
            .font(ty::ui(13.0).lh(18.0))
            .color(t.fg)
            .wrap(),
        )
        .footer(
            row()
                .justify_end()
                .gap(8.0)
                .child(
                    Btn::manage(t, format!("sm-vanish-cancel-{id}"), "Cancel")
                        .on_press(cancel)
                        .build(t),
                )
                .child(
                    Btn::destructive(t, format!("sm-vanish-ok-{id}"), "OK")
                        .on_press({
                            let id = id.to_owned();
                            move |s: &mut State| vanish_object(s, &id)
                        })
                        .build(t)
                        .default_button(),
                ),
        )
        .build(t, layer);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn space_manage_phrase_prefers_name() {
        assert_eq!(phrase_for("Garden", true), "Garden");
        assert_eq!(phrase_for("", true), "delete");
        assert_eq!(phrase_for("", false), "leave");
    }

    #[test]
    fn space_manage_phrase_match_trims_and_ignores_case() {
        assert!(phrase_matches("  garden ", "Garden"));
        assert!(phrase_matches("DELETE", "delete"));
        assert!(!phrase_matches("delet", "delete"));
        assert!(!phrase_matches("", "delete"));
    }

    #[test]
    fn space_manage_short_npub_keeps_twenty_chars() {
        assert_eq!(
            short_npub("npub1abcdefghijklmnopqrstuvwxyz"),
            "npub1abcdefghijklmno…"
        );
        assert_eq!(short_npub("npub1"), "npub1…");
    }
}
