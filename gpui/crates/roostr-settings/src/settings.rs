//! The Settings modal (`Settings.svelte`) as the local-mode web app shows
//! it: profile, chat colour, appearance, Nostr identity, relays, the desktop
//! sync check, the spellcheck dictionary, export, the Claude plan and agent
//! keys.
//!
//! The modal's state is one [`Settings`] value in the [`SettingsModal`] slot
//! while it is open; async work checks the open generation before it writes
//! back, so a reopened modal never hears an answer meant for the last one.

use std::time::Duration;

use futures::FutureExt as _;
use roostr_core::color::{DEFAULT_CHAT_COLOR, normalize_hex};
use roostr_core::devices::{self, DEVICE_ID_PREFIX, DeviceRecord};
use roostr_core::lite::VaultEstimate;
use roostr_core::mode::{self, Mode};
use roostr_core::profile as nostr_profile;
use roostr_core::serving::{MachineRow, fetch_machines, machine_name};
use roostr_core::sync_status::{progress_text, sync_progress};
use roostr_core::{State, StoreEvent, SyncPhase, api, io, lite_ui};
use roostr_rt::Task;
use roostr_ui::ty::Weights as _;
use roostr_ui::{
    ColorPicker, ColorPickerState, Glyph, InputState, SelectState, Theme, ThemePref,
    parse_color_hex, radius, ty,
};
use rune::{
    Color, Cx, Drag, Effect, El, Phase, ScrollSpec, Spring, SpringPair, Transition, Tx, col, el,
    row, text,
};
use serde_json::{Value as Json, json};

use crate::export::{ExportFormat, export_all};
use crate::local::{self, AgentAuth, PlanStatus, with_status};
use crate::profile;
use crate::section_nav::{self, Spy};
use crate::widgets::{
    Btn, Field, FieldStyle, SelectOption, Span, flash, h3, hint, rich, select, spinner, switch,
    sync_select,
};

/// The editor's "Add to dictionary" list (web localStorage `spell-ignore`).
pub(crate) const IGNORE_KEY: &str = "spell-ignore";
/// Gives up on a desktop that doesn't answer, so the check never sits on "Checking…" for good.
const DESK_TIMEOUT: Duration = Duration::from_secs(15);
/// Follow a Claude sign-in this often until it finishes.
const PLAN_POLL: Duration = Duration::from_millis(2000);
const FLASH: Duration = Duration::from_millis(1500);
const BUILD_STAMP: &str = env!("ROOSTR_BUILD_STAMP");
/// A pull on the phone sheet past this closes it.
const SHEET_CLOSE: f32 = 110.0;

/// Relays offered as toggles: ours on by default, public ones off.
const KNOWN_RELAYS: &[(&str, &str, &str)] = &[
    (
        "wss://roostr-relay.fly.dev",
        "Roostr relay",
        "self-hosted default",
    ),
    ("wss://relay.damus.io", "Damus", "public"),
    ("wss://nos.lol", "nos.lol", "public"),
    (
        "wss://relay.nostr.band",
        "nostr.band",
        "public aggregator - often read-only",
    ),
];

/// The theme choices of the Appearance section.
const THEMES: [(ThemePref, &str); 3] = [
    (ThemePref::Dark, "Dark"),
    (ThemePref::Light, "Light"),
    (ThemePref::System, "System"),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Desk {
    Idle,
    Checking,
    Match,
    Differ,
    Offline,
}

/// The modal's slot: its state while open, the open generation and the
/// window size its layer host last measured.
#[derive(Default)]
pub struct SettingsModal {
    open: Option<Box<Settings>>,
    generation: u64,
    viewport: [f32; 2],
}

/// The settings panel's state (the modal's content).
pub struct Settings {
    settings_error: String,
    desk: Desk,
    desk_detail: String,

    // Nostr identity
    npub: String,
    copied: &'static str,
    importing: bool,
    confirm_logout: bool,
    /// Replica mode: the revealed private key ("" = hidden).
    nsec: String,
    import_draft: InputState,
    import_error: String,
    /// The section navigation's scroll-spy.
    pub(crate) spy: Spy,

    // Relays
    relays: Vec<String>,
    new_relay: InputState,
    save_state: &'static str,

    // Agent credentials: `None` while checking, `Some(None)` when unavailable.
    agent_auth: Option<Option<AgentAuth>>,
    kimi_draft: InputState,
    agent_saved: &'static str,
    login_error: String,

    // Claude plan
    plan: Option<PlanStatus>,
    plan_machines: Vec<MachineRow>,
    /// The computer to keep the plan fresh: this one, until changed.
    plan_server: String,
    plan_server_select: SelectState,
    plan_code: InputState,
    plan_error: String,
    plan_busy: bool,
    plan_poll: Option<Task<()>>,

    // Profile
    profile_picture: String,
    profile_name: InputState,
    /// The name field changed since it was last saved (saved when it loses focus).
    name_dirty: bool,
    /// The name as last saved, so an unchanged field never republishes the profile.
    saved_name: String,
    avatar_busy: bool,
    avatar_state: String,

    // Chat colour
    chat_color_draft: InputState,
    color_dirty: bool,
    chat_color_state: String,
    swatch: ColorPickerState,

    // Appearance
    theme_select: SelectState,

    // Dictionary
    ignored: Vec<String>,

    // Export
    export_format: ExportFormat,
    export_select: SelectState,
    exporting: bool,
    export_note: String,

    // Lite: how much of the vault this device holds (`None` while estimating),
    // and the "Fully sync this device" details sheet.
    estimate: Option<VaultEstimate>,
    confirm_full: bool,
    full_requested: bool,
    // Full replica in a browser: the "Switch to Lite" sheet, and why the
    // replica refused (changes not sent yet).
    confirm_lite: bool,
    lite_error: Option<String>,

    // "Your devices": every device record of the account, this backend's
    // device id (from the settings), the record being renamed and its draft.
    devices: Vec<DeviceRecord>,
    device_id: String,
    show_older_devices: bool,
    renaming_device: Option<String>,
    device_name_draft: InputState,
    /// The phone sheet's drag-to-close: how far it is pulled down, and
    /// whether a finger is dragging it (it follows 1:1, else settles back).
    sheet_y: f32,
    sheet_drag: bool,
}

impl Settings {
    fn new(s: &State) -> Self {
        let cached = nostr_profile::cached(s);
        let name = cached.name.clone().unwrap_or_default();
        let chat_color = s.store.chat_color.clone();
        let swatch_color = parse_color_hex(&chat_color)
            .or_else(|| parse_color_hex(DEFAULT_CHAT_COLOR))
            .unwrap_or(Color::WHITE);
        let pref = crate::theme_pref(s);
        let theme_select = SelectState::new(
            THEMES
                .iter()
                .map(|(p, label)| roostr_ui::SelectItem::new(p.name(), *label))
                .collect(),
            THEMES.iter().position(|(p, _)| *p == pref),
        );
        let export_select = SelectState::new(
            vec![
                roostr_ui::SelectItem::new("markdown", ExportFormat::Markdown.label()),
                roostr_ui::SelectItem::new("json", ExportFormat::Json.label()),
            ],
            Some(0),
        );
        Self {
            settings_error: String::new(),
            desk: Desk::Idle,
            desk_detail: String::new(),
            npub: String::new(),
            copied: "",
            importing: false,
            confirm_logout: false,
            nsec: String::new(),
            import_draft: InputState::new().masked(true),
            import_error: String::new(),
            spy: Spy::default(),
            relays: Vec::new(),
            new_relay: InputState::new(),
            save_state: "",
            agent_auth: None,
            kimi_draft: InputState::new().masked(true),
            agent_saved: "",
            login_error: String::new(),
            plan: None,
            plan_machines: Vec::new(),
            plan_server: String::new(),
            plan_server_select: SelectState::new(Vec::new(), None),
            plan_code: InputState::new(),
            plan_error: String::new(),
            plan_busy: false,
            plan_poll: None,
            profile_picture: cached.picture.unwrap_or_default(),
            profile_name: InputState::with_value(name.clone()),
            name_dirty: false,
            saved_name: name,
            avatar_busy: false,
            avatar_state: String::new(),
            chat_color_draft: InputState::with_value(chat_color),
            color_dirty: false,
            chat_color_state: String::new(),
            swatch: ColorPickerState::new().default_value(swatch_color),
            theme_select,
            ignored: read_ignored(s),
            export_format: ExportFormat::Markdown,
            export_select,
            exporting: false,
            export_note: String::new(),
            estimate: None,
            confirm_full: false,
            full_requested: false,
            confirm_lite: false,
            lite_error: None,
            devices: Vec::new(),
            device_id: String::new(),
            show_older_devices: false,
            renaming_device: None,
            device_name_draft: InputState::new(),
            sheet_y: 0.0,
            sheet_drag: false,
        }
    }
}

// ----------------------------------------------------------------- open / close

/// Open Settings over the window (fresh state, its loads started). The app's
/// view draws it through [`settings_modal`].
pub fn open_settings(s: &mut State) {
    let settings = Settings::new(s);
    let slot = s.slot::<SettingsModal>();
    slot.generation += 1;
    slot.open = Some(Box::new(settings));
    // The column's scroll hook writes here (it exists before the first frame).
    s.slot::<SpyClock>();
    load_profile(s);
    load(s);
    load_estimate(s);
    load_devices(s);
}

/// Close Settings (Escape, the dim, ×, a pull down on the phone sheet).
pub fn close_settings(s: &mut State) {
    s.slot::<SettingsModal>().open = None;
}

/// Whether Settings is open.
pub fn settings_open(s: &State) -> bool {
    s.get::<SettingsModal>().is_some_and(|m| m.open.is_some())
}

/// The open panel's state.
pub(crate) fn open_mut(s: &mut State) -> Option<&mut Settings> {
    s.slot::<SettingsModal>().open.as_deref_mut()
}

fn open_ref(s: &State) -> Option<&Settings> {
    s.get::<SettingsModal>()?.open.as_deref()
}

fn generation(s: &mut State) -> u64 {
    s.slot::<SettingsModal>().generation
}

/// Runs `f` on the state when `task` lands, if the modal opened as
/// `generation` is still open.
fn after<T: 'static>(s: &mut State, task: Task<T>, f: impl FnOnce(&mut State, T) + 'static) {
    let generation = generation(s);
    s.rt.spawn(async move |cx| {
        let value = task.await;
        cx.update(|s| {
            if is_current(s, generation) {
                f(s, value)
            }
        });
    })
    .detach();
}

fn is_current(s: &mut State, generation: u64) -> bool {
    let slot = s.slot::<SettingsModal>();
    slot.generation == generation && slot.open.is_some()
}

/// The open panel; only called where [`is_current`] / [`open_mut`] held.
fn cur(s: &mut State) -> &mut Settings {
    open_mut(s).expect("settings are open")
}

/// Runs `f` on the open panel (a handler's press).
fn edit(s: &mut State, f: impl FnOnce(&mut Settings)) {
    if let Some(settings) = open_mut(s) {
        f(settings)
    }
}

