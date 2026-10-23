//! The Settings modal (`Settings.svelte`) as the local-mode web app shows
//! it: profile, chat colour, Nostr identity, relays, the desktop sync check,
//! the spellcheck dictionary, export, the Claude plan and agent keys.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use futures::FutureExt as _;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::avatar::Avatar;
use gpui_kit::component::color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState};
use gpui_kit::component::group_box::{GroupBox, GroupBoxVariants as _};
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::tag::Tag;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::color::{DEFAULT_CHAT_COLOR, hex, normalize_hex};
use roostr_core::devices::{self, DEVICE_ID_PREFIX, DeviceRecord};
use roostr_core::harness::harness;
use roostr_core::io;
use roostr_core::lite::VaultEstimate;
use roostr_core::profile as nostr_profile;
use roostr_core::serving::{MachineRow, fetch_machines, machine_name};
use roostr_core::sync_status::{progress_text, sync_progress};
use roostr_core::ui::data_url_image;
use roostr_core::{Glyph, Store, StoreEvent, UiState, api, lite_ui, motion, theme};
use serde_json::{Value as Json, json};

use crate::export::{ExportFormat, export_all};
use crate::local::{self, AgentAuth, PlanStatus, with_status};
use crate::profile;
use crate::widgets::{
    Btn, FieldStyle, Flash, Select, Span, field, h3, hint, rich, spinner, switch,
};

/// The editor's "Add to dictionary" list (web localStorage `spell-ignore`).
const IGNORE_KEY: &str = "spell-ignore";
/// Gives up on a desktop that doesn't answer, so the check never sits on "Checking…" for good.
const DESK_TIMEOUT: Duration = Duration::from_secs(15);
/// Follow a Claude sign-in this often until it finishes.
const PLAN_POLL: Duration = Duration::from_millis(2000);
const FLASH: Duration = Duration::from_millis(1500);
/// The mobile sheet rising into place on open (a soft `ease`).
const SHEET_ENTER: motion::Tween = motion::Tween::ease(260);
const BUILD_STAMP: &str = env!("ROOSTR_BUILD_STAMP");

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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Desk {
    Idle,
    Checking,
    Match,
    Differ,
    Offline,
}

/// The settings panel (the modal's content).
pub struct Settings {
    focus: FocusHandle,
    scroll: ScrollHandle,
    settings_error: SharedString,
    desk: Desk,
    desk_detail: SharedString,

    // Nostr identity
    npub: String,
    copied: &'static str,
    importing: bool,
    confirm_logout: bool,
    /// Replica mode: the revealed private key ("" = hidden).
    nsec: SharedString,
    import_draft: Entity<InputState>,
    import_error: SharedString,
    /// The settings scroll area's height the import field was last kept in
    /// view at, while it has focus (see [`keep_in_view`]).
    import_in_view: Rc<Cell<Option<Pixels>>>,

    // Relays
    relays: Vec<String>,
    new_relay: Entity<InputState>,
    save_state: &'static str,
    save_flash: Flash,

    // Agent credentials: `None` while checking, `Some(None)` when unavailable.
    agent_auth: Option<Option<AgentAuth>>,
    kimi_draft: Entity<InputState>,
    agent_saved: &'static str,
    agent_flash: Flash,
    login_error: SharedString,

    // Claude plan
    plan: Option<PlanStatus>,
    plan_machines: Vec<MachineRow>,
    /// The computer to keep the plan fresh: this one, until changed.
    plan_server: String,
    plan_code: Entity<InputState>,
    plan_error: SharedString,
    plan_busy: bool,
    plan_poll: Option<Task<()>>,

    // Profile
    profile_picture: String,
    profile_image: Option<Arc<Image>>,
    profile_name: Entity<InputState>,
    /// The name as last saved, so an unchanged field never republishes the profile.
    saved_name: String,
    avatar_busy: bool,
    avatar_state: SharedString,

    // Chat colour
    chat_color_draft: Entity<InputState>,
    chat_color_state: SharedString,
    swatch: Entity<ColorPickerState>,

    // Dictionary
    ignored: Vec<String>,

    // Export
    export_format: ExportFormat,
    exporting: bool,
    export_note: SharedString,

    // Lite: how much of the vault this device holds (`None` while estimating),
    // and the "Fully sync this device" details sheet.
    estimate: Option<VaultEstimate>,
    confirm_full: bool,
    full_requested: bool,

    // "Your devices": every device record of the account, this backend's
    // device id (from the settings), the record being renamed and its draft.
    devices: Vec<DeviceRecord>,
    device_id: String,
    show_older_devices: bool,
    renaming_device: Option<String>,
    device_name_draft: Entity<InputState>,
    /// The mobile sheet's drag-to-close: how far it is pulled down, where
    /// the finger started while it drags, its presented offset (entering
    /// from below, then following the finger 1:1 and settling back over
    /// `sheet_settle` after a short pull).
    sheet_y: Pixels,
    sheet_drag: Option<Pixels>,
    sheet: Option<motion::ValueState>,
    sheet_settle: motion::Tween,
    _subscriptions: Vec<Subscription>,
}

fn input(window: &mut Window, cx: &mut Context<Settings>, placeholder: &str) -> Entity<InputState> {
    let placeholder = placeholder.to_string();
    cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
}

fn to_hex(color: Hsla) -> String {
    let rgba = Rgba::from(color);
    let c = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    format!("#{:02X}{:02X}{:02X}", c(rgba.r), c(rgba.g), c(rgba.b))
}