/// Clears a flash after 1.5 s.
fn flash_later(s: &mut State, clear: fn(&mut Settings)) {
    let timer = s.rt.timer(FLASH);
    after(s, timer, move |s, ()| clear(cur(s)));
}

// ----------------------------------------------------------------- listeners

/// "Your devices" reload whenever a device record commits or the stream reconnects.
pub(crate) fn on_store(s: &mut State, event: &StoreEvent) {
    let reload = match event {
        StoreEvent::Committed(id) => id.starts_with(DEVICE_ID_PREFIX),
        StoreEvent::Reconnected => true,
        StoreEvent::Refreshed | StoreEvent::Live(_) => false,
    };
    if reload && settings_open(s) {
        load_devices(s);
    }
}

/// Commit-on-blur (`onchange`): the profile name and the chat colour save
/// once their field loses focus after a change.
pub(crate) fn on_poll(s: &mut State) -> bool {
    let Some(settings) = open_mut(s) else {
        return false;
    };
    let name = settings.name_dirty && !settings.profile_name.is_focused();
    let color = settings.color_dirty && !settings.chat_color_draft.is_focused();
    if name {
        settings.name_dirty = false;
        save_name(s);
    }
    if color {
        let settings = cur(s);
        settings.color_dirty = false;
        let raw = settings.chat_color_draft.value().to_string();
        save_chat_color(s, raw);
    }
    name || color
}

// ----------------------------------------------------------------- load

fn load(s: &mut State) {
    cur(s).settings_error.clear();
    let backend = s.store.backend();
    let task = s.rt.background(backend.settings());
    after(s, task, |s, result| {
        match result {
            Ok(settings) => {
                let me = cur(s);
                me.relays = settings.relays;
                me.npub = settings.npub.unwrap_or_default();
                me.device_id = settings.device_id;
            }
            Err(error) => cur(s).settings_error = error.to_string(),
        }
        load_agent_auth(s);
        load_plan(s);
    });
}

/// "Your devices": the account's device records, reloaded on open, after
/// a rename/remove and whenever a device record commits.
fn load_devices(s: &mut State) {
    let task = devices::list(s);
    after(s, task, |s, result| match result {
        Ok(records) => cur(s).devices = records,
        Err(error) => log::warn!("device list failed: {error:#}"),
    });
}

fn start_rename_device(s: &mut State, record: &DeviceRecord) {
    edit(s, |me| {
        me.renaming_device = Some(record.id.clone());
        me.device_name_draft.set_value(record.name.clone());
        me.device_name_draft.focus();
    });
}

fn save_device_name(s: &mut State) {
    let Some(me) = open_mut(s) else { return };
    let Some(id) = me.renaming_device.clone() else {
        return;
    };
    let name = me.device_name_draft.value().trim().to_string();
    if name.is_empty() {
        return;
    }
    me.renaming_device = None;
    let task = devices::rename(&id, &name, s);
    after_device_change(s, task);
}

fn remove_device(s: &mut State, id: &str) {
    let task = devices::remove(id, s);
    after_device_change(s, task);
}

fn after_device_change<T: 'static>(s: &mut State, task: Task<anyhow::Result<T>>) {
    cur(s).settings_error.clear();
    after(s, task, |s, result| {
        if let Err(error) = result {
            cur(s).settings_error = error.to_string();
        }
        load_devices(s);
    });
}

/// Replica only: the vault estimate, computed each time settings opens.
fn load_estimate(s: &mut State) {
    let Some(control) = roostr_core::lite::lite(s) else {
        return;
    };
    let task = s.rt.background(async move { control.estimate().await });
    after(s, task, |s, result| match result {
        Ok(estimate) => cur(s).estimate = Some(estimate),
        Err(error) => log::warn!("vault estimate failed: {error:#}"),
    });
}

fn load_profile(s: &mut State) {
    cur(s).avatar_state.clear();
    let task = nostr_profile::fetch(s);
    after(s, task, |s, result| match result {
        Ok(p) => apply_profile(cur(s), p),
        Err(error) => cur(s).avatar_state = error.to_string(),
    });
}

fn apply_profile(me: &mut Settings, p: nostr_profile::Profile) {
    me.profile_picture = p.picture.unwrap_or_default();
    me.saved_name = p.name.unwrap_or_default();
    me.profile_name.set_value(me.saved_name.clone());
    me.name_dirty = false;
}

fn read_ignored(s: &State) -> Vec<String> {
    let mut words: Vec<String> = s.ui.get(IGNORE_KEY).unwrap_or_default();
    words.sort();
    words.dedup();
    words
}

// ----------------------------------------------------------------- profile

fn pick_avatar(s: &mut State) {
    if io::WEB {
        cur(s).avatar_state = io::NO_FILE_PICKER.into();
        return;
    }
    let Some(roostr_core::create::PickFile(pick)) = s.pick_file.clone() else {
        cur(s).avatar_state = "This app has no file dialog to choose a picture with.".into();
        return;
    };
    let picked = pick(s);
    let generation = generation(s);
    s.rt.spawn(async move |cx| {
        let Some(path) = picked.await else {
            return;
        };
        let open = cx.update(|s| {
            let current = is_current(s, generation);
            if current {
                let me = cur(s);
                me.avatar_busy = true;
                me.avatar_state = "saving…".into();
            }
            current
        });
        if !open {
            return;
        }
        let picture = cx
            .background(async move { profile::image_to_avatar(&path) })
            .await;
        let result = match picture {
            Ok(picture) => {
                let save = cx
                    .update(|s| nostr_profile::save(nostr_profile::patch("picture", &picture), s));
                save.await
            }
            Err(error) => Err(error),
        };
        cx.update(|s| {
            if !is_current(s, generation) {
                return;
            }
            let me = cur(s);
            match result {
                Ok(p) => {
                    apply_profile(me, p);
                    me.avatar_state = "saved".into();
                }
                Err(error) => me.avatar_state = error.to_string(),
            }
            me.avatar_busy = false;
        });
    })
    .detach();
}

fn save_profile(s: &mut State, patch: Json, saving: bool) {
    let me = cur(s);
    me.avatar_busy = true;
    if saving {
        me.avatar_state = "saving…".into();
    }
    let task = nostr_profile::save(patch, s);
    after(s, task, |s, result| {
        let me = cur(s);
        match result {
            Ok(p) => {
                me.profile_picture = p.picture.unwrap_or_default();
                me.saved_name = p.name.unwrap_or_default();
                me.avatar_state = "saved".into();
            }
            Err(error) => me.avatar_state = error.to_string(),
        }
        me.avatar_busy = false;
    });
}

fn save_name(s: &mut State) {
    let me = cur(s);
    let name = me.profile_name.value().trim().to_string();
    if name == me.saved_name {
        return;
    }
    save_profile(s, nostr_profile::patch("name", &name), true);
}

// ----------------------------------------------------------------- chat colour

fn save_chat_color(s: &mut State, raw: String) {
    if normalize_hex(&raw).is_none() {
        cur(s).chat_color_state = "Use a hex color like #EDCF7F".into();
        return;
    }
    cur(s).chat_color_state = "saving…".into();
    let task = api::user_prefs::set_chat_color(&raw, s);
    after(s, task, |s, result| match result {
        Ok(hex_value) => {
            s.store.chat_color = hex_value.clone();
            let me = cur(s);
            me.chat_color_draft.set_value(hex_value.clone());
            me.color_dirty = false;
            if let Some(color) = parse_color_hex(&hex_value) {
                me.swatch.set_value(color);
            }
            me.chat_color_state = "saved".into();
        }
        Err(error) => cur(s).chat_color_state = error.to_string(),
    });
}

// ----------------------------------------------------------------- identity

fn copy(s: &mut State, tx: &mut Tx, text: String, label: &'static str) {
    tx.copy(text);
    cur(s).copied = label;
    flash_later(s, |me| me.copied = "");
}

fn import_key(s: &mut State) {
    let me = cur(s);
    me.import_error.clear();
    let key = me.import_draft.value().trim().to_string();
    let task = api::settings::import_key(&key, s);
    after(s, task, |s, result| match result {
        Ok(_) => {
            // The web reloads the page: close Settings and reopen the vault under the new identity.
            close_settings(s);
            s.store.reconnect();
        }
        Err(error) => cur(s).import_error = error.to_string(),
    });
}

fn cancel_import(me: &mut Settings) {
    me.importing = false;
    me.import_draft.clear();
    me.import_error.clear();
}

/// Replica mode: "Log out on this device" (api.ts `settings.logout`).
fn log_out(s: &mut State) {
    cur(s).settings_error.clear();
    let Some(account) = roostr_core::account::account(s) else {
        return;
    };
    let task = s.rt.background(account.logout());
    after(s, task, |s, result| match result {
        Ok(()) => {
            close_settings(s);
            // No key now: the vault reopens on the sign-in gate.
            s.store.reconnect();
        }
        Err(error) => {
            let me = cur(s);
            me.settings_error = error.to_string();
            me.confirm_logout = false;
        }
    });
}

// ----------------------------------------------------------------- relays

fn save_relays(s: &mut State, next: Vec<String>) {
    cur(s).settings_error.clear();
    let task = api::settings::set_relays(&next, s);
    after(s, task, move |s, result| match result {
        Ok(_) => {
            if let Some(settings) = s.store.settings.as_mut() {
                settings.relays = next.clone();
            }
            let me = cur(s);
            me.relays = next;
            me.save_state = "Saved";
            flash_later(s, |me| me.save_state = "");
        }
        Err(error) => cur(s).settings_error = error.to_string(),
    });
}

fn add_relay(s: &mut State) {
    let me = cur(s);
    let mut v = me.new_relay.value().trim().to_string();
    if v.is_empty() {
        return;
    }
    if !v.starts_with("wss://") && !v.starts_with("ws://") {
        v = format!("wss://{v}");
    }
    me.new_relay.clear();
    if !me.relays.contains(&v) {
        let mut next = me.relays.clone();
        next.push(v);
        save_relays(s, next);
    }
}

fn toggle_relay(s: &mut State, url: &str, on: bool) {
    let mut next: Vec<String> = cur(s)
        .relays
        .iter()
        .filter(|r| *r != url)
        .cloned()
        .collect();
    if on {
        next.push(url.to_string());
    }
    save_relays(s, next);
}

// ----------------------------------------------------------------- desktop check

/// Compare the vault fingerprint the daemon reports with this app's view
/// of it. In native mode both refer to the one daemon.
fn check_desktop(s: &mut State) {
    let me = cur(s);
    me.desk = Desk::Checking;
    me.desk_detail = "Asking the desktop for its fingerprint…".into();
    let backend = s.store.backend();
    let remote = s.rt.background(backend.sync_digest());
    let timeout = s.rt.timer(DESK_TIMEOUT);
    let generation = generation(s);
    s.rt
        .spawn(async move |cx| {
            let remote = futures::select! {
                r = remote.fuse() => r,
                _ = timeout.fuse() => Err(anyhow::anyhow!(
                    "the desktop didn't answer within {} s - is Roostr running on it?",
                    DESK_TIMEOUT.as_secs()
                )),
            };
            let num = |m: &serde_json::Map<String, Json>, k: &str| {
                m.get(k).and_then(Json::as_i64).unwrap_or(0)
            };
            let remote = match remote {
                Ok(remote) => remote,
                Err(error) => {
                    cx.update(|s| {
                        if is_current(s, generation) {
                            let me = cur(s);
                            me.desk = Desk::Offline;
                            me.desk_detail = error.to_string();
                        }
                    });
                    return;
                }
            };
            let (r_objects, r_changes) = (num(&remote, "objects"), num(&remote, "changes"));
            cx.update(|s| {
                if is_current(s, generation) {
                    cur(s).desk_detail =
                        format!("Desktop has {r_changes} changes. Counting this device's…");
                }
            });
            let local = cx.background(backend.sync_digest()).await;
            cx.update(|s| {
                if !is_current(s, generation) {
                    return;
                }
                let me = cur(s);
                match local {
                    Ok(local) => {
                        let digest = |m: &serde_json::Map<String, Json>| {
                            m.get("digest")
                                .and_then(Json::as_str)
                                .unwrap_or("")
                                .to_string()
                        };
                        let (l_objects, l_changes) =
                            (num(&local, "objects"), num(&local, "changes"));
                        if digest(&local) == digest(&remote) {
                            me.desk = Desk::Match;
                            me.desk_detail =
                                format!("identical — {r_objects} objects, {r_changes} changes");
                        } else {
                            me.desk = Desk::Differ;
                            let who = if l_changes < r_changes {
                                "this device is behind"
                            } else if l_changes > r_changes {
                                "the desktop is behind"
                            } else {
                                "same size, different content"
                            };
                            me.desk_detail = format!(
                                "desktop {r_objects}/{r_changes} · here {l_objects}/{l_changes} (objects/changes) — {who}"
                            );
                        }
                    }
                    Err(error) => {
                        me.desk = Desk::Offline;
                        me.desk_detail = error.to_string();
                    }
                }
            });
        })
        .detach();
}

// ----------------------------------------------------------------- dictionary / export

fn unignore(s: &mut State, word: &str) {
    let word = word.to_lowercase();
    let mut words: Vec<String> = s.ui.get(IGNORE_KEY).unwrap_or_default();
    words.retain(|w| *w != word);
    s.ui.set(IGNORE_KEY, &words);
    let ignored = read_ignored(s);
    cur(s).ignored = ignored;
}

fn run_export(s: &mut State) {
    let me = cur(s);
    me.exporting = true;
    me.export_note.clear();
    let format = me.export_format;
    let backend = s.store.backend();
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<(usize, usize)>();
    let task =
        s.rt.background(export_all(backend, format, move |done, total| {
            let _ = tx.unbounded_send((done, total));
        }));
    let generation = generation(s);
    s.rt.spawn(async move |cx| {
        use futures::StreamExt as _;
        let mut task = task.fuse();
        let result = loop {
            futures::select! {
                progress = rx.next() => if let Some((done, total)) = progress {
                    cx.update(|s| if is_current(s, generation) {
                        cur(s).export_note = format!("Preparing {done}/{total}…");
                    });
                },
                result = task => break result,
            }
        };
        cx.update(|s| {
            if is_current(s, generation) {
                let me = cur(s);
                me.export_note = match result {
                    Ok(n) => format!("Exported {n} objects."),
                    Err(error) => error.to_string(),
                };
                me.exporting = false;
            }
        });
    })
    .detach();
}

// ----------------------------------------------------------------- Claude plan

fn plan_call(s: &mut State, path: &'static str, body: Option<Json>) -> Task<()> {
    cur(s).plan_error.clear();
    let task: Task<anyhow::Result<PlanStatus>> = match body {
        Some(body) => local::post(path, body, s),
        None => local::get(path, s),
    };
    let generation = generation(s);
    s.rt.spawn(async move |cx| {
        let result = task
            .await
            .map_err(|e| with_status(e, |s| format!("Claude plan: {s}")));
        cx.update(|s| {
            if !is_current(s, generation) {
                return;
            }
            match result {
                Ok(plan) => {
                    // Signing in: follow it until the window finishes (or stops and the paste box takes over).
                    let poll = plan.signing_in.then(|| {
                        s.rt.spawn(async move |cx| {
                            cx.timer(PLAN_POLL).await;
                            let next = cx.update(|s| {
                                is_current(s, generation)
                                    .then(|| plan_call(s, "/claude-plan", None))
                            });
                            if let Some(next) = next {
                                next.await;
                            }
                        })
                    });
                    let me = cur(s);
                    if me.plan_server.is_empty() {
                        me.plan_server = if plan.served_by.is_empty() {
                            plan.this_computer.clone()
                        } else {
                            plan.served_by.clone()
                        };
                    }
                    me.plan_poll = poll;
                    me.plan = Some(plan);
                    sync_plan_select(me);
                }
                Err(error) => cur(s).plan_error = error.to_string(),
            }
        });
    })
}

/// The "Kept fresh by" select's options and value.
fn sync_plan_select(me: &mut Settings) {
    let this_computer = me
        .plan
        .as_ref()
        .map(|p| p.this_computer.clone())
        .unwrap_or_default();
    let options: Vec<SelectOption> = me
        .plan_machines
        .iter()
        .map(|m| {
            let this_one = if m.machine_id == this_computer {
                " (this computer)"
            } else {
                ""
            };
            (m.machine_id.clone(), format!("{}{this_one}", m.name))
        })
        .collect();
    sync_select(&mut me.plan_server_select, &options, &me.plan_server);
}

fn load_plan(s: &mut State) {
    if s.harness.is_none() {
        cur(s).plan = None;
        return;
    }
    let call = plan_call(s, "/claude-plan", None);
    let generation = generation(s);
    s.rt.spawn(async move |cx| {
        call.await;
        let machines = cx.update(|s| is_current(s, generation).then(|| fetch_machines(s)));
        let Some(machines) = machines else {
            return;
        };
        let machines = machines.await.map(|(_, m)| m).unwrap_or_default();
        cx.update(|s| {
            if is_current(s, generation) {
                let me = cur(s);
                me.plan_machines = machines;
                sync_plan_select(me);
            }
        });
    })
    .detach();
}

fn plan_action(s: &mut State, path: &'static str, body: Option<Json>) -> Task<()> {
    cur(s).plan_busy = true;
    let call = plan_call(s, path, body);
    let generation = generation(s);
    s.rt.spawn(async move |cx| {
        call.await;
        cx.update(|s| {
            if is_current(s, generation) {
                cur(s).plan_busy = false;
            }
        });
    })
}

fn submit_plan_code(s: &mut State) {
    let code = cur(s).plan_code.value().trim().to_string();
    if code.is_empty() {
        return;
    }
    let action = plan_action(s, "/claude-plan/code", Some(json!({ "code": code })));
    after(s, action, |s, ()| cur(s).plan_code.clear());
}

fn plan_computer(me: &Settings, id: &str) -> String {
    if id.is_empty() {
        "this computer".into()
    } else {
        machine_name(&me.plan_machines, id)
    }
}

// ----------------------------------------------------------------- agent keys

fn load_agent_auth(s: &mut State) {
    cur(s).login_error.clear();
    if s.harness.is_none() {
        cur(s).agent_auth = Some(None);
        return;
    }
    let task: Task<anyhow::Result<AgentAuth>> = local::get("/auth/status", s);
    after(s, task, |s, result| {
        let me = cur(s);
        match result.map_err(|e| with_status(e, |s| format!("Agent credentials: {s}"))) {
            Ok(auth) => me.agent_auth = Some(Some(auth)),
            Err(error) => {
                me.agent_auth = Some(None);
                me.login_error = error.to_string();
            }
        }
    });
}

fn save_agent_key(s: &mut State, provider: &'static str, key: String) {
    cur(s).login_error.clear();
    let key = key.trim().to_string();
    let task: Task<anyhow::Result<Json>> =
        local::post("/auth/key", json!({ "provider": provider, "key": key }), s);
    after(s, task, move |s, result| {
        match result.map_err(|e| with_status(e, |s| format!("Credential update failed: {s}"))) {
            Ok(_) => {
                cur(s).kimi_draft.clear();
                load_agent_auth(s);
                cur(s).agent_saved = if key.is_empty() { "Cleared" } else { "Saved" };
                flash_later(s, |me| me.agent_saved = "");
            }
            Err(error) => cur(s).login_error = error.to_string(),
        }
    });
}

// ----------------------------------------------------------------- render helpers

/// A section card under its heading, with an optional transient " · Saved".
fn section(t: &Theme, title: &'static str, flash_text: &str) -> El {
    let slug = section_nav::slug(title);
    let mut heading = row().gap(4.0).mb(6.0).child(h3(t, title));
    if let Some(f) = flash(&format!("settings-flash-{slug}"), flash_text, |s| {
        h3(t, format!("· {s}"))
    }) {
        heading = heading.child(f);
    }
    col()
        .key(format!("settings-section-{slug}"))
        .mt(14.0)
        .px(17.0)
        .pt(15.0)
        .pb(17.0)
        .radius(10.0)
        .fill(t.panel)
        .border(1.0, t.border)
        .items_stretch()
        .child(heading)
}

/// `.hint` with its bottom margin (10px).
fn p(t: &Theme, s: impl Into<String>) -> El {
    hint(t, s).mb(10.0)
}

/// `.hint.error`: in the web stylesheet `.hint` comes after `.error`, so
/// these messages draw muted like any hint.
fn error_p(t: &Theme, s: impl Into<String>) -> El {
    p(t, s)
}

/// A bordered settings card (holding confirm, device rows): an outline group box.
fn card(key: impl Into<String>, border: Color, fill: Option<Color>, gap: f32) -> El {
    let mut c = col()
        .key(key.into())
        .px(10.0)
        .py(8.0)
        .radius(8.0)
        .border(1.0, border)
        .gap(gap)
        .items_stretch();
    if let Some(fill) = fill {
        c = c.fill(fill);
    }
    c
}

/// The small outline label beside a key ("SECRET", "SHAREABLE").
fn key_tag(label: &'static str, color: Color, border: Color) -> El {
    row()
        .px(8.0)
        .radius(999.0)
        .border(1.0, border)
        .child(text(label).font(ty::ui(10.0).lh(16.0)).color(color))
}

/// The rounded outline badge of the mode line ("Full mode") and the device rows.
fn pill(label: impl Into<String>, color: Color) -> El {
    row()
        .px(8.0)
        .py(2.0)
        .radius(999.0)
        .border(1.0, color)
        .child(
            text(label.into())
                .font(ty::ui(12.0).lh(16.0).weight(ty::SEMIBOLD))
                .color(color),
        )
}