impl Settings {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let cached = nostr_profile::cached(cx);
        let name = cached.name.clone().unwrap_or_default();
        let chat_color = Store::global(cx).read(cx).chat_color.clone();
        let import_draft = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("nsec1… or 64-char hex")
                .masked(true)
        });
        let kimi_draft = cx.new(|cx| InputState::new(window, cx).placeholder("sk-…").masked(true));
        let new_relay = input(window, cx, "wss://relay.example.com");
        let plan_code = input(window, cx, "Paste the code from claude.ai");
        let profile_name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Your name")
                .default_value(name.clone())
        });
        let chat_color_draft = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(DEFAULT_CHAT_COLOR)
                .default_value(chat_color.clone())
        });
        let swatch_color = hex(&chat_color)
            .or_else(|| hex(DEFAULT_CHAT_COLOR))
            .unwrap_or(theme::white());
        let swatch = cx.new(|cx| ColorPickerState::new(window, cx).default_value(swatch_color));
        let device_name_draft = input(window, cx, "Device name");

        let subscriptions = vec![
            cx.subscribe_in(
                &import_draft,
                window,
                |this, state, event: &InputEvent, _, cx| {
                    if let InputEvent::PressEnter { .. } = event
                        && !state.read(cx).value().trim().is_empty()
                    {
                        this.import_key(cx);
                    }
                },
            ),
            cx.subscribe_in(
                &new_relay,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.add_relay(window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &kimi_draft,
                window,
                |this, state, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        let key = state.read(cx).value().to_string();
                        if !key.trim().is_empty() {
                            this.save_agent_key("kimi", key, window, cx);
                        }
                    }
                },
            ),
            cx.subscribe_in(
                &plan_code,
                window,
                |this, state, event: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.submit_plan_code(state.clone(), window, cx);
                    }
                },
            ),
            // `onchange`: Enter blurs the field, and leaving it saves.
            cx.subscribe_in(
                &profile_name,
                window,
                |this, _, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } => this.focus.focus(window, cx),
                    InputEvent::Blur => this.save_name(cx),
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &chat_color_draft,
                window,
                |this, state, event: &InputEvent, window, cx| match event {
                    InputEvent::PressEnter { .. } => this.focus.focus(window, cx),
                    InputEvent::Blur => {
                        let raw = state.read(cx).value().to_string();
                        this.save_chat_color(raw, window, cx);
                    }
                    InputEvent::Change => cx.notify(),
                    _ => {}
                },
            ),
            cx.subscribe_in(
                &swatch,
                window,
                |this, _, event: &ColorPickerEvent, window, cx| {
                    if let ColorPickerEvent::Change(Some(color)) = event {
                        this.save_chat_color(to_hex(*color), window, cx);
                    }
                },
            ),
            // The sync status line under "Fully sync this device" follows the store.
            cx.observe(&Store::global(cx), |_, _, cx| cx.notify()),
            cx.subscribe_in(
                &device_name_draft,
                window,
                |this, _, event: &InputEvent, _, cx| {
                    if let InputEvent::PressEnter { .. } = event {
                        this.save_device_name(cx);
                    }
                },
            ),
            cx.subscribe(&Store::global(cx), |this, _, event: &StoreEvent, cx| {
                let reload = match event {
                    StoreEvent::Committed(id) => id.starts_with(DEVICE_ID_PREFIX),
                    StoreEvent::Reconnected => true,
                    StoreEvent::Refreshed => false,
                };
                if reload {
                    this.load_devices(cx);
                }
            }),
        ];

        let mut this = Self {
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            settings_error: SharedString::default(),
            desk: Desk::Idle,
            desk_detail: SharedString::default(),
            npub: String::new(),
            copied: "",
            importing: false,
            confirm_logout: false,
            nsec: SharedString::default(),
            import_draft,
            import_error: SharedString::default(),
            import_in_view: Rc::default(),
            relays: Vec::new(),
            new_relay,
            save_state: "",
            save_flash: Flash::default(),
            agent_auth: None,
            kimi_draft,
            agent_saved: "",
            agent_flash: Flash::default(),
            login_error: SharedString::default(),
            plan: None,
            plan_machines: Vec::new(),
            plan_server: String::new(),
            plan_code,
            plan_error: SharedString::default(),
            plan_busy: false,
            plan_poll: None,
            profile_image: cached.picture.as_deref().and_then(data_url_image),
            profile_picture: cached.picture.unwrap_or_default(),
            profile_name,
            saved_name: name,
            avatar_busy: false,
            avatar_state: SharedString::default(),
            chat_color_draft,
            chat_color_state: SharedString::default(),
            swatch,
            ignored: Self::read_ignored(cx),
            export_format: ExportFormat::Markdown,
            exporting: false,
            export_note: SharedString::default(),
            estimate: None,
            confirm_full: false,
            full_requested: false,
            devices: Vec::new(),
            device_id: String::new(),
            show_older_devices: false,
            renaming_device: None,
            device_name_draft,
            sheet_y: px(0.),
            sheet_drag: None,
            sheet: None,
            sheet_settle: SHEET_ENTER,
            _subscriptions: subscriptions,
        };
        this.load_profile(window, cx);
        this.load(cx);
        this.load_estimate(cx);
        this.load_devices(cx);
        this
    }

    fn paired(cx: &App) -> bool {
        harness(cx).is_some()
    }

    /// Clear `field` after the 1.5 s flash.
    fn flash(cx: &mut Context<Self>, clear: impl FnOnce(&mut Self) + 'static) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FLASH).await;
            let _ = this.update(cx, |this, cx| {
                clear(this);
                cx.notify();
            });
        })
        .detach();
    }

    // ------------------------------------------------------------- load

    fn load(&mut self, cx: &mut Context<Self>) {
        self.settings_error = SharedString::default();
        let backend = Store::global(cx).read(cx).backend();
        let task = io::spawn(cx, backend.settings());
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(settings) => {
                        this.relays = settings.relays;
                        this.npub = settings.npub.unwrap_or_default();
                        this.device_id = settings.device_id;
                    }
                    Err(error) => this.settings_error = error.to_string().into(),
                }
                this.load_agent_auth(cx);
                this.load_plan(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// "Your devices": the account's device records, reloaded on open, after
    /// a rename/remove and whenever a device record commits.
    fn load_devices(&mut self, cx: &mut Context<Self>) {
        let task = devices::list(cx);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(records) => this.devices = records,
                    Err(error) => log::warn!("device list failed: {error:#}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn start_rename_device(
        &mut self,
        record: &DeviceRecord,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.renaming_device = Some(record.id.clone());
        let name = record.name.clone();
        self.device_name_draft.update(cx, |s, cx| {
            s.set_value(name, window, cx);
            s.focus(window, cx);
        });
        cx.notify();
    }

    fn save_device_name(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.renaming_device.clone() else {
            return;
        };
        let name = self.device_name_draft.read(cx).value().trim().to_string();
        if name.is_empty() {
            return;
        }
        self.renaming_device = None;
        let task = devices::rename(&id, &name, cx);
        self.after_device_change(task, cx);
    }

    fn remove_device(&mut self, id: &str, cx: &mut Context<Self>) {
        let task = devices::remove(id, cx);
        self.after_device_change(task, cx);
    }

    fn after_device_change<T: 'static>(
        &mut self,
        task: Task<anyhow::Result<T>>,
        cx: &mut Context<Self>,
    ) {
        self.settings_error = SharedString::default();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Err(error) = result {
                    this.settings_error = error.to_string().into();
                }
                this.load_devices(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Replica only: the vault estimate, computed each time settings opens.
    fn load_estimate(&mut self, cx: &mut Context<Self>) {
        let Some(control) = roostr_core::lite::lite(cx) else {
            return;
        };
        let task = io::spawn(cx, async move { control.estimate().await });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(estimate) => this.estimate = Some(estimate),
                    Err(error) => log::warn!("vault estimate failed: {error:#}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.avatar_state = SharedString::default();
        let task = nostr_profile::fetch(cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(p) => this.apply_profile(p, window, cx),
                    Err(error) => this.avatar_state = error.to_string().into(),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn apply_profile(
        &mut self,
        p: nostr_profile::Profile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.profile_picture = p.picture.unwrap_or_default();
        self.profile_image = data_url_image(&self.profile_picture);
        self.saved_name = p.name.unwrap_or_default();
        let name = self.saved_name.clone();
        self.profile_name
            .update(cx, |s, cx| s.set_value(name, window, cx));
    }

    fn read_ignored(cx: &App) -> Vec<String> {
        let mut words: Vec<String> = UiState::get(cx, IGNORE_KEY).unwrap_or_default();
        words.sort();
        words.dedup();
        words
    }

    // ------------------------------------------------------------- profile

    fn pick_avatar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if io::WEB {
            self.avatar_state = io::NO_FILE_PICKER.into();
            cx.notify();
            return;
        }
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.avatar_busy = true;
                this.avatar_state = "saving…".into();
                cx.notify();
            });
            let picture = cx
                .background_spawn(async move { profile::image_to_avatar(&path) })
                .await;
            let result = match picture {
                Ok(picture) => match cx.update(|_, cx| {
                    nostr_profile::save(nostr_profile::patch("picture", &picture), cx)
                }) {
                    Ok(task) => task.await,
                    Err(error) => Err(error),
                },
                Err(error) => Err(error),
            };
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(p) => {
                        this.apply_profile(p, window, cx);
                        this.avatar_state = "saved".into();
                    }
                    Err(error) => this.avatar_state = error.to_string().into(),
                }
                this.avatar_busy = false;
                cx.notify();
            });
        })
        .detach();
    }

    fn save_profile(&mut self, patch: Json, saving: bool, cx: &mut Context<Self>) {
        self.avatar_busy = true;
        if saving {
            self.avatar_state = "saving…".into();
        }
        cx.notify();
        let task = nostr_profile::save(patch, cx);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(p) => {
                        this.profile_picture = p.picture.unwrap_or_default();
                        this.profile_image = data_url_image(&this.profile_picture);
                        this.saved_name = p.name.unwrap_or_default();
                        this.avatar_state = "saved".into();
                    }
                    Err(error) => this.avatar_state = error.to_string().into(),
                }
                this.avatar_busy = false;
                cx.notify();
            });
        })
        .detach();
    }

    fn save_name(&mut self, cx: &mut Context<Self>) {
        let name = self.profile_name.read(cx).value().trim().to_string();
        if name == self.saved_name {
            return;
        }
        self.save_profile(nostr_profile::patch("name", &name), true, cx);
    }

    // ------------------------------------------------------------- chat colour

    fn save_chat_color(&mut self, raw: String, window: &mut Window, cx: &mut Context<Self>) {
        if normalize_hex(&raw).is_none() {
            self.chat_color_state = "Use a hex color like #EDCF7F".into();
            cx.notify();
            return;
        }
        self.chat_color_state = "saving…".into();
        cx.notify();
        let task = api::user_prefs::set_chat_color(&raw, cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(hex_value) => {
                        Store::global(cx).update(cx, |store, cx| {
                            store.chat_color = hex_value.clone();
                            cx.notify();
                        });
                        this.chat_color_draft
                            .update(cx, |s, cx| s.set_value(hex_value.clone(), window, cx));
                        if let Some(color) = hex(&hex_value) {
                            this.swatch
                                .update(cx, |s, cx| s.set_value(color, window, cx));
                        }
                        this.chat_color_state = "saved".into();
                    }
                    Err(error) => this.chat_color_state = error.to_string().into(),
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ------------------------------------------------------------- identity

    fn copy(&mut self, text: String, label: &'static str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copied = label;
        cx.notify();
        Self::flash(cx, |this| this.copied = "");
    }

    fn import_key(&mut self, cx: &mut Context<Self>) {
        self.import_error = SharedString::default();
        let key = self.import_draft.read(cx).value().trim().to_string();
        let task = api::settings::import_key(&key, cx);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(_) => {
                    // The web reloads the page: close Settings and reopen the vault under the new identity.
                    this.importing = false;
                    if let Some(window) = cx.active_window() {
                        let _ = window.update(cx, |_, window, cx| window.close_dialog(cx));
                    }
                    Store::global(cx).update(cx, |store, cx| store.reconnect(cx));
                }
                Err(error) => {
                    this.import_error = error.to_string().into();
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn cancel_import(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.importing = false;
        self.import_draft
            .update(cx, |s, cx| s.set_value("", window, cx));
        self.import_error = SharedString::default();
        cx.notify();
    }

    /// Replica mode: "Log out on this device" (api.ts `settings.logout`).
    fn log_out(&mut self, cx: &mut Context<Self>) {
        self.settings_error = SharedString::default();
        let Some(account) = roostr_core::account::account(cx) else {
            return;
        };
        let task = io::spawn(cx, account.logout());
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => {
                    if let Some(window) = cx.active_window() {
                        let _ = window.update(cx, |_, window, cx| window.close_dialog(cx));
                    }
                    // No key now: the vault reopens on the sign-in gate.
                    Store::global(cx).update(cx, |store, cx| store.reconnect(cx));
                }
                Err(error) => {
                    this.settings_error = error.to_string().into();
                    this.confirm_logout = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Replica mode: "Reveal private key" and, revealed, the nsec to copy.
    fn render_reveal(&self, identity: Div, cx: &mut Context<Self>) -> Div {
        if self.nsec.is_empty() {
            return identity.child(div().mt(px(6.)).mb(px(6.)).flex().child(
                Btn::action("reveal-key", "Reveal private key").on_click(cx.listener(
                    |this, _, _, cx| {
                        match roostr_core::account::account(cx).map(|a| a.export_key()) {
                            Some(Ok((nsec, _))) => this.nsec = nsec.into(),
                            Some(Err(error)) => this.settings_error = error.to_string().into(),
                            None => {}
                        }
                        cx.notify();
                    },
                )),
            ));
        }
        let nsec = self.nsec.to_string();
        identity
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .mt(px(10.))
                    .mb(px(4.))
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .child("Private key")
                    .child(
                        div()
                            .text_size(px(11.))
                            .font_weight(FontWeight::NORMAL)
                            .font_family(theme::fonts::MONO)
                            .text_color(theme::muted())
                            .child("nsec"),
                    )
                    .child(key_tag("SECRET", theme::red(), theme::red())),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .mb(px(6.))
                    .child(
                        div()
                            .id("nsec-code")
                            .flex_1()
                            .min_w_0()
                            .overflow_x_scroll()
                            .px(px(10.))
                            .py(px(7.))
                            .rounded(px(8.))
                            .bg(theme::bg())
                            .border_1()
                            .border_color(theme::border())
                            .font_family(theme::fonts::MONO)
                            .text_size(px(11.))
                            .whitespace_nowrap()
                            .child(self.nsec.clone()),
                    )
                    .child(
                        Btn::small("copy-nsec", if self.copied == "nsec" { "Copied" } else { "Copy" })
                            .on_click(cx.listener(move |this, _, _, cx| this.copy(nsec.clone(), "nsec", cx))),
                    ),
            )
            .child(p(
                "Paste this nsec into \"Sign in with existing key\" on another Roostr to log in as you.",
            ))
            .child(
                div().mt(px(6.)).flex().child(
                    Btn::action("hide-key", "Hide").subtle().on_click(cx.listener(|this, _, _, cx| {
                        this.nsec = SharedString::default();
                        cx.notify();
                    })),
                ),
            )
    }

    /// Replica only: "This device holds about N% of your vault" and, on a
    /// lite replica, "Fully sync this device" behind a details sheet. After
    /// confirming, the sync status line shows the download.
    fn render_holding(&self, vault: Div, window: &Window, cx: &mut Context<Self>) -> Div {
        let vault = vault
            .child(self.render_mode_line(cx))
            .child(self.render_devices(window, cx));
        if roostr_core::lite::lite(cx).is_none() {
            return vault;
        }
        let lite = roostr_core::lite::status(cx).is_some();
        let status = Store::global(cx).read(cx).status.clone();
        let progress = sync_progress(&status).map(|p| progress_text(&p));
        vault
            .child(p(lite_ui::percent_text(self.estimate.as_ref())).mt(px(10.)))
            .when(lite && !self.confirm_full && !self.full_requested, |d| {
                d.child(
                    div().mt(px(6.)).flex().child(
                        Btn::action("sync-fully", "Fully sync this device")
                            .subtle()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.confirm_full = true;
                                cx.notify();
                            })),
                    ),
                )
            })
            .when(lite && self.confirm_full, |d| {
                let lines = match &self.estimate {
                    Some(e) => lite_ui::estimate_lines(e),
                    None => vec!["Estimating what a full sync downloads…".into()],
                };
                d.child(
                    card(
                        "sync-fully-panel",
                        StyleRefinement::default()
                            .p(px(12.))
                            .rounded(px(8.))
                            .border_color(theme::border())
                            .bg(theme::bg())
                            .gap(px(4.)),
                    )
                    .mt(px(8.))
                    .children(lines.into_iter().map(p))
                    .child(
                        div()
                            .mt(px(6.))
                            .flex()
                            .gap(px(8.))
                            .child(
                                Btn::action("sync-fully-confirm", "Fully sync this device")
                                    .armed()
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.confirm_full = false;
                                        this.full_requested = true;
                                        lite_ui::sync_fully(cx);
                                        cx.notify();
                                    })),
                            )
                            .child(Btn::small("sync-fully-cancel", "Cancel").on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.confirm_full = false;
                                    cx.notify();
                                }),
                            )),
                    ),
                )
            })
            .when(self.full_requested, |d| {
                d.child(
                    p(progress.unwrap_or_else(|| "Fully syncing this device…".into())).mt(px(6.)),
                )
            })
    }

    /// One line at the top of "Where your vault lives": which mode this
    /// device is in (Full via the daemon, Full replica, Lite replica) and
    /// where its sync stands - so neither has to be inferred from the rest.
    fn render_mode_line(&self, cx: &mut Context<Self>) -> Div {
        use roostr_core::SyncPhase;
        let status = Store::global(cx).read(cx).status.clone();
        let replica = roostr_core::lite::lite(cx).is_some();
        let lite = status.lite.is_some();
        let (mode, mode_color) = match (replica, lite) {
            (false, _) => ("Full", theme::green()),
            (true, false) => ("Full", theme::green()),
            (true, true) => ("Lite", theme::orange()),
        };
        let progress = sync_progress(&status).map(|p| progress_text(&p));
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
                theme::red(),
            ),
            _ if progress.is_some() => (progress.unwrap_or_default(), theme::orange()),
            _ if !replica => (
                "Holds your whole vault (this computer's engine)".to_string(),
                theme::green(),
            ),
            _ if lite => (
                match &self.estimate {
                    Some(e) => format!(
                        "About {}% on this device · the rest downloads as you open it",
                        e.percent()
                    ),
                    None => "Part of your vault on this device · the rest downloads as you open it"
                        .to_string(),
                },
                theme::muted(),
            ),
            SyncPhase::Live if status.bootstrapped => {
                ("100% synced · up to date".to_string(), theme::green())
            }
            SyncPhase::Live | SyncPhase::Backfill => ("Syncing…".to_string(), theme::orange()),
            SyncPhase::Idle => ("Not connected to a relay".to_string(), theme::muted()),
        };
        let pending = status.pending.filter(|n| *n > 0).map(|n| {
            format!(
                " · {n} change{} not sent yet",
                if n == 1 { "" } else { "s" }
            )
        });
        div()
            .mt(px(10.))
            .flex()
            .items_center()
            .gap(px(8.))
            .flex_wrap()
            .child(pill(format!("{mode} mode"), mode_color))
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(state_color)
                    .child(format!("{state}{}", pending.unwrap_or_default())),
            )
    }

    /// "Your devices" under the mode line: this device first, then the most
    /// recent; devices unseen for 30 days fold under "Older devices".
    fn render_devices(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let list = div()
            .mt(px(14.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(h3("Your devices"));
        if self.devices.is_empty() {
            return list.child(hint("No devices yet"));
        }
        let now = chrono::Utc::now().timestamp_millis();
        let (shown, older) = devices::arrange(self.devices.clone(), &self.device_id, now);
        let older_count = older.len();
        let open = self.show_older_devices;
        let mut list = list.children(shown.iter().map(|r| self.render_device(r, now, window, cx)));
        if older_count > 0 {
            list = list.child(
                div().flex().child(
                    Btn::action(
                        "devices-older",
                        format!(
                            "{} Older devices ({older_count})",
                            if open { "▾" } else { "▸" }
                        ),
                    )
                    .subtle()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_older_devices = !this.show_older_devices;
                        cx.notify();
                    })),
                ),
            );
            if open {
                list = list.children(older.iter().map(|r| self.render_device(r, now, window, cx)));
            }
        }
        list.child(hint(
            "Remove deletes a record only - the device adds itself again the next time it runs.",
        ))
    }

    fn render_device(
        &self,
        record: &DeviceRecord,
        now: i64,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> GroupBox {
        let this_device = record.device_id() == self.device_id;
        let name = if this_device {
            format!("{} (this device)", record.name)
        } else {
            record.name.clone()
        };
        let (badge, color) = if record.lite {
            ("Lite", theme::orange())
        } else {
            ("Full", theme::green())
        };
        let id = record.id.clone();
        let renaming = self.renaming_device.as_deref() == Some(record.id.as_str());
        let surface = StyleRefinement::default()
            .px(px(10.))
            .py(px(8.))
            .rounded(px(8.))
            .gap(px(6.));
        let surface = if record.lite {
            surface
                .border_color(theme::orange().opacity(0.4))
                .bg(theme::orange().opacity(0.06))
        } else {
            surface.border_color(theme::border())
        };
        let row = card(SharedString::from(format!("device-{id}")), surface);
        if renaming {
            return row.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(field(&self.device_name_draft, modal_field(), window, cx).flex_1())
                    .child(
                        Btn::small(SharedString::from(format!("device-save-{id}")), "Save")
                            .on_click(cx.listener(|this, _, _, cx| this.save_device_name(cx))),
                    )
                    .child(
                        Btn::action(SharedString::from(format!("device-cancel-{id}")), "Cancel")
                            .subtle()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.renaming_device = None;
                                cx.notify();
                            })),
                    ),
            );
        }
        let rename_record = record.clone();
        row.child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .flex_wrap()
                .child(div().text_size(px(14.)).text_color(theme::fg()).child(name))
                .child(pill(badge, color))
                .when(record.lite, |d| {
                    d.child(div().text_size(px(13.)).text_color(theme::orange()).child(format!("{}% held", record.percent)))
                })
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(theme::muted())
                        .child(format!("last seen {}", devices::last_seen_text(now, record.last_seen))),
                ),
        )
        .child(
            div()
                .flex()
                .gap(px(8.))
                .child(
                    Btn::action(SharedString::from(format!("device-rename-{id}")), "Rename")
                        .subtle()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.start_rename_device(&rename_record, window, cx)
                        })),
                )
                .child(
                    Btn::action(SharedString::from(format!("device-remove-{id}")), "Remove")
                        .subtle()
                        .tooltip("Removes this record only - the device adds itself again the next time it runs.")
                        .on_click(cx.listener(move |this, _, _, cx| this.remove_device(&id, cx))),
                ),
        )
    }

    // ------------------------------------------------------------- relays

    fn save_relays(&mut self, next: Vec<String>, cx: &mut Context<Self>) {
        self.settings_error = SharedString::default();
        let task = api::settings::set_relays(&next, cx);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(_) => {
                        Store::global(cx).update(cx, |store, _| {
                            if let Some(settings) = store.settings.as_mut() {
                                settings.relays = next.clone();
                            }
                        });
                        this.relays = next;
                        this.save_state = "Saved";
                        Self::flash(cx, |this| this.save_state = "");
                    }
                    Err(error) => this.settings_error = error.to_string().into(),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn add_relay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut v = self.new_relay.read(cx).value().trim().to_string();
        if v.is_empty() {
            return;
        }
        if !v.starts_with("wss://") && !v.starts_with("ws://") {
            v = format!("wss://{v}");
        }
        if !self.relays.contains(&v) {
            let mut next = self.relays.clone();
            next.push(v);
            self.save_relays(next, cx);
        }
        self.new_relay
            .update(cx, |s, cx| s.set_value("", window, cx));
    }

    fn toggle_relay(&mut self, url: &str, on: bool, cx: &mut Context<Self>) {
        let mut next: Vec<String> = self.relays.iter().filter(|r| *r != url).cloned().collect();
        if on {
            next.push(url.to_string());
        }
        self.save_relays(next, cx);
    }

    // ------------------------------------------------------------- desktop check

    /// Compare the vault fingerprint the daemon reports with this app's view
    /// of it. In native mode both refer to the one daemon.
    fn check_desktop(&mut self, cx: &mut Context<Self>) {
        self.desk = Desk::Checking;
        self.desk_detail = "Asking the desktop for its fingerprint…".into();
        cx.notify();
        let backend = Store::global(cx).read(cx).backend();
        let remote = io::spawn(cx, backend.sync_digest());
        let timeout = cx.background_executor().timer(DESK_TIMEOUT);
        cx.spawn(async move |this, cx| {
            let remote = futures::select! {
                r = remote.fuse() => r,
                _ = timeout.fuse() => Err(anyhow::anyhow!(
                    "the desktop didn't answer within {} s - is Roostr running on it?",
                    DESK_TIMEOUT.as_secs()
                )),
            };
            let num = |m: &serde_json::Map<String, Json>, k: &str| m.get(k).and_then(Json::as_i64).unwrap_or(0);
            let remote = match remote {
                Ok(remote) => remote,
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        this.desk = Desk::Offline;
                        this.desk_detail = error.to_string().into();
                        cx.notify();
                    });
                    return;
                }
            };
            let (r_objects, r_changes) = (num(&remote, "objects"), num(&remote, "changes"));
            let _ = this.update(cx, |this, cx| {
                this.desk_detail = format!("Desktop has {r_changes} changes. Counting this device's…").into();
                cx.notify();
            });
            let local = io::spawn_async(cx, backend.sync_digest()).await;
            let _ = this.update(cx, |this, cx| {
                match local {
                    Ok(local) => {
                        let digest = |m: &serde_json::Map<String, Json>| m.get("digest").and_then(Json::as_str).unwrap_or("").to_string();
                        let (l_objects, l_changes) = (num(&local, "objects"), num(&local, "changes"));
                        if digest(&local) == digest(&remote) {
                            this.desk = Desk::Match;
                            this.desk_detail = format!("identical — {r_objects} objects, {r_changes} changes").into();
                        } else {
                            this.desk = Desk::Differ;
                            let who = if l_changes < r_changes {
                                "this device is behind"
                            } else if l_changes > r_changes {
                                "the desktop is behind"
                            } else {
                                "same size, different content"
                            };
                            this.desk_detail = format!(
                                "desktop {r_objects}/{r_changes} · here {l_objects}/{l_changes} (objects/changes) — {who}"
                            )
                            .into();
                        }
                    }
                    Err(error) => {
                        this.desk = Desk::Offline;
                        this.desk_detail = error.to_string().into();
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ------------------------------------------------------------- dictionary / export

    fn unignore(&mut self, word: &str, cx: &mut Context<Self>) {
        let word = word.to_lowercase();
        let mut words: Vec<String> = UiState::get(cx, IGNORE_KEY).unwrap_or_default();
        words.retain(|w| *w != word);
        UiState::set(cx, IGNORE_KEY, &words);
        self.ignored = Self::read_ignored(cx);
        cx.notify();
    }

    fn run_export(&mut self, cx: &mut Context<Self>) {
        self.exporting = true;
        self.export_note = SharedString::default();
        cx.notify();
        let backend = Store::global(cx).read(cx).backend();
        let format = self.export_format;
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<(usize, usize)>();
        let task = io::spawn(
            cx,
            export_all(backend, format, move |done, total| {
                let _ = tx.unbounded_send((done, total));
            }),
        );
        cx.spawn(async move |this, cx| {
            use futures::StreamExt as _;
            let mut task = task.fuse();
            let result = loop {
                futures::select! {
                    progress = rx.next() => if let Some((done, total)) = progress {
                        let _ = this.update(cx, |this, cx| {
                            this.export_note = format!("Preparing {done}/{total}…").into();
                            cx.notify();
                        });
                    },
                    result = task => break result,
                }
            };
            let _ = this.update(cx, |this, cx| {
                this.export_note = match result {
                    Ok(n) => format!("Exported {n} objects.").into(),
                    Err(error) => error.to_string().into(),
                };
                this.exporting = false;
                cx.notify();
            });
        })
        .detach();
    }

    // ------------------------------------------------------------- Claude plan

    fn plan_call(
        &mut self,
        path: &'static str,
        body: Option<Json>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        self.plan_error = SharedString::default();
        let task: Task<anyhow::Result<PlanStatus>> = match body {
            Some(body) => local::post(path, body, cx),
            None => local::get(path, cx),
        };
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .map_err(|e| with_status(e, |s| format!("Claude plan: {s}")));
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(plan) => {
                        if this.plan_server.is_empty() {
                            this.plan_server = if plan.served_by.is_empty() {
                                plan.this_computer.clone()
                            } else {
                                plan.served_by.clone()
                            };
                        }
                        // Signing in: follow it until the window finishes (or stops and the paste box takes over).
                        this.plan_poll = plan.signing_in.then(|| {
                            cx.spawn(async move |this, cx| {
                                cx.background_executor().timer(PLAN_POLL).await;
                                let Ok(task) = this.update(cx, |this, cx| {
                                    this.plan_call("/claude-plan", None, cx)
                                }) else {
                                    return;
                                };
                                task.await;
                            })
                        });
                        this.plan = Some(plan);
                    }
                    Err(error) => this.plan_error = error.to_string().into(),
                }
                cx.notify();
            });
        })
    }

    fn load_plan(&mut self, cx: &mut Context<Self>) {
        if !Self::paired(cx) {
            self.plan = None;
            return;
        }
        let call = self.plan_call("/claude-plan", None, cx);
        cx.spawn(async move |this, cx| {
            call.await;
            let Ok(machines) = this.update(cx, |_, cx| fetch_machines(cx)) else {
                return;
            };
            let machines = machines.await.map(|(_, m)| m).unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.plan_machines = machines;
                cx.notify();
            });
        })
        .detach();
    }

    fn plan_action(
        &mut self,
        path: &'static str,
        body: Option<Json>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        self.plan_busy = true;
        cx.notify();
        let call = self.plan_call(path, body, cx);
        cx.spawn(async move |this, cx| {
            call.await;
            let _ = this.update(cx, |this, cx| {
                this.plan_busy = false;
                cx.notify();
            });
        })
    }

    fn submit_plan_code(
        &mut self,
        state: Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let code = state.read(cx).value().trim().to_string();
        if code.is_empty() {
            return;
        }
        let action = self.plan_action("/claude-plan/code", Some(json!({ "code": code })), cx);
        cx.spawn_in(window, async move |_, cx| {
            action.await;
            let _ = cx.update(|window, cx| state.update(cx, |s, cx| s.set_value("", window, cx)));
        })
        .detach();
    }

    fn plan_computer(&self, id: &str) -> String {
        if id.is_empty() {
            "this computer".into()
        } else {
            machine_name(&self.plan_machines, id)
        }
    }

    // ------------------------------------------------------------- agent keys

    fn load_agent_auth(&mut self, cx: &mut Context<Self>) {
        self.login_error = SharedString::default();
        if !Self::paired(cx) {
            self.agent_auth = Some(None);
            return;
        }
        let task: Task<anyhow::Result<AgentAuth>> = local::get("/auth/status", cx);
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .map_err(|e| with_status(e, |s| format!("Agent credentials: {s}")));
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(auth) => this.agent_auth = Some(Some(auth)),
                    Err(error) => {
                        this.agent_auth = Some(None);
                        this.login_error = error.to_string().into();
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn save_agent_key(
        &mut self,
        provider: &'static str,
        key: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.login_error = SharedString::default();
        let key = key.trim().to_string();
        let task: Task<anyhow::Result<Json>> =
            local::post("/auth/key", json!({ "provider": provider, "key": key }), cx);
        let draft = self.kimi_draft.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = task
                .await
                .map_err(|e| with_status(e, |s| format!("Credential update failed: {s}")));
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(_) => {
                        draft.update(cx, |s, cx| s.set_value("", window, cx));
                        this.load_agent_auth(cx);
                        this.agent_saved = if key.is_empty() { "Cleared" } else { "Saved" };
                        Self::flash(cx, |this| this.agent_saved = "");
                    }
                    Err(error) => this.login_error = error.to_string().into(),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Focusable for Settings {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

// ----------------------------------------------------------------- render

fn section(title: impl Into<SharedString>) -> Div {
    section_flash(title, Empty.into_any_element())
}

/// A section whose heading carries a transient " · Saved" ([`Flash`]).
fn section_flash(title: impl Into<SharedString>, flash: AnyElement) -> Div {
    div()
        .mt(px(14.))
        .px(px(16.))
        .pt(px(14.))
        .pb(px(16.))
        .rounded(px(10.))
        .bg(theme::panel())
        .border_1()
        .border_color(theme::border())
        .child(
            div()
                .flex()
                .gap(px(4.))
                .mb(px(6.))
                .child(h3(title))
                .child(flash),
        )
}

/// A heading's flash text (`h3 · Saved`).
fn flash_suffix(text: SharedString) -> Div {
    h3(format!("· {text}"))
}

/// `.hint` with its bottom margin (10px).
fn p(text: impl Into<SharedString>) -> Div {
    hint(text).mb(px(10.))
}

/// `.hint.error`: in the web stylesheet `.hint` comes after `.error`, so
/// these messages draw muted like any hint.
fn error_p(text: impl Into<SharedString>) -> Div {
    p(text)
}

/// A bordered settings card (holding confirm, device rows): a kit outline
/// [`GroupBox`] with the surface restyled by `surface`.
fn card(id: impl Into<ElementId>, surface: StyleRefinement) -> GroupBox {
    GroupBox::new().id(id).outline().content_style(surface)
}

/// The small outline label beside a key ("SECRET", "SHAREABLE").
fn key_tag(text: &'static str, color: Hsla, border: Hsla) -> Tag {
    Tag::custom(color, color, border)
        .outline()
        .rounded_full()
        .px(px(8.))
        .py_0()
        .text_size(px(10.))
        .line_height(px(16.))
        .child(text)
}

/// The rounded outline badge of the mode line ("Full mode") and the device rows.
fn pill(text: impl Into<SharedString>, color: Hsla) -> gpui_kit::component::tag::Tag {
    gpui_kit::component::tag::Tag::custom(color, color, color)
        .outline()
        .rounded(px(999.))
        .px(px(8.))
        .py(px(2.))
        .text_size(px(12.))
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
        .child(text.into())
}

fn modal_field() -> FieldStyle {
    FieldStyle::modal()
}

impl Render for Settings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let paired = Self::paired(cx);
        let narrow = roostr_core::layout::is_mobile(window);
        let this = cx.entity();

        let header = div()
            .relative()
            .flex()
            .justify_between()
            .items_center()
            .mb(px(8.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(17.))
                    .line_height(px(21.))
                    .font_weight(FontWeight::BOLD)
                    .child(div().text_size(px(15.)).child("⚙️"))
                    .child("Settings"),
            )
            .when(!narrow, |d| {
                d.child(Btn::x("close-settings").on_click(|_, window, cx| window.close_dialog(cx)))
            })
            .when(narrow, |d| d.child(self.sheet_grip(cx)));

        // ── Profile
        let avatar = Avatar::new()
            .with_size(px(56.))
            .flex_none()
            .map(
                |a| match (&self.profile_image, self.profile_picture.is_empty()) {
                    (Some(image), _) => a.src(image.clone()),
                    (None, false) => a.src(SharedString::from(self.profile_picture.clone())),
                    (None, true) => a.placeholder(Glyph::Settings),
                },
            )
            .into_any_element();
        let has_picture = !self.profile_picture.is_empty();
        let profile = section("Profile")
            .child(p(
                "Your name and avatar (your Nostr profile), synced to every device holding this key. The avatar also shows on the Spaces screen.",
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(avatar)
                    .child(field(&self.profile_name, modal_field(), window, cx).flex_1().min_w(px(120.)).max_w(px(220.)))
                    .child(
                        Btn::action("pick-avatar", if has_picture { "Change picture" } else { "Upload picture" })
                            .disabled(self.avatar_busy)
                            .on_click(cx.listener(|this, _, window, cx| this.pick_avatar(window, cx))),
                    )
                    .when(has_picture, |d| {
                        d.child(
                            Btn::small("remove-avatar", "Remove")
                                .disabled(self.avatar_busy)
                                .on_click(cx.listener(|this, _, _, cx| this.save_profile(nostr_profile::patch("picture", ""), false, cx))),
                        )
                    })
                    .when(!self.avatar_state.is_empty(), |d| d.child(hint(self.avatar_state.clone()).min_w_0())),
            );

        // ── Chat colour
        let draft = self.chat_color_draft.read(cx).value().to_string();
        let not_default = normalize_hex(&draft).as_deref() != Some(DEFAULT_CHAT_COLOR);
        let chat = section("Chat color")
            .child(p(
                "Your message bubbles and send button, on every device holding this key. Text on it switches to dark or white to stay readable.",
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex_none()
                            .w(px(40.))
                            .h(px(32.))
                            .p(px(2.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .bg(theme::panel())
                            .border_1()
                            .border_color(theme::border())
                            .child(ColorPicker::new(&self.swatch).accessibility_label("Chat color picker")),
                    )
                    .child(field(&self.chat_color_draft, modal_field(), window, cx).flex_1().max_w(px(220.)))
                    .when(not_default, |d| {
                        d.child(Btn::small("reset-color", "Reset").on_click(cx.listener(|this, _, window, cx| {
                            this.save_chat_color(DEFAULT_CHAT_COLOR.to_string(), window, cx)
                        })))
                    })
                    .when(!self.chat_color_state.is_empty(), |d| d.child(hint(self.chat_color_state.clone()).min_w_0())),
            );

        // ── Nostr identity
        let mut identity = section("Nostr identity").child(p(
            "This key encrypts and signs everything you sync. Anyone holding it has your full identity — export it only to back it up or to log in on another device.",
        ));
        if !self.npub.is_empty() {
            let npub = self.npub.clone();
            identity = identity
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .mt(px(10.))
                        .mb(px(4.))
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .child("Public key")
                        .child(
                            div()
                                .text_size(px(11.))
                                .font_weight(FontWeight::NORMAL)
                                .font_family(theme::fonts::MONO)
                                .text_color(theme::muted())
                                .child("npub"),
                        )
                        .child(key_tag(
                            "SHAREABLE",
                            rgb(0x35b57f).into(),
                            rgba(0x35b57f73).into(),
                        )),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .mb(px(6.))
                        .child(
                            div()
                                .id("npub-code")
                                .flex_1()
                                .min_w_0()
                                .overflow_x_scroll()
                                .px(px(10.))
                                .py(px(7.))
                                .rounded(px(8.))
                                .bg(theme::bg())
                                .border_1()
                                .border_color(theme::border())
                                .font_family(theme::fonts::MONO)
                                .text_size(px(11.))
                                .whitespace_nowrap()
                                .child(npub.clone()),
                        )
                        .child(
                            Btn::small(
                                "copy-npub",
                                if self.copied == "npub" {
                                    "Copied"
                                } else {
                                    "Copy"
                                },
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| this.copy(npub.clone(), "npub", cx),
                            )),
                        ),
                );
        }
        let replica = roostr_core::mode::current(cx) == roostr_core::mode::Mode::Replica;
        if replica {
            identity = self.render_reveal(identity, cx);
        } else {
            identity = identity.child(
                div().text_size(px(12.)).line_height(px(18.)).mb(px(10.)).child(rich(
                    &[
                        Span::Plain("Back up the native private key only from the operator's terminal on this computer: "),
                        Span::Code("./glon-odin key-export"),
                        Span::Plain(
                            " (run from the native installation directory). Keep the output secret; this app cannot request or reveal it.",
                        ),
                    ],
                    theme::muted(),
                    window,
                )),
            );
        }
        if !self.importing {
            identity = identity
                .child(
                    div().mt(px(6.)).flex().child(
                        Btn::action("import-key", "Sign in with existing key…").subtle().on_click(cx.listener(|this, _, window, cx| {
                            this.importing = true;
                            this.import_draft.focus_handle(cx).focus(window, cx);
                            cx.notify();
                        })),
                    ),
                )
                .when(!replica, |d| {
                    d.child(p(
                        "Replaces this machine's daemon identity for every paired tab; the previous identity's shared-space authority is revoked.",
                    ))
                })
                .child(
                    div().mt(px(6.)).flex().child(
                        Btn::action("logout", match (self.confirm_logout, replica, io::WEB) {
                            (true, true, _) => "Erase local copy & sign out?",
                            (false, true, _) => "Log out on this device",
                            (true, false, false) => "Disconnect this app?",
                            (false, false, false) => "Disconnect this app",
                            (true, false, true) => "Disconnect this browser?",
                            (false, false, true) => "Unpair this browser",
                        })
                            .subtle()
                            .when(self.confirm_logout, Btn::armed)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.confirm_logout {
                                    this.confirm_logout = true;
                                    cx.notify();
                                    return;
                                }
                                if replica {
                                    this.log_out(cx);
                                    return;
                                }
                                // Disconnecting the only client of a native app is quitting it;
                                // a browser tab drops its pairing and returns to the pairing gate.
                                #[cfg(target_family = "wasm")]
                                roostr_core::web::unpair();
                                #[cfg(not(target_family = "wasm"))]
                                cx.quit();
                            })),
                    ),
                )
                .when(self.confirm_logout, |d| {
                    d.child(p(match (replica, io::WEB) {
                        (true, _) => "Removes this device's key and its local replica only after pending changes are published. Your encrypted history stays on your relays — log in with any nsec afterwards.",
                        (false, true) => "Disconnects this browser only. The native key, local data, and running daemon are not erased.",
                        (false, false) => "Disconnects this app only. The native key, local data, and running daemon are not erased.",
                    }))
                });
        } else {
            identity = identity
                .child(div().text_size(px(12.)).line_height(px(18.)).mb(px(10.)).child(rich(
                    &[
                        Span::Bold("Replaces this device's identity."),
                        Span::Plain(
                            " Paste the nsec from your other device — after the sync daemon connects, your objects will backfill from the relays.",
                        ),
                    ],
                    theme::muted(),
                    window,
                )))
                .child(
                    div()
                        .relative()
                        .flex()
                        .gap(px(8.))
                        .mt(px(6.))
                        .child(keep_in_view(
                            self.import_draft.focus_handle(cx),
                            self.scroll.clone(),
                            self.import_in_view.clone(),
                        ))
                        .child(field(&self.import_draft, modal_field(), window, cx).flex_1())
                        .child(Btn::small("import-submit", "Import").on_click(cx.listener(|this, _, _, cx| {
                            if !this.import_draft.read(cx).value().trim().is_empty() {
                                this.import_key(cx);
                            }
                        })))
                        .child(Btn::small("import-cancel", "Cancel").on_click(cx.listener(|this, _, window, cx| this.cancel_import(window, cx)))),
                )
                .when(!self.import_error.is_empty(), |d| d.child(error_p(self.import_error.clone()).mt(px(10.))));
        }

        // ── Relays
        let save_flash = self
            .save_flash
            .render(self.save_state, flash_suffix, window, cx);
        let mut relays = section_flash("Relays", save_flash).child(p(
            "Where your encrypted changes sync. Your data stays local until relays are configured.",
        ));
        for (ix, (url, label, note)) in KNOWN_RELAYS.iter().enumerate() {
            let on = self.relays.iter().any(|r| r == url);
            let row_this = this.clone();
            relays = relays.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .mb(px(6.))
                    .px(px(10.))
                    .py(px(6.))
                    .rounded(px(8.))
                    .bg(theme::bg())
                    .border_1()
                    .border_color(theme::border())
                    .font_family(theme::fonts::MONO)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(1.))
                            .child(div().text_size(px(13.)).line_height(px(16.)).child(*label))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .line_height(px(14.))
                                    .text_color(theme::muted())
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(format!(
                                        "{} · {note}",
                                        url.trim_start_matches("wss://")
                                    )),
                            ),
                    )
                    .child(switch(("relay-switch", ix), on, move |on, _, cx| {
                        row_this.update(cx, |this, cx| this.toggle_relay(url, on, cx))
                    })),
            );
        }
        for r in self
            .relays
            .iter()
            .filter(|r| !KNOWN_RELAYS.iter().any(|(u, _, _)| u == *r))
        {
            let url = r.clone();
            relays = relays.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .mb(px(6.))
                    .px(px(10.))
                    .py(px(6.))
                    .rounded(px(8.))
                    .bg(theme::bg())
                    .border_1()
                    .border_color(theme::border())
                    .font_family(theme::fonts::MONO)
                    .text_size(px(13.))
                    .child(r.clone())
                    .child(
                        Btn::x(SharedString::from(format!("remove-relay-{r}"))).on_click(
                            cx.listener(move |this, _, _, cx| {
                                let next =
                                    this.relays.iter().filter(|x| **x != url).cloned().collect();
                                this.save_relays(next, cx);
                            }),
                        ),
                    ),
            );
        }
        if self.relays.is_empty() {
            relays =
                relays.child(hint("No relays enabled - nothing syncs until one is on.").my(px(4.)));
        }
        relays = relays.child(
            div()
                .flex()
                .gap(px(8.))
                .mt(px(8.))
                .child(field(&self.new_relay, modal_field(), window, cx).flex_1())
                .child(
                    Btn::small("add-relay", "Add")
                        .on_click(cx.listener(|this, _, window, cx| this.add_relay(window, cx))),
                ),
        );

        // ── Where the vault lives
        let vault = section("Where your vault lives")
            .child(p(if replica {
                if io::WEB {
                    "In this browser: its own replica syncs end-to-end encrypted over your relays, no daemon needed. Roostr running on this computer can be paired instead."
                } else {
                    "On this device: its own replica syncs end-to-end encrypted over your relays, no daemon needed. The glon-odin daemon on this computer can serve the app instead."
                }
            } else if io::WEB {
                "Paired with Roostr running on this computer. This browser can keep its own replica instead, synced over your relays without a daemon."
            } else {
                "Served by the glon-odin daemon on this computer. This app can keep its own replica instead, synced over your relays without a daemon."
            }))
            .child(
                div().mt(px(6.)).flex().child(
                    Btn::action(
                        "switch-mode",
                        match (replica, io::WEB) {
                            (true, true) => "Pair with Roostr on this computer",
                            (true, false) => "Use the daemon on this computer",
                            (false, true) => "Use this browser's own replica",
                            (false, false) => "Use this device's own replica",
                        },
                    )
                    .subtle()
                    .on_click(move |_, _, cx| {
                        roostr_core::mode::switch(
                            if replica {
                                roostr_core::mode::Mode::Daemon
                            } else {
                                roostr_core::mode::Mode::Replica
                            },
                            cx,
                        )
                    }),
                ),
            );
        let vault = self.render_holding(vault, window, cx);

        // ── Desktop sync check
        let checking = self.desk == Desk::Checking;
        let desk_color = match self.desk {
            Desk::Match => theme::green(),
            Desk::Differ => theme::orange(),
            _ => theme::muted(),
        };
        let desk = section("Desktop sync check")
            .child(p(
                "Compare the active vault fingerprint with the explicitly paired local daemon. Equal digests mean identical change sets; in native mode both refer to that daemon.",
            ))
            .child(
                div().flex().mt(px(8.)).child(
                    Btn::small("compare", if checking { "Checking…" } else { "Compare" })
                        .when(checking, |b| b.leading(spinner(theme::fg())))
                        .disabled(!paired || checking)
                        .on_click(cx.listener(|this, _, _, cx| this.check_desktop(cx))),
                ),
            )
            .when(!paired, |d| {
                d.child(p("Pair with a desktop under This machine first - until then there is nothing to compare with.").mt(px(8.)))
            })
            .when(paired && self.desk != Desk::Idle, |d| d.child(p(self.desk_detail.clone()).mt(px(8.)).text_color(desk_color)));

        // ── Dictionary
        let mut dictionary =
            section("Dictionary").child(p("Words you added to the dictionary from the editor's spellcheck. Remove one to flag it again."));
        if self.ignored.is_empty() {
            dictionary =
                dictionary.child(hint("Nothing added yet — right-click a flagged word and choose \"Add to dictionary\".").my(px(4.)));
        } else {
            dictionary = dictionary.child(div().flex().flex_wrap().gap(px(6.)).children(
                self.ignored.iter().map(|w| {
                    let word = w.clone();
                    Tag::custom(theme::bg(), theme::fg(), theme::border())
                        .rounded_full()
                        .gap(px(2.))
                        .pl(px(12.))
                        .pr(px(4.))
                        .py(px(2.))
                        .font_family(theme::fonts::MONO)
                        .text_size(px(13.))
                        .child(w.clone())
                        .child(
                            Btn::x(SharedString::from(format!("unignore-{w}")))
                                .tooltip("Remove")
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.unignore(&word, cx)),
                                ),
                        )
                }),
            ));
        }

        // ── Export
        let formats = vec![
            (
                "markdown".to_string(),
                SharedString::from(ExportFormat::Markdown.label()),
            ),
            (
                "json".to_string(),
                SharedString::from(ExportFormat::Json.label()),
            ),
        ];
        let selected_format = if self.export_format == ExportFormat::Markdown {
            "markdown"
        } else {
            "json"
        };
        let export = section("Export")
            .child(p(
                "Download everything as a zip — one file per object, grouped by type. Markdown for reading and portability; JSON for a complete structural backup (fields, blocks, provenance).",
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        Select::new("export-format", formats, selected_format)
                            .on_pick({
                                let this = this.clone();
                                move |value, _, cx| {
                                    this.update(cx, |this, cx| {
                                        this.export_format = if value == "json" { ExportFormat::Json } else { ExportFormat::Markdown };
                                        cx.notify();
                                    })
                                }
                            }),
                    )
                    .child(
                        Btn::action("export", if self.exporting { "Exporting…" } else { "Export everything" })
                            .disabled(self.exporting)
                            .on_click(cx.listener(|this, _, _, cx| this.run_export(cx))),
                    )
                    .when(!self.export_note.is_empty(), |d| d.child(hint(self.export_note.clone()))),
            );

        // ── Claude plan
        let mut plan_section = section("Claude plan");
        if !paired {
            plan_section = plan_section.child(p("Pair with a computer under This machine first: the Claude sign-in opens in Chrome there."));
        } else if let Some(plan) = self.plan.clone() {
            plan_section = plan_section.child(p(
                "Your Claude plan, on the Anthropic credential of every space you own. Agents use the plan while it works and the credential's API key otherwise.",
            ));
            let mut row = div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(10.))
                .my(px(6.));
            if !plan.account.is_empty() {
                row = row.child(div().text_size(px(13.)).child(rich(
                    &[Span::Plain("Signed in as "), Span::Bold(&plan.account)],
                    theme::fg(),
                    window,
                )));
            }
            if plan.signing_in {
                row = row.child(
                    div().flex().items_start().text_size(px(12.)).line_height(px(18.)).text_color(theme::muted()).child(spinner(theme::muted())).child(
                        format!(
                            "Sign in to Claude in the browser that opened on {} and press Authorize. If claude.ai shows a code instead, paste it below.",
                            self.plan_computer(&plan.this_computer)
                        ),
                    ),
                );
            } else {
                row = row.child(
                    Btn::action(
                        "plan-sign-in",
                        if plan.account.is_empty() {
                            "Sign in with Claude"
                        } else {
                            "Sign in again"
                        },
                    )
                    .disabled(self.plan_busy)
                    .on_click(cx.listener(|this, _, _, cx| {
                        let server = this.plan_server.clone();
                        this.plan_action(
                            "/claude-plan/start",
                            Some(json!({ "servedBy": server })),
                            cx,
                        )
                        .detach();
                    })),
                );
                if !plan.account.is_empty() {
                    row = row.child(
                        Btn::small("plan-sign-out", "Sign out")
                            .subtle()
                            .disabled(self.plan_busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.plan_action("/claude-plan/signout", None, cx).detach()
                            })),
                    );
                }
            }
            plan_section = plan_section.child(row);
            let machines: Vec<(String, SharedString)> = self
                .plan_machines
                .iter()
                .map(|m| {
                    let this_one = if m.machine_id == plan.this_computer {
                        " (this computer)"
                    } else {
                        ""
                    };
                    (m.machine_id.clone(), format!("{}{this_one}", m.name).into())
                })
                .collect();
            let has_account = !plan.account.is_empty();
            plan_section = plan_section.child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .my(px(6.))
                    .text_size(px(13.))
                    .child("Kept fresh by")
                    .child(
                        Select::new("plan-server", machines, self.plan_server.clone()).on_pick({
                            let this = this.clone();
                            move |machine, _, cx| {
                                let machine = machine.to_string();
                                this.update(cx, |this, cx| {
                                    this.plan_server = machine.clone();
                                    if has_account {
                                        this.plan_action(
                                            "/claude-plan/serve",
                                            Some(json!({ "machine": machine })),
                                            cx,
                                        )
                                        .detach();
                                    }
                                    cx.notify();
                                })
                            }
                        }),
                    )
                    .child(hint(
                        "One always-on computer renews the plan for every space.",
                    )),
            );
            if !plan.sign_in_error.is_empty() {
                plan_section = plan_section.child(error_p(plan.sign_in_error.clone()));
            }
            if plan.signing_in || !plan.sign_in_error.is_empty() {
                // claude.ai shows a code when it can't hand it back by itself: it goes here.
                let code = self.plan_code.clone();
                plan_section = plan_section.child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .mt(px(8.))
                        .child(field(&self.plan_code, modal_field(), window, cx).flex_1())
                        .child(
                            Btn::small("plan-finish", "Finish")
                                .subtle()
                                .disabled(self.plan_busy)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.submit_plan_code(code.clone(), window, cx)
                                })),
                        ),
                );
            }
            if has_account {
                plan_section = plan_section.child(
                    div()
                        .mt(px(8.))
                        .flex()
                        .flex_col()
                        .text_size(px(13.))
                        .children(plan.spaces.iter().map(|sp| {
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
                            div()
                                .id(SharedString::from(format!("plan-space-{}", sp.id)))
                                .flex()
                                .justify_between()
                                .gap(px(12.))
                                .py(px(3.))
                                .border_b_1()
                                .border_color(theme::border())
                                .child(sp.name.clone())
                                // `.hint.error` draws muted (see `error_p`).
                                .child(hint(state))
                        })),
                );
            }
            if !self.plan_error.is_empty() {
                plan_section = plan_section.child(error_p(self.plan_error.clone()));
            }
        } else {
            plan_section = plan_section.child(p(if self.plan_error.is_empty() {
                SharedString::from("Checking…")
            } else {
                self.plan_error.clone()
            }));
        }

        // ── Agent
        let agent_flash = self
            .agent_flash
            .render(self.agent_saved, flash_suffix, window, cx);
        let mut agent = section_flash("Agent", agent_flash);
        match &self.agent_auth {
            None => agent = agent.child(p("Checking agent daemon…")),
            Some(None) => {
                agent = agent.child(p(if paired {
                    "The paired agent daemon is unavailable. Start the local harness to manage credentials."
                } else {
                    "Pair under This machine to manage agent credentials. Browser data and relay chat remain available without pairing."
                }));
                if paired {
                    agent = agent.child(
                        div().mt(px(6.)).flex().child(
                            Btn::action("retry-agent", "Retry agent connection")
                                .subtle()
                                .on_click(cx.listener(|this, _, _, cx| this.load_agent_auth(cx))),
                        ),
                    );
                }
            }
            Some(Some(auth)) => {
                agent = agent.child(div().text_size(px(12.)).line_height(px(18.)).mb(px(10.)).child(rich(
                    &[
                        Span::Plain("Keys for this computer only ("),
                        Span::Code("~/.glon/auth.json"),
                        Span::Plain("), for agents that list no credential of their own. Claude for your spaces is under Claude plan above."),
                    ],
                    theme::muted(),
                    window,
                )));
                let mut provider = div().flex().items_center().gap(px(10.)).py(px(6.)).child(
                    div()
                        .flex_none()
                        .w(px(130.))
                        .text_size(px(13.))
                        .child("Kimi (Moonshot)"),
                );
                if auth.kimi.mode == "api_key" {
                    provider = provider
                        .child(
                            div()
                                .font_family(theme::fonts::MONO)
                                .text_size(px(12.))
                                .text_color(theme::muted())
                                .child(format!(
                                    "API key set ({})",
                                    auth.kimi.masked.clone().unwrap_or_default()
                                )),
                        )
                        .child(Btn::small("clear-kimi", "Clear").on_click(cx.listener(
                            |this, _, window, cx| {
                                this.save_agent_key("kimi", String::new(), window, cx)
                            },
                        )));
                } else {
                    provider = provider.child(
                        div()
                            .flex_1()
                            .flex()
                            .gap(px(8.))
                            .child(field(&self.kimi_draft, modal_field(), window, cx).flex_1())
                            .child(Btn::small("save-kimi", "Save").on_click(cx.listener(
                                |this, _, window, cx| {
                                    let key = this.kimi_draft.read(cx).value().to_string();
                                    if !key.trim().is_empty() {
                                        this.save_agent_key("kimi", key, window, cx);
                                    }
                                },
                            ))),
                    );
                }
                agent = agent.child(provider);
            }
        }
        if !self.login_error.is_empty() {
            agent = agent.child(error_p(self.login_error.clone()));
        }

        let content = vec![
            header.into_any_element(),
            div()
                .when(!self.settings_error.is_empty(), |d| {
                    d.child(error_p(self.settings_error.clone()))
                })
                .into_any_element(),
            profile.into_any_element(),
            chat.into_any_element(),
            identity.into_any_element(),
            relays.into_any_element(),
            // The iOS app is replica-only: there is no daemon to switch to.
            div()
                .when(!cfg!(target_os = "ios"), |d| d.child(vault))
                .into_any_element(),
            desk.into_any_element(),
            dictionary.into_any_element(),
            export.into_any_element(),
            plan_section.into_any_element(),
            agent.into_any_element(),
            div()
                .mt(px(14.))
                .mx(px(2.))
                .text_size(px(11.))
                .text_color(theme::muted())
                .opacity(0.7)
                .text_center()
                .child(format!("Build {BUILD_STAMP}"))
                .into_any_element(),
        ];

        if !narrow {
            return div()
                .id("settings")
                .track_focus(&self.focus)
                .key_context("Settings")
                .w_full()
                .max_h(roostr_core::layout::viewport(window).height * 0.8)
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .pt(px(18.))
                .px(px(22.))
                .pb(px(22.))
                .text_color(theme::fg())
                .text_size(px(13.))
                .children(content)
                .into_any_element();
        }

        // ≤720px: a bottom sheet - pinned to the bottom edge, full width,
        // rounded top, a grabber bar and drag-to-close; the × hides (the
        // sheet IS the affordance). `max-height: calc(100dvh - max(48px,
        // env(safe-area-inset-top) + 32px))`, the inset being 0 inside the
        // host's safe area.
        let viewport = roostr_core::layout::viewport(window);
        // Rises from below the window on open; tracks the finger 1:1.
        let transition = if self.sheet_drag.is_some() {
            motion::Transition::Instant
        } else {
            self.sheet_settle.transition()
        };
        let sheet_state = self
            .sheet
            .get_or_insert_with(|| motion::ValueState::new(f64::from(viewport.height)));
        let sheet_top = px(sheet_state
            .frame(f64::from(self.sheet_y), transition, window, cx)
            .value() as f32);
        let sheet = div()
            .id("settings")
            .track_focus(&self.focus)
            .key_context("Settings")
            .relative()
            .top(sheet_top)
            .w_full()
            .max_h(viewport.height - px(48.))
            .flex()
            .flex_col()
            .bg(theme::panel())
            .border_t_1()
            .border_l_1()
            .border_r_1()
            .border_color(theme::border())
            .rounded_t(px(18.))
            .shadow(roostr_core::ui::modal_shadow())
            .occlude()
            .text_color(theme::fg())
            .text_size(px(13.))
            // `.grab-zone`: sticky - the grabber never scrolls away with the
            // content, it IS the close affordance.
            .child(
                div()
                    .relative()
                    .flex_none()
                    .flex()
                    .justify_center()
                    .pt(px(10.))
                    .pb(px(6.))
                    .child(
                        div()
                            .w(px(40.))
                            .h(px(4.))
                            .rounded(px(2.))
                            .bg(theme::border()),
                    )
                    .child(self.sheet_grip(cx)),
            )
            .child(
                div()
                    .id("settings-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .px(px(22.))
                    .pb(px(22.))
                    .children(content),
            );
        let sheet = sheet.into_any_element();
        // The dialog lends its backdrop, focus and Escape; the sheet places
        // itself against the bottom edge of the area the app is laid out in
        // (inside the iOS safe area, above the keyboard). It escapes the
        // dialog's empty surface as a deferred layer at the dialog layer's
        // own priority (gpui-base: 10 + its layer, 0 for Settings): nested
        // in that layer, it paints after it - over the backdrop's dim - and
        // still under popups (100) and any dialog opened above it.
        let visible = window.fully_visible_bounds();
        div()
            .child(
                deferred(
                    anchored().position(visible.origin).child(
                        div()
                            .w(viewport.width)
                            .h(viewport.height)
                            .flex()
                            .flex_col()
                            .justify_end()
                            .child(sheet),
                    ),
                )
                .with_priority(10),
            )
            .into_any_element()
    }
}