/// The rich paragraph style of the identity / agent notes (12px, 18px line).
fn note(t: &Theme, spans: &[Span], color: Color) -> El {
    rich(t, spans, color, 12.0, 18.0).mb(10.0)
}

/// A key display row: the monospace key in a bordered box and its Copy button.
fn key_row(t: &Theme, key: &str, value: &str, copied: bool, label: &'static str) -> El {
    let copy_value = value.to_string();
    row()
        .center_y()
        .gap(8.0)
        .mb(6.0)
        .child(
            row()
                .key(format!("{key}-code"))
                .grow(1.0)
                .min_w(0.0)
                .scroll_x()
                .px(11.0)
                .py(8.0)
                .radius(8.0)
                .fill(t.bg)
                .border(1.0, t.border)
                .child(text(value.to_string()).font(ty::mono(11.0)).color(t.fg)),
        )
        .child(
            Btn::small(
                t,
                format!("copy-{key}"),
                if copied { "Copied" } else { "Copy" },
            )
            .on_press_tx(move |s: &mut State, tx: &mut Tx| copy(s, tx, copy_value.clone(), label))
            .build(t),
        )
}

fn key_heading(t: &Theme, title: &'static str, kind: &'static str, tag: El) -> El {
    row()
        .center_y()
        .gap(8.0)
        .mt(10.0)
        .mb(4.0)
        .child(
            text(title)
                .font(ty::ui(12.0).weight(ty::MEDIUM))
                .color(t.fg),
        )
        .child(text(kind).font(ty::mono(11.0)).color(t.muted))
        .child(tag)
}

fn btn_row(b: El) -> El {
    row().mt(6.0).child(b)
}

// ----------------------------------------------------------------- sections

fn profile_section(s: &State, me: &Settings, t: &Theme) -> El {
    let image = (!me.profile_picture.is_empty())
        .then(|| s.get::<roostr_ui::Images>())
        .flatten()
        .and_then(|images| images.image(&me.profile_picture, 56.0));
    let mut avatar = roostr_ui::Avatar::new()
        .size(roostr_ui::KitSize::Px(56.0))
        .placeholder(Glyph::Settings);
    if let Some(image) = image {
        avatar = avatar.src(image.pixels().clone());
    }
    let has_picture = !me.profile_picture.is_empty();
    let mut controls = row()
        .flex_wrap()
        .center_y()
        .gap(12.0)
        .child(avatar.build(t))
        .child(
            Field::new(
                "settings-profile-name",
                &me.profile_name,
                FieldStyle::modal(t),
            )
            .placeholder("Your name")
            .on_edit(|s: &mut State| &mut me_or_dummy(s).profile_name)
            .on_change(|s: &mut State, _: &mut Tx| edit(s, |me| me.name_dirty = true))
            .on_submit(|s: &mut State, _: &mut Tx| {
                edit(s, |me| {
                    me.profile_name.edit.focused = false;
                    me.name_dirty = false;
                });
                if settings_open(s) {
                    save_name(s);
                }
            })
            .build(t)
            .grow(1.0)
            .min_w(120.0)
            .max_w(220.0),
        )
        .child(
            Btn::action(
                t,
                "pick-avatar",
                if has_picture {
                    "Change picture"
                } else {
                    "Upload picture"
                },
            )
            .disabled(me.avatar_busy)
            .on_press(|s: &mut State| {
                if settings_open(s) {
                    pick_avatar(s)
                }
            })
            .build(t),
        );
    if has_picture {
        controls = controls.child(
            Btn::small(t, "remove-avatar", "Remove")
                .disabled(me.avatar_busy)
                .on_press(|s: &mut State| {
                    if settings_open(s) {
                        save_profile(s, nostr_profile::patch("picture", ""), false)
                    }
                })
                .build(t),
        );
    }
    if !me.avatar_state.is_empty() {
        controls = controls.child(hint(t, me.avatar_state.clone()));
    }
    section(t, "Profile", "")
        .child(p(
            t,
            "Your name and avatar (your Nostr profile), synced to every device holding this key. The avatar also shows on the Spaces screen.",
        ))
        .child(controls)
}

fn chat_section(me: &Settings, t: &Theme, layer: &mut Vec<El>) -> El {
    let draft = me.chat_color_draft.value();
    let not_default = normalize_hex(draft).as_deref() != Some(DEFAULT_CHAT_COLOR);
    let picker = ColorPicker::new("settings-chat-swatch", &me.swatch, |s: &mut State| {
        &mut me_or_dummy(s).swatch
    })
    .accessibility_label("Chat color picker")
    .on_change(|s: &mut State, color: Option<Color>| {
        if let Some(color) = color
            && settings_open(s)
        {
            save_chat_color(s, roostr_ui::color_hex(color))
        }
    })
    .build(t, layer);
    let mut controls = row()
        .flex_wrap()
        .center_y()
        .gap(12.0)
        .child(
            row()
                .w(40.0)
                .h(32.0)
                .padding(2.0)
                .center()
                .radius(6.0)
                .fill(t.panel)
                .border(1.0, t.border)
                .child(picker),
        )
        .child(
            Field::new(
                "settings-chat-color",
                &me.chat_color_draft,
                FieldStyle::modal(t),
            )
            .placeholder(DEFAULT_CHAT_COLOR)
            .on_edit(|s: &mut State| &mut me_or_dummy(s).chat_color_draft)
            .on_change(|s: &mut State, _: &mut Tx| edit(s, |me| me.color_dirty = true))
            .on_submit(|s: &mut State, _: &mut Tx| {
                let Some(me) = open_mut(s) else { return };
                me.chat_color_draft.edit.focused = false;
                me.color_dirty = false;
                let raw = me.chat_color_draft.value().to_string();
                save_chat_color(s, raw);
            })
            .build(t)
            .grow(1.0)
            .max_w(220.0),
        );
    if not_default {
        controls = controls.child(
            Btn::small(t, "reset-color", "Reset")
                .on_press(|s: &mut State| {
                    if settings_open(s) {
                        save_chat_color(s, DEFAULT_CHAT_COLOR.to_string())
                    }
                })
                .build(t),
        );
    }
    if !me.chat_color_state.is_empty() {
        controls = controls.child(hint(t, me.chat_color_state.clone()));
    }
    section(t, "Chat color", "")
        .child(p(
            t,
            "Your message bubbles and send button, on every device holding this key. Text on it switches to dark or white to stay readable.",
        ))
        .child(controls)
}

fn appearance_section(me: &Settings, t: &Theme, layer: &mut Vec<El>) -> El {
    section(t, "Appearance", "")
        .child(p(
            t,
            "How Roostr looks on this device. System follows your computer's light or dark setting.",
        ))
        .child(
            row().child(select(
                t,
                layer,
                "settings-theme",
                &me.theme_select,
                |s: &mut State| &mut me_or_dummy(s).theme_select,
                120.0,
                t.bg,
                |s: &mut State, value: &str, tx: &mut Tx| {
                    crate::set_theme_pref(s, tx, ThemePref::from_name(value))
                },
            )),
        )
}

fn identity_section(s: &State, me: &Settings, t: &Theme) -> El {
    let mut identity = section(t, "Nostr identity", "").child(p(
        t,
        "This key encrypts and signs everything you sync. Anyone holding it has your full identity — export it only to back it up or to log in on another device.",
    ));
    if !me.npub.is_empty() {
        let green = roostr_ui::color::rgb(0x35b57f);
        identity = identity
            .child(key_heading(
                t,
                "Public key",
                "npub",
                key_tag("SHAREABLE", green, green.alpha(0x73 as f32 / 255.0)),
            ))
            .child(key_row(t, "npub", &me.npub, me.copied == "npub", "npub"));
    }
    let replica = mode::current(s) == Mode::Replica;
    if replica {
        identity = reveal(me, t, identity);
    } else {
        identity = identity.child(note(
            t,
            &[
                Span::Plain(
                    "Back up the native private key only from the operator's terminal on this computer: ",
                ),
                Span::Code("./glon-odin key-export"),
                Span::Plain(
                    " (run from the native installation directory). Keep the output secret; this app cannot request or reveal it.",
                ),
            ],
            t.muted,
        ));
    }
    if !me.importing {
        identity = identity.child(btn_row(
            Btn::action(t, "import-key", "Sign in with existing key…")
                .subtle(t)
                .on_press(|s: &mut State| {
                    edit(s, |me| {
                        me.importing = true;
                        me.import_draft.focus();
                    })
                })
                .build(t),
        ));
        if !replica {
            identity = identity.child(p(
                t,
                "Replaces this machine's daemon identity for every paired tab; the previous identity's shared-space authority is revoked.",
            ));
        }
        let label = match (me.confirm_logout, replica, io::WEB) {
            (true, true, _) => "Erase local copy & sign out?",
            (false, true, _) => "Log out on this device",
            (true, false, false) => "Disconnect this app?",
            (false, false, false) => "Disconnect this app",
            (true, false, true) => "Disconnect this browser?",
            (false, false, true) => "Unpair this browser",
        };
        identity = identity.child(btn_row(
            Btn::action(t, "logout", label)
                .subtle(t)
                .when(me.confirm_logout, Btn::armed)
                .on_press_tx(move |s: &mut State, _tx: &mut Tx| {
                    let Some(me) = open_mut(s) else { return };
                    if !me.confirm_logout {
                        me.confirm_logout = true;
                        return;
                    }
                    if replica {
                        log_out(s);
                        return;
                    }
                    // Disconnecting the only client of a native app is quitting it;
                    // a browser tab drops its pairing and returns to the pairing gate.
                    #[cfg(target_family = "wasm")]
                    roostr_core::web::unpair();
                    #[cfg(not(target_family = "wasm"))]
                    _tx.quit();
                })
                .build(t),
        ));
        if me.confirm_logout {
            identity = identity.child(p(
                t,
                match (replica, io::WEB) {
                    (true, _) => {
                        "Removes this device's key and its local replica only after pending changes are published. Your encrypted history stays on your relays — log in with any nsec afterwards."
                    }
                    (false, true) => {
                        "Disconnects this browser only. The native key, local data, and running daemon are not erased."
                    }
                    (false, false) => {
                        "Disconnects this app only. The native key, local data, and running daemon are not erased."
                    }
                },
            ));
        }
    } else {
        identity = identity
            .child(note(
                t,
                &[
                    Span::Bold("Replaces this device's identity."),
                    Span::Plain(
                        " Paste the nsec from your other device — after the sync daemon connects, your objects will backfill from the relays.",
                    ),
                ],
                t.muted,
            ))
            .child(
                row()
                    .gap(8.0)
                    .mt(6.0)
                    .child(
                        Field::new("settings-import", &me.import_draft, FieldStyle::modal(t))
                            .placeholder("nsec1… or 64-char hex")
                            .on_edit(|s: &mut State| &mut me_or_dummy(s).import_draft)
                            .on_submit(|s: &mut State, _: &mut Tx| {
                                if open_ref(s)
                                    .is_some_and(|me| !me.import_draft.value().trim().is_empty())
                                {
                                    import_key(s)
                                }
                            })
                            .build(t)
                            .grow(1.0),
                    )
                    .child(
                        Btn::small(t, "import-submit", "Import")
                            .on_press(|s: &mut State| {
                                if open_ref(s)
                                    .is_some_and(|me| !me.import_draft.value().trim().is_empty())
                                {
                                    import_key(s)
                                }
                            })
                            .build(t),
                    )
                    .child(
                        Btn::small(t, "import-cancel", "Cancel")
                            .on_press(|s: &mut State| edit(s, cancel_import))
                            .build(t),
                    ),
            );
        if !me.import_error.is_empty() {
            identity = identity.child(error_p(t, me.import_error.clone()).mt(10.0));
        }
    }
    identity
}

/// Replica mode: "Reveal private key" and, revealed, the nsec to copy.
fn reveal(me: &Settings, t: &Theme, identity: El) -> El {
    if me.nsec.is_empty() {
        return identity.child(
            row().mt(6.0).mb(6.0).child(
                Btn::action(t, "reveal-key", "Reveal private key")
                    .on_press(|s: &mut State| {
                        let exported = roostr_core::account::account(s).map(|a| a.export_key());
                        edit(s, |me| match exported {
                            Some(Ok((nsec, _))) => me.nsec = nsec,
                            Some(Err(error)) => me.settings_error = error.to_string(),
                            None => {}
                        })
                    })
                    .build(t),
            ),
        );
    }
    identity
        .child(key_heading(
            t,
            "Private key",
            "nsec",
            key_tag("SECRET", t.red, t.red),
        ))
        .child(key_row(t, "nsec", &me.nsec, me.copied == "nsec", "nsec"))
        .child(p(
            t,
            "Paste this nsec into \"Sign in with existing key\" on another Roostr to log in as you.",
        ))
        .child(btn_row(
            Btn::action(t, "hide-key", "Hide")
                .subtle(t)
                .on_press(|s: &mut State| edit(s, |me| me.nsec.clear()))
                .build(t),
        ))
}

fn relays_section(me: &Settings, t: &Theme) -> El {
    let mut relays = section(t, "Relays", me.save_state).child(p(
        t,
        "Where your encrypted changes sync. Your data stays local until relays are configured.",
    ));
    let relay_row = |key: String| {
        row()
            .key(key)
            .center_y()
            .gap(10.0)
            .mb(6.0)
            .px(11.0)
            .py(7.0)
            .radius(8.0)
            .fill(t.bg)
            .border(1.0, t.border)
    };
    for (ix, (url, label, note)) in KNOWN_RELAYS.iter().enumerate() {
        let on = me.relays.iter().any(|r| r == url);
        relays = relays.child(
            relay_row(format!("relay-{ix}"))
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .gap(1.0)
                        .child(text(*label).font(ty::mono(13.0).lh(16.0)).color(t.fg))
                        .child(
                            text(format!("{} · {note}", url.trim_start_matches("wss://")))
                                .font(ty::mono(11.0).lh(14.0))
                                .color(t.muted)
                                .truncate(),
                        ),
                )
                .child(switch(
                    t,
                    format!("relay-switch-{ix}"),
                    on,
                    move |s: &mut State, on| {
                        if settings_open(s) {
                            toggle_relay(s, url, on)
                        }
                    },
                )),
        );
    }
    for r in me
        .relays
        .iter()
        .filter(|r| !KNOWN_RELAYS.iter().any(|(u, _, _)| u == *r))
    {
        let url = r.clone();
        relays = relays.child(
            relay_row(format!("relay-{r}"))
                .justify_between()
                .child(text(r.clone()).font(ty::mono(13.0)).color(t.fg).min_w(0.0))
                .child(
                    Btn::x(t, format!("remove-relay-{r}"))
                        .on_press(move |s: &mut State| {
                            let Some(me) = open_mut(s) else { return };
                            let next = me.relays.iter().filter(|x| **x != url).cloned().collect();
                            save_relays(s, next);
                        })
                        .build(t),
                ),
        );
    }
    if me.relays.is_empty() {
        relays = relays.child(
            hint(t, "No relays enabled - nothing syncs until one is on.")
                .mt(4.0)
                .mb(4.0),
        );
    }
    relays.child(
        row()
            .gap(8.0)
            .mt(8.0)
            .child(
                Field::new("settings-new-relay", &me.new_relay, FieldStyle::modal(t))
                    .placeholder("wss://relay.example.com")
                    .on_edit(|s: &mut State| &mut me_or_dummy(s).new_relay)
                    .on_submit(|s: &mut State, _: &mut Tx| {
                        if settings_open(s) {
                            add_relay(s)
                        }
                    })
                    .build(t)
                    .grow(1.0),
            )
            .child(
                Btn::small(t, "add-relay", "Add")
                    .on_press(|s: &mut State| {
                        if settings_open(s) {
                            add_relay(s)
                        }
                    })
                    .build(t),
            ),
    )
}

fn vault_section(s: &State, me: &Settings, t: &Theme) -> El {
    let replica = mode::current(s) == Mode::Replica;
    let vault = section_plain(t, "Where your vault lives")
        .child(p(
            t,
            if replica {
                if io::WEB {
                    "In this browser: its own replica syncs end-to-end encrypted over your relays, no daemon needed. Roostr running on this computer can be paired instead."
                } else {
                    "On this device: its own replica syncs end-to-end encrypted over your relays, no daemon needed. The glon-odin daemon on this computer can serve the app instead."
                }
            } else if io::WEB {
                "Paired with Roostr running on this computer. This browser can keep its own replica instead, synced over your relays without a daemon."
            } else {
                "Served by the glon-odin daemon on this computer. This app can keep its own replica instead, synced over your relays without a daemon."
            },
        ))
        .child(btn_row(
            Btn::action(
                t,
                "switch-mode",
                match (replica, io::WEB) {
                    (true, true) => "Pair with Roostr on this computer",
                    (true, false) => "Use the daemon on this computer",
                    (false, true) => "Use this browser's own replica",
                    (false, false) => "Use this device's own replica",
                },
            )
            .subtle(t)
            .on_press_tx(move |_: &mut State, tx: &mut Tx| {
                let next = if replica { Mode::Daemon } else { Mode::Replica };
                if mode::switch(next) {
                    tx.quit();
                }
            })
            .build(t),
        ));
    holding(s, me, t, vault)
}

/// The settings section builder with no flash.
fn section_plain(t: &Theme, title: &'static str) -> El {
    section(t, title, "")
}

/// Replica only: "This device holds about N% of your vault" and, on a
/// lite replica, "Fully sync this device" behind a details sheet. After
/// confirming, the sync status line shows the download.
fn holding(s: &State, me: &Settings, t: &Theme, vault: El) -> El {
    let vault = vault.child(mode_line(s, me, t)).child(devices_list(me, t));
    if roostr_core::lite::lite(s).is_none() {
        return vault;
    }
    let lite = roostr_core::lite::status(s).is_some();
    // A browser or the iPhone app holding the whole vault can go back to
    // lite: its copy is dropped and it opens again, still signed in.
    let mut vault = if cfg!(any(target_family = "wasm", target_os = "ios")) && !lite {
        switch_to_lite(me, t, vault)
    } else {
        vault
    };
    let progress = sync_progress(&s.store.status).map(|p| progress_text(&p));
    vault = vault.child(p(t, lite_ui::percent_text(me.estimate.as_ref())).mt(10.0));
    if lite && !me.confirm_full && !me.full_requested {
        vault = vault.child(btn_row(
            Btn::action(t, "sync-fully", "Fully sync this device")
                .subtle(t)
                .on_press(|s: &mut State| edit(s, |me| me.confirm_full = true))
                .build(t),
        ));
    }
    if lite && me.confirm_full {
        let lines = match &me.estimate {
            Some(e) => lite_ui::estimate_lines(e),
            None => vec!["Estimating what a full sync downloads…".into()],
        };
        vault = vault.child(
            card("sync-fully-panel", t.border, Some(t.bg), 4.0)
                .padding(12.0)
                .mt(8.0)
                .children(lines.into_iter().map(|l| p(t, l)))
                .child(
                    row()
                        .mt(6.0)
                        .gap(8.0)
                        .child(
                            Btn::action(t, "sync-fully-confirm", "Fully sync this device")
                                .armed()
                                .on_press(|s: &mut State| {
                                    edit(s, |me| {
                                        me.confirm_full = false;
                                        me.full_requested = true;
                                    });
                                    lite_ui::sync_fully(s);
                                })
                                .build(t),
                        )
                        .child(
                            Btn::small(t, "sync-fully-cancel", "Cancel")
                                .on_press(|s: &mut State| edit(s, |me| me.confirm_full = false))
                                .build(t),
                        ),
                ),
        );
    }
    if me.full_requested {
        vault = vault.child(
            p(
                t,
                progress.unwrap_or_else(|| "Fully syncing this device…".into()),
            )
            .mt(6.0),
        );
    }
    vault
}

/// "Switch to Lite" and its confirm sheet, under the mode line of a full
/// replica in a browser or the iPhone app.
fn switch_to_lite(me: &Settings, t: &Theme, vault: El) -> El {
    if !me.confirm_lite {
        return vault
            .child(p(t, "This device holds your whole vault. Lite keeps only what you open, which is lighter for a big vault.").mt(10.0))
            .child(btn_row(
                Btn::action(t, "switch-lite", "Switch to Lite")
                    .subtle(t)
                    .on_press(|s: &mut State| {
                        edit(s, |me| {
                            me.confirm_lite = true;
                            me.lite_error = None;
                        })
                    })
                    .build(t),
            ));
    }
    let mut panel = card("switch-lite-panel", t.border, Some(t.bg), 4.0)
        .padding(12.0)
        .mt(8.0)
        .child(p(t, "This device's copy of your vault is removed and Roostr opens again in Lite: you stay signed in, and objects download as you open them. Nothing is deleted from your vault or your other devices."));
    if let Some(error) = &me.lite_error {
        panel = panel.child(text(error.clone()).font(ty::ui(13.0)).color(t.red).wrap());
    }
    vault.child(
        panel.child(
            row()
                .mt(6.0)
                .gap(8.0)
                .child(
                    Btn::action(t, "switch-lite-confirm", "Switch to Lite")
                        .armed()
                        .on_press(|s: &mut State| {
                            lite_ui::switch_to_lite(s, |result, s| {
                                if let Err(error) = result {
                                    edit(s, |me| me.lite_error = Some(error));
                                }
                            })
                        })
                        .build(t),
                )
                .child(
                    Btn::small(t, "switch-lite-cancel", "Cancel")
                        .on_press(|s: &mut State| {
                            edit(s, |me| {
                                me.confirm_lite = false;
                                me.lite_error = None;
                            })
                        })
                        .build(t),
                ),
        ),
    )
}