impl Settings {
    /// The grab zone and header's touch drag (`sheetStart`/`sheetMove`/
    /// `sheetEnd`): pull the sheet down past 110px to close it, else it
    /// springs back. Content scrolling is untouched - the drag starts only
    /// here.
    fn sheet_grip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        canvas(
            |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
            move |_, hitbox, window, _| {
                let this = this.clone();
                window.on_mouse_event(move |ev: &TouchDragEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    let Some(this) = this.upgrade() else {
                        return;
                    };
                    match ev.phase {
                        TouchPhase::Started => {
                            if hitbox.is_hovered(window) {
                                window.prevent_default();
                                this.update(cx, |s, cx| {
                                    s.sheet_drag = Some(ev.start_position.y);
                                    cx.notify();
                                });
                            }
                        }
                        TouchPhase::Moved => this.update(cx, |s, cx| {
                            if let Some(start) = s.sheet_drag {
                                s.sheet_y = (ev.position.y - start).max(px(0.));
                                cx.notify();
                            }
                        }),
                        TouchPhase::Ended | TouchPhase::Cancelled => {
                            let close = this.update(cx, |s, cx| {
                                if s.sheet_drag.take().is_none() {
                                    return false;
                                }
                                if s.sheet_y > px(110.) {
                                    return true;
                                }
                                // Settles back from a pull short of the threshold (`transform 0.18s ease`).
                                s.sheet_settle = motion::SHEET_SNAP;
                                s.sheet_y = px(0.);
                                cx.notify();
                                false
                            });
                            if close {
                                window.close_dialog(cx);
                            }
                        }
                    }
                });
            },
        )
        .absolute()
        .inset_0()
    }
}