/// One line at the top of "Where your vault lives": which mode this
/// device is in (Full via the daemon, Full replica, Lite replica) and
/// where its sync stands - so neither has to be inferred from the rest.
fn mode_line(s: &State, me: &Settings, t: &Theme) -> El {
    let status = &s.store.status;
    let replica = roostr_core::lite::lite(s).is_some();
    let lite = status.lite.is_some();
    let (mode, mode_color) = match (replica, lite) {
        (true, true) => ("Lite", t.orange),
        _ => ("Full", t.green),
    };
    let progress = sync_progress(status).map(|p| progress_text(&p));
    let (state, state_color) = match status.phase {
        SyncPhase::Error => (
            format!(
                "Sync error{}",
                status
                    .detail
                    .as_deref()
                    .map(|d| format!(": {d}"))
                    .unwrap_or_default()
            ),
            t.red,
        ),
        _ if progress.is_some() => (progress.unwrap_or_default(), t.orange),
        _ if !replica => (
            "Holds your whole vault (this computer's engine)".to_string(),
            t.green,
        ),
        _ if lite => (
            match &me.estimate {
                Some(e) => format!(
                    "About {}% on this device · the rest downloads as you open it",
                    e.percent()
                ),
                None => "Part of your vault on this device · the rest downloads as you open it"
                    .to_string(),
            },
            t.muted,
        ),
        SyncPhase::Live if status.bootstrapped => ("100% synced · up to date".to_string(), t.green),
        SyncPhase::Live | SyncPhase::Backfill => ("Syncing…".to_string(), t.orange),
        SyncPhase::Idle => ("Not connected to a relay".to_string(), t.muted),
    };
    // The pending count rolls as changes go out.
    let pending = status.pending.filter(|n| *n > 0).map(|n| {
        roostr_ui::motion::rolling_text(
            t,
            "settings-pending",
            format!(
                " · {n} change{} not sent yet",
                if n == 1 { "" } else { "s" }
            ),
            n as f64,
            ty::ui(13.0),
            state_color,
        )
    });
    row()
        .mt(10.0)
        .center_y()
        .gap(8.0)
        .flex_wrap()
        .child(pill(format!("{mode} mode"), mode_color))
        .child(
            row()
                .center_y()
                .flex_wrap()
                .grow(1.0)
                .min_w(0.0)
                .child(
                    text(state)
                        .font(ty::ui(13.0))
                        .color(state_color)
                        .wrap()
                        .shrink()
                        .min_w(0.0),
                )
                .children(pending),
        )
}

/// "Your devices" under the mode line: this device first, then the most
/// recent; devices unseen for 30 days fold under "Older devices".
fn devices_list(me: &Settings, t: &Theme) -> El {
    let list = col()
        .mt(14.0)
        .gap(6.0)
        .items_stretch()
        .child(h3(t, "Your devices"));
    if me.devices.is_empty() {
        return list.child(hint(t, "No devices yet"));
    }
    let now = chrono::Utc::now().timestamp_millis();
    let (shown, older) = devices::arrange(me.devices.clone(), &me.device_id, now);
    let older_count = older.len();
    let open = me.show_older_devices;
    let mut list = list.children(shown.iter().map(|r| device(me, t, r, now)));
    if older_count > 0 {
        list = list.child(
            row().child(
                Btn::action(
                    t,
                    "devices-older",
                    format!(
                        "{} Older devices ({older_count})",
                        if open { "▾" } else { "▸" }
                    ),
                )
                .subtle(t)
                .on_press(|s: &mut State| {
                    edit(s, |me| me.show_older_devices = !me.show_older_devices)
                })
                .build(t),
            ),
        );
        if open {
            list = list.children(older.iter().map(|r| device(me, t, r, now)));
        }
    }
    list.child(hint(
        t,
        "Remove deletes a record only - the device adds itself again the next time it runs.",
    ))
}

fn device(me: &Settings, t: &Theme, record: &DeviceRecord, now: i64) -> El {
    let this_device = record.device_id() == me.device_id;
    let name = if this_device {
        format!("{} (this device)", record.name)
    } else {
        record.name.clone()
    };
    let (badge, color) = if record.lite {
        ("Lite", t.orange)
    } else {
        ("Full", t.green)
    };
    let id = record.id.clone();
    let renaming = me.renaming_device.as_deref() == Some(record.id.as_str());
    let row_card = if record.lite {
        card(
            format!("device-{id}"),
            t.orange.alpha(0.4),
            Some(t.orange.alpha(0.06)),
            6.0,
        )
    } else {
        card(format!("device-{id}"), t.border, None, 6.0)
    };
    if renaming {
        return row_card.child(
            row()
                .center_y()
                .gap(8.0)
                .child(
                    Field::new(
                        format!("device-name-{id}"),
                        &me.device_name_draft,
                        FieldStyle::modal(t),
                    )
                    .placeholder("Device name")
                    .on_edit(|s: &mut State| &mut me_or_dummy(s).device_name_draft)
                    .on_submit(|s: &mut State, _: &mut Tx| {
                        if settings_open(s) {
                            save_device_name(s)
                        }
                    })
                    .build(t)
                    .grow(1.0),
                )
                .child(
                    Btn::small(t, format!("device-save-{id}"), "Save")
                        .on_press(|s: &mut State| {
                            if settings_open(s) {
                                save_device_name(s)
                            }
                        })
                        .build(t),
                )
                .child(
                    Btn::action(t, format!("device-cancel-{id}"), "Cancel")
                        .subtle(t)
                        .on_press(|s: &mut State| edit(s, |me| me.renaming_device = None))
                        .build(t),
                ),
        );
    }
    let rename_record = record.clone();
    let mut line = row()
        .center_y()
        .gap(8.0)
        .flex_wrap()
        .child(text(name).font(ty::ui(14.0)).color(t.fg))
        .child(pill(badge, color));
    if record.lite {
        line = line.child(
            text(format!("{}% held", record.percent))
                .font(ty::ui(13.0))
                .color(t.orange),
        );
    }
    line = line.child(
        text(format!(
            "last seen {}",
            devices::last_seen_text(now, record.last_seen)
        ))
        .font(ty::ui(13.0))
        .color(t.muted),
    );
    row_card.child(line).child(
        row()
            .gap(8.0)
            .child(
                Btn::action(t, format!("device-rename-{id}"), "Rename")
                    .subtle(t)
                    .on_press(move |s: &mut State| start_rename_device(s, &rename_record))
                    .build(t),
            )
            .child(
                Btn::action(t, format!("device-remove-{id}"), "Remove")
                    .subtle(t)
                    .tooltip("Removes this record only - the device adds itself again the next time it runs.")
                    .on_press(move |s: &mut State| {
                        if settings_open(s) {
                            remove_device(s, &id)
                        }
                    })
                    .build(t),
            ),
    )
}

fn desk_section(s: &State, me: &Settings, t: &Theme) -> El {
    let paired = s.harness.is_some();
    let checking = me.desk == Desk::Checking;
    let desk_color = match me.desk {
        Desk::Match => t.green,
        Desk::Differ => t.orange,
        _ => t.muted,
    };
    let mut desk = section_plain(t, "Desktop sync check")
        .child(p(
            t,
            "Compare the active vault fingerprint with the explicitly paired local daemon. Equal digests mean identical change sets; in native mode both refer to that daemon.",
        ))
        .child(
            row().mt(8.0).child(
                Btn::small(t, "compare", if checking { "Checking…" } else { "Compare" })
                    .when(checking, |b| b.leading(spinner(t, t.fg)))
                    .disabled(!paired || checking)
                    .on_press(|s: &mut State| {
                        if settings_open(s) {
                            check_desktop(s)
                        }
                    })
                    .build(t),
            ),
        );
    if !paired {
        desk = desk.child(
            p(t, "Pair with a desktop under This machine first - until then there is nothing to compare with.")
                .mt(8.0),
        );
    }
    if paired && me.desk != Desk::Idle {
        desk = desk.child(
            text(me.desk_detail.clone())
                .font(ty::ui(12.0).lh(18.0))
                .color(desk_color)
                .wrap()
                .mt(8.0)
                .mb(10.0),
        );
    }
    desk
}

fn dictionary_section(me: &Settings, t: &Theme) -> El {
    let dictionary = section_plain(t, "Dictionary").child(p(
        t,
        "Words you added to the dictionary from the editor's spellcheck. Remove one to flag it again.",
    ));
    if me.ignored.is_empty() {
        return dictionary.child(
            hint(
                t,
                "Nothing added yet — right-click a flagged word and choose \"Add to dictionary\".",
            )
            .mt(4.0)
            .mb(4.0),
        );
    }
    dictionary.child(
        row()
            .flex_wrap()
            .gap(6.0)
            .children(me.ignored.iter().map(|w| {
                let word = w.clone();
                row()
                    .key(format!("ignored-{w}"))
                    .center_y()
                    .gap(2.0)
                    .pl(12.0)
                    .pr(4.0)
                    .py(2.0)
                    .radius(999.0)
                    .fill(t.bg)
                    .border(1.0, t.border)
                    .child(text(w.clone()).font(ty::mono(13.0)).color(t.fg))
                    .child(
                        Btn::x(t, format!("unignore-{w}"))
                            .tooltip("Remove")
                            .on_press(move |s: &mut State| {
                                if settings_open(s) {
                                    unignore(s, &word)
                                }
                            })
                            .build(t),
                    )
            })),
    )
}

fn export_section(me: &Settings, t: &Theme, layer: &mut Vec<El>) -> El {
    let mut controls = row()
        .flex_wrap()
        .center_y()
        .gap(10.0)
        .child(select(
            t,
            layer,
            "export-format",
            &me.export_select,
            |s: &mut State| &mut me_or_dummy(s).export_select,
            0.0,
            t.bg,
            |s: &mut State, value: &str, _: &mut Tx| {
                edit(s, |me| {
                    me.export_format = if value == "json" {
                        ExportFormat::Json
                    } else {
                        ExportFormat::Markdown
                    }
                })
            },
        ))
        .child(
            Btn::action(
                t,
                "export",
                if me.exporting {
                    "Exporting…"
                } else {
                    "Export everything"
                },
            )
            .disabled(me.exporting)
            .on_press(|s: &mut State| {
                if settings_open(s) {
                    run_export(s)
                }
            })
            .build(t),
        );
    if !me.export_note.is_empty() {
        controls = controls.child(hint(t, me.export_note.clone()));
    }
    section_plain(t, "Export")
        .child(p(
            t,
            "Download everything as a zip — one file per object, grouped by type. Markdown for reading and portability; JSON for a complete structural backup (fields, blocks, provenance).",
        ))
        .child(controls)
}

fn plan_section(s: &State, me: &Settings, t: &Theme, layer: &mut Vec<El>) -> El {
    let paired = s.harness.is_some();
    let mut plan_section = section_plain(t, "Claude plan");
    let Some(plan) = me.plan.as_ref().filter(|_| paired) else {
        return plan_section.child(p(
            t,
            if !paired {
                "Pair with a computer under This machine first: the Claude sign-in opens in Chrome there."
                    .to_string()
            } else if me.plan_error.is_empty() {
                "Checking…".to_string()
            } else {
                me.plan_error.clone()
            },
        ));
    };
    plan_section = plan_section.child(p(
        t,
        "Your Claude plan, on the Anthropic credential of every space you own. Agents use the plan while it works and the credential's API key otherwise.",
    ));
    let mut line = row().flex_wrap().center_y().gap(10.0).mt(6.0).mb(6.0);
    if !plan.account.is_empty() {
        line = line.child(rich(
            t,
            &[Span::Plain("Signed in as "), Span::Bold(&plan.account)],
            t.fg,
            13.0,
            16.0,
        ));
    }
    let start = |s: &mut State| {
        let Some(me) = open_mut(s) else { return };
        let server = me.plan_server.clone();
        plan_action(s, "/claude-plan/start", Some(json!({ "servedBy": server }))).detach();
    };
    if plan.signing_in {
        line = line
            .child(
                row()
                    .items_start()
                    .child(spinner(t, t.muted))
                    .child(
                        text(format!(
                            "Sign in to Claude in the browser that opened on {} and press Authorize. If claude.ai shows a code instead, paste it below.",
                            plan_computer(me, &plan.this_computer)
                        ))
                        .font(ty::ui(12.0).lh(18.0))
                        .color(t.muted)
                        .wrap(),
                    )
                    .shrink()
                    .min_w(0.0),
            )
            // The window was closed, or opened in the wrong browser: stop
            // waiting, or open a fresh sign-in.
            .child(
                Btn::small(t, "plan-restart", "Open sign-in again")
                    .subtle(t)
                    .disabled(me.plan_busy)
                    .on_press(start)
                    .build(t),
            )
            .child(
                Btn::small(t, "plan-cancel", "Cancel")
                    .subtle(t)
                    .disabled(me.plan_busy)
                    .on_press(|s: &mut State| {
                        if settings_open(s) {
                            plan_action(s, "/claude-plan/cancel", None).detach()
                        }
                    })
                    .build(t),
            );
    } else {
        line = line.child(
            Btn::action(
                t,
                "plan-sign-in",
                if plan.account.is_empty() {
                    "Sign in with Claude"
                } else {
                    "Sign in again"
                },
            )
            .disabled(me.plan_busy)
            .on_press(start)
            .build(t),
        );
        if !plan.account.is_empty() {
            line = line.child(
                Btn::small(t, "plan-sign-out", "Sign out")
                    .subtle(t)
                    .disabled(me.plan_busy)
                    .on_press(|s: &mut State| {
                        if settings_open(s) {
                            plan_action(s, "/claude-plan/signout", None).detach()
                        }
                    })
                    .build(t),
            );
        }
    }
    plan_section = plan_section.child(line);
    let has_account = !plan.account.is_empty();
    plan_section = plan_section.child(
        row()
            .flex_wrap()
            .center_y()
            .gap(8.0)
            .mt(6.0)
            .mb(6.0)
            .child(text("Kept fresh by").font(ty::ui(13.0)).color(t.fg))
            .child(select(
                t,
                layer,
                "plan-server",
                &me.plan_server_select,
                |s: &mut State| &mut me_or_dummy(s).plan_server_select,
                0.0,
                t.bg,
                move |s: &mut State, machine: &str, _: &mut Tx| {
                    let Some(me) = open_mut(s) else { return };
                    me.plan_server = machine.to_string();
                    if has_account {
                        plan_action(s, "/claude-plan/serve", Some(json!({ "machine": machine })))
                            .detach();
                    }
                },
            ))
            .child(hint(
                t,
                "One always-on computer renews the plan for every space.",
            )),
    );
    if !plan.sign_in_error.is_empty() {
        plan_section = plan_section.child(error_p(t, plan.sign_in_error.clone()));
    }
    if plan.signing_in || !plan.sign_in_error.is_empty() {
        // claude.ai shows a code when it can't hand it back by itself: it goes here.
        plan_section = plan_section.child(
            row()
                .gap(8.0)
                .mt(8.0)
                .child(
                    Field::new("settings-plan-code", &me.plan_code, FieldStyle::modal(t))
                        .placeholder("Paste the code from claude.ai")
                        .on_edit(|s: &mut State| &mut me_or_dummy(s).plan_code)
                        .on_submit(|s: &mut State, _: &mut Tx| {
                            if settings_open(s) {
                                submit_plan_code(s)
                            }
                        })
                        .build(t)
                        .grow(1.0),
                )
                .child(
                    Btn::small(t, "plan-finish", "Finish")
                        .subtle(t)
                        .disabled(me.plan_busy)
                        .on_press(|s: &mut State| {
                            if settings_open(s) {
                                submit_plan_code(s)
                            }
                        })
                        .build(t),
                ),
        );
    }
    if has_account {
        plan_section = plan_section.child(col().mt(8.0).items_stretch().children(
            plan.spaces.iter().map(|sp| {
                let active = sp.status == "active";
                let mut state = if active {
                    if sp.auth == "plan" {
                        "Plan".to_string()
                    } else {
                        "API key".to_string()
                    }
                } else {
                    "Not working".to_string()
                };
                if !sp.error.is_empty() {
                    state.push_str(&format!(" - {}", sp.error));
                }
                row()
                    .key(format!("plan-space-{}", sp.id))
                    .justify_between()
                    .gap(12.0)
                    .py(3.0)
                    .border_sides([0.0, 0.0, 1.0, 0.0], t.border)
                    .child(text(sp.name.clone()).font(ty::ui(13.0)).color(t.fg))
                    // `.hint.error` draws muted (see `error_p`).
                    .child(hint(t, state))
            }),
        ));
    }
    if !me.plan_error.is_empty() {
        plan_section = plan_section.child(error_p(t, me.plan_error.clone()));
    }
    plan_section
}

fn agent_section(s: &State, me: &Settings, t: &Theme) -> El {
    let paired = s.harness.is_some();
    let mut agent = section(t, "Agent", me.agent_saved);
    match &me.agent_auth {
        None => agent = agent.child(p(t, "Checking agent daemon…")),
        Some(None) => {
            agent = agent.child(p(
                t,
                if paired {
                    "The paired agent daemon is unavailable. Start the local harness to manage credentials."
                } else {
                    "Pair under This machine to manage agent credentials. Browser data and relay chat remain available without pairing."
                },
            ));
            if paired {
                agent = agent.child(btn_row(
                    Btn::action(t, "retry-agent", "Retry agent connection")
                        .subtle(t)
                        .on_press(|s: &mut State| {
                            if settings_open(s) {
                                load_agent_auth(s)
                            }
                        })
                        .build(t),
                ));
            }
        }
        Some(Some(auth)) => {
            agent = agent.child(note(
                t,
                &[
                    Span::Plain("Keys for this computer only ("),
                    Span::Code("~/.glon/auth.json"),
                    Span::Plain(
                        "), for agents that list no credential of their own. Claude for your spaces is under Claude plan above.",
                    ),
                ],
                t.muted,
            ));
            let mut provider = row().flex_wrap().center_y().gap(10.0).py(6.0).child(
                text("Kimi (Moonshot)")
                    .font(ty::ui(13.0))
                    .color(t.fg)
                    .w(130.0),
            );
            if auth.kimi.mode == "api_key" {
                provider = provider
                    .child(
                        text(format!(
                            "API key set ({})",
                            auth.kimi.masked.clone().unwrap_or_default()
                        ))
                        .font(ty::mono(12.0))
                        .color(t.muted)
                        .min_w(0.0),
                    )
                    .child(
                        Btn::small(t, "clear-kimi", "Clear")
                            .on_press(|s: &mut State| {
                                if settings_open(s) {
                                    save_agent_key(s, "kimi", String::new())
                                }
                            })
                            .build(t),
                    );
            } else {
                let save = |s: &mut State| {
                    let Some(me) = open_mut(s) else { return };
                    let key = me.kimi_draft.value().to_string();
                    if !key.trim().is_empty() {
                        save_agent_key(s, "kimi", key);
                    }
                };
                provider = provider.child(
                    row()
                        .grow(1.0)
                        .gap(8.0)
                        .child(
                            Field::new("settings-kimi", &me.kimi_draft, FieldStyle::modal(t))
                                .placeholder("sk-…")
                                .on_edit(|s: &mut State| &mut me_or_dummy(s).kimi_draft)
                                .on_submit(move |s: &mut State, _: &mut Tx| save(s))
                                .build(t)
                                .grow(1.0),
                        )
                        .child(Btn::small(t, "save-kimi", "Save").on_press(save).build(t)),
                );
            }
            agent = agent.child(provider);
        }
    }
    if !me.login_error.is_empty() {
        agent = agent.child(error_p(t, me.login_error.clone()));
    }
    agent
}

/// The open panel for a field lens; a closed modal's late keystroke lands in
/// a throwaway state instead of panicking.
fn me_or_dummy(s: &mut State) -> &mut Settings {
    let slot = s.slot::<SettingsModal>();
    if slot.open.is_none() {
        slot.open = Some(Box::new(Settings::new_closed()));
        slot.generation += 1;
    }
    slot.open.as_deref_mut().expect("just ensured")
}

impl Settings {
    /// A panel no loader writes to (see [`me_or_dummy`]).
    fn new_closed() -> Self {
        Self {
            settings_error: String::new(),
            desk: Desk::Idle,
            desk_detail: String::new(),
            npub: String::new(),
            copied: "",
            importing: false,
            confirm_logout: false,
            nsec: String::new(),
            import_draft: InputState::new(),
            import_error: String::new(),
            spy: Spy::default(),
            relays: Vec::new(),
            new_relay: InputState::new(),
            save_state: "",
            agent_auth: None,
            kimi_draft: InputState::new(),
            agent_saved: "",
            login_error: String::new(),
            plan: None,
            plan_machines: Vec::new(),
            plan_server: String::new(),
            plan_server_select: SelectState::new(Vec::new(), None),
            plan_code: InputState::new(),
            plan_error: String::new(),
            plan_busy: false,
            plan_poll: None,
            profile_picture: String::new(),
            profile_name: InputState::new(),
            name_dirty: false,
            saved_name: String::new(),
            avatar_busy: false,
            avatar_state: String::new(),
            chat_color_draft: InputState::new(),
            color_dirty: false,
            chat_color_state: String::new(),
            swatch: ColorPickerState::new(),
            theme_select: SelectState::new(Vec::new(), None),
            ignored: Vec::new(),
            export_format: ExportFormat::Markdown,
            export_select: SelectState::new(Vec::new(), None),
            exporting: false,
            export_note: String::new(),
            estimate: None,
            confirm_full: false,
            full_requested: false,
            confirm_lite: false,
            lite_error: None,
            devices: Vec::new(),
            device_id: String::new(),
            show_older_devices: false,
            renaming_device: None,
            device_name_draft: InputState::new(),
            sheet_y: 0.0,
            sheet_drag: false,
        }
    }
}