/// Keeps its parent (a text field's row) inside the scroll area `scroll`
/// while `focus` has focus, as iOS WebKit kept a focused input in view: once
/// on focus, and again each time the area changes height - the on-screen
/// keyboard rising shortens the phone sheet, and would otherwise cover a
/// field low in it. Scrolls no further than needed; scrolling away while
/// typing is left alone. `in_view` holds the height last handled.
fn keep_in_view(
    focus: FocusHandle,
    scroll: ScrollHandle,
    in_view: Rc<Cell<Option<Pixels>>>,
) -> impl IntoElement {
    const MARGIN: Pixels = px(12.);
    canvas(
        move |bounds, window, _| {
            if !focus.is_focused(window) {
                in_view.set(None);
                return;
            }
            let area = scroll.bounds();
            if in_view.replace(Some(area.size.height)) == Some(area.size.height) {
                return;
            }
            let below = bounds.bottom() + MARGIN - area.bottom();
            let above = area.top() - (bounds.top() - MARGIN);
            let mut offset = scroll.offset();
            if below > px(0.) {
                offset.y -= below;
            } else if above > px(0.) {
                offset.y += above;
            } else {
                return;
            }
            offset.y = offset.y.clamp(-scroll.max_offset().y, px(0.));
            scroll.set_offset(offset);
            window.request_animation_frame();
        },
        |_, _, _, _| {},
    )
    .absolute()
    .inset_0()
}

/// Open Settings as a modal over the window (580px); at ≤720px, a bottom
/// sheet (see the narrow branch of its render).
pub fn open_settings(window: &mut Window, cx: &mut App) {
    let settings = cx.new(|cx| Settings::new(window, cx));
    window.open_dialog(cx, move |dialog, window, _| {
        let dialog = dialog
            .close_button(false)
            // Enter in a field submits that field's form (web <form>), never the whole modal.
            .on_ok(|_, _, _| false)
            .p_0()
            .gap_0()
            .child(settings.clone());
        if roostr_core::layout::is_mobile(window) {
            // The sheet draws itself; the dialog's own surface stays empty.
            dialog
                .w(px(0.))
                .min_h(px(0.))
                .border_0()
                .bg(transparent_black())
        } else {
            dialog
                .w(px(580.))
                .bg(theme::panel())
                .border_color(theme::border())
                .rounded(theme::radius::modal())
                .shadow(roostr_core::ui::modal_shadow())
        }
    });
}