// ----------------------------------------------------------------- the modal

/// The modal's sections in order, each under the title the navigation lists.
fn sections(s: &State, me: &Settings, t: &Theme, layer: &mut Vec<El>) -> Vec<(&'static str, El)> {
    let mut sections = vec![
        ("Profile", profile_section(s, me, t)),
        ("Chat color", chat_section(me, t, layer)),
        ("Appearance", appearance_section(me, t, layer)),
        ("Nostr identity", identity_section(s, me, t)),
        ("Relays", relays_section(me, t)),
    ];
    // The iOS app is replica-only: there is no daemon to switch to.
    if !cfg!(target_os = "ios") {
        sections.push(("Where your vault lives", vault_section(s, me, t)));
    }
    sections.extend([
        ("Desktop sync check", desk_section(s, me, t)),
        ("Dictionary", dictionary_section(me, t)),
        ("Export", export_section(me, t, layer)),
        ("Claude plan", plan_section(s, me, t, layer)),
        ("Agent", agent_section(s, me, t)),
    ]);
    sections
}

/// The Settings modal while open: dimmed backdrop and the panel (desktop),
/// or a bottom sheet (≤720px wide). Pushes nothing while closed.
pub fn settings_modal(s: &State, cx: &mut Cx<State>, t: &Theme, layer: &mut Vec<El>) {
    let Some(modal) = s.get::<SettingsModal>() else {
        return;
    };
    let Some(me) = modal.open.as_deref() else {
        return;
    };
    // The window's size: the layer host below spans it (Rune re-runs the
    // view in the same commit once it is laid out).
    let [vw, vh] = cx
        .layout_size("settings-viewport")
        .unwrap_or(modal.viewport);
    let narrow = vw > 0.0 && roostr_ui::layout::is_mobile(vw);

    // Measures the window for the layout below (the layer spans it).
    layer.push(
        el().key("settings-viewport")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .inert()
            .on_layout(|s: &mut State, r: [f32; 4]| {
                s.slot::<SettingsModal>().viewport = [r[2], r[3]];
            }),
    );

    let mut overlay = Vec::new();
    let mut body = sections(s, me, t, &mut overlay);
    let titles: Vec<&'static str> = body.iter().map(|(title, _)| *title).collect();
    let count = titles.len();
    let active = me.spy.active.min(count.saturating_sub(1));

    let header = row().justify_between().center_y().mb(8.0).child(
        row()
            .center_y()
            .gap(6.0)
            .child(text("⚙️").font(ty::ui(15.0)))
            .child(
                text("Settings")
                    .font(ty::ui(17.0).lh(21.0).weight(ty::BOLD))
                    .color(t.fg),
            ),
    );
    let header = if narrow {
        header
    } else {
        header.child(
            Btn::x(t, "close-settings")
                .on_press(close_settings)
                .build(t),
        )
    };

    // The scrolling column: the error line, the sections, the build stamp.
    let mut column = col()
        .key(section_nav::SCROLL_KEY)
        .shrink()
        .min_w(0.0)
        .min_h(0.0)
        .items_stretch()
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .on_scroll_offset({
            let offset = s.get::<SpyClock>().map(|c| c.0.clone());
            move |at| {
                if let Some(offset) = &offset {
                    offset.set((at.offset[1], at.max[1]));
                }
            }
        })
        .on_layout(|s: &mut State, r: [f32; 4]| {
            let now = s.rt.now();
            let (offset, max) = scroll_now(s);
            edit(s, |me| {
                me.spy.area_at(r[1], now);
                me.spy.scrolled(offset, max, now);
            })
        })
        .pb(22.0);
    // Desktop: beside the panel, taking the rest of the width; phone: as
    // tall as its content, down to what the sheet leaves it.
    column = if narrow {
        column.px(22.0)
    } else {
        column.grow(1.0).pl(8.0).pr(22.0)
    };
    let mut content = col().items_stretch();
    if !me.settings_error.is_empty() {
        content = content.child(error_p(t, me.settings_error.clone()));
    }
    let chip_keys: std::rc::Rc<[String]> =
        titles.iter().map(|t| section_nav::chip_key(t)).collect();
    for (ix, (_, card)) in body.drain(..).enumerate() {
        let chip_keys = chip_keys.clone();
        content = content.child(card.on_layout_tx(
            move |s: &mut State, r: [f32; 4], tx: &mut Tx| card_moved(s, ix, r[1], &chip_keys, tx),
        ));
    }
    content = content.child(
        text(format!("Build {BUILD_STAMP}"))
            .font(ty::ui(11.0))
            .color(t.muted)
            .opacity(0.7)
            .align(rune::Align::Center)
            .mt(14.0)
            .mx(2.0),
    );
    column = column.child(content);
    let header = row().px(22.0).child(header.grow(1.0));

    if !narrow {
        // The header across the top; below it the section panel and the
        // scrolling column side by side.
        let wide = 580.0 + section_nav::PANEL_W - 14.0;
        let room = if vw > 0.0 { vw - 32.0 } else { wide };
        let width = wide.min(room);
        let max_h = if vh > 0.0 { vh * 0.8 } else { 640.0 };
        layer.push(
            el().key("settings-overlay")
                .absolute(0.0, 0.0)
                .w_full()
                .h_full()
                .fill(t.kit.overlay)
                .transition(scrim_enter()),
        );
        let surface = col()
            .key("settings")
            .role("dialog")
            .label("Settings")
            .w(width)
            .max_h(max_h)
            .items_stretch()
            .fill(t.panel)
            .border(1.0, t.border)
            .radius(radius::MODAL)
            .box_shadow(roostr_ui::shadow::modal())
            .clip_rounded(radius::MODAL)
            .child(header.pt(18.0))
            .child(
                row()
                    .shrink()
                    .min_h(0.0)
                    .items_stretch()
                    .child(section_nav::panel(t, &titles, active))
                    .child(column),
            )
            .transition(dialog_enter())
            .instant_changes();
        let surface = roostr_ui::overlay::dismissable(surface, "settings", close_settings, layer);
        // A tenth of the window down: one part above, nine below.
        layer.push(
            col()
                .key("settings-host")
                .absolute(0.0, 0.0)
                .w_full()
                .h_full()
                .px(16.0)
                .items_center()
                .child(el().grow(1.0).h(0.0))
                .child(col().grow(9.0).h(0.0).items_center().child(surface)),
        );
        layer.extend(overlay);
        return;
    }

    // ≤720px: a bottom sheet - pinned to the bottom edge, full width, rounded
    // top, a grabber bar and drag-to-close; the × hides (the sheet IS the
    // affordance).
    let grab =
        |e: El| e.on_drag(|s: &mut State, drag: &mut Drag, tx: &mut Tx| sheet_drag(s, drag, tx));
    let sheet = col()
        .key("settings")
        .role("dialog")
        .label("Settings")
        .w_full()
        .max_h((vh - 48.0).max(0.0))
        .items_stretch()
        .fill(t.panel)
        .border_sides([1.0, 1.0, 0.0, 1.0], t.border)
        .corners([18.0, 18.0, 0.0, 0.0])
        .box_shadow(roostr_ui::shadow::modal())
        .offset(0.0, me.sheet_y)
        .child(grab(
            row()
                .key("settings-grab")
                .justify_center()
                .pt(10.0)
                .pb(6.0)
                .child(el().w(40.0).h(4.0).radius(2.0).fill(t.border)),
        ))
        .child(grab(header))
        .child(section_nav::chips(t, &titles, active))
        .child(column)
        .transition(Transition {
            insertion: Effect {
                offset: rune::Vec2::new(0.0, vh.max(1.0)),
                ..Effect::IDENTITY
            },
            removal: Effect::opacity(0.0),
            springs: SpringPair::new(roostr_ui::motion::SHEET_OPEN, Spring::NONE),
            ..Transition::identity()
        });
    let sheet = if me.sheet_drag {
        sheet
    } else {
        sheet.motion(roostr_ui::motion::SHEET_SNAP.spring())
    };
    layer.push(
        el().key("settings-overlay")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .fill(t.kit.overlay)
            .transition(Transition::opacity()),
    );
    let sheet = roostr_ui::overlay::dismissable(sheet, "settings", close_settings, layer);
    layer.push(
        col()
            .key("settings-host")
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .justify_end()
            .child(sheet),
    );
    layer.extend(overlay);
}

/// The dialog's entrance: down from above while fading in, landing on
/// [`roostr_ui::motion::POP`] with a slight bounce; it leaves at once.
fn dialog_enter() -> Transition {
    roostr_ui::motion::pop(Effect {
        offset: rune::Vec2::new(0.0, -48.0),
        ..Effect::opacity(0.0)
    })
}

/// The dialog's scrim: only fades in (250ms, no bounce); gone at once.
fn scrim_enter() -> Transition {
    Transition {
        insertion: Effect::opacity(0.0),
        removal: Effect::IDENTITY,
        springs: SpringPair::new(Spring::new(0.25, 0.0), Spring::NONE),
        ..Transition::identity()
    }
}

/// The grab zone and header's drag (`sheetStart`/`sheetMove`/`sheetEnd`):
/// pull the sheet down past 110px to close it, else it springs back.
fn sheet_drag(s: &mut State, drag: &Drag, tx: &mut Tx) {
    let Some(me) = open_mut(s) else { return };
    match drag.phase {
        Phase::Down => {
            me.sheet_drag = true;
        }
        Phase::Move => {
            me.sheet_y = drag.dy.max(0.0);
            tx.instant();
        }
        Phase::Up => {
            me.sheet_drag = false;
            if me.sheet_y > SHEET_CLOSE {
                close_settings(s);
            } else {
                me.sheet_y = 0.0;
            }
        }
    }
}

/// The column's scroll position, which its scroll hook writes outside any
/// handler (it can't change state).
#[derive(Default)]
pub(crate) struct SpyClock(std::rc::Rc<std::cell::Cell<(f32, f32)>>);

fn scroll_now(s: &mut State) -> (f32, f32) {
    s.slot::<SpyClock>().0.get()
}

/// A section card moved (layout or scroll): update the scroll-spy and keep
/// the active chip in view.
fn card_moved(s: &mut State, ix: usize, top: f32, chip_keys: &[String], tx: &mut Tx) {
    let now = s.rt.now();
    let (offset, max) = scroll_now(s);
    let narrow = s
        .get::<SettingsModal>()
        .is_some_and(|m| m.viewport[0] > 0.0 && roostr_ui::layout::is_mobile(m.viewport[0]));
    let Some(me) = open_mut(s) else { return };
    let before = me.spy.active;
    me.spy.scrolled(offset, max, now);
    me.spy.card_at(ix, chip_keys.len(), top, now);
    let active = me.spy.active;
    if active != before
        && narrow
        && let Some(key) = chip_keys.get(active)
    {
        tx.scroll_into_view(key.clone());
    }
}
