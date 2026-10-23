//! A File object's page (`File.svelte`). The object carries the facts
//! (name, hash, size, MIME, which computers hold the bytes) and, for files
//! on Blossom, the blob and its key: those bytes are fetched and decrypted
//! right here. Older files without a blob come from the harness on this
//! computer, which fetches from a peer when it lacks them.

use std::sync::Arc;

use gpui_kit::component::attachment::{
    Attachment, AttachmentContent, AttachmentMedia, AttachmentStatus, AttachmentTitle,
};
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_kit::component::description_list::DescriptionList;
use gpui_kit::component::link::Link;
use gpui_kit::component::{Icon, IconName};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::serving::{self, MachineRow};
use roostr_core::types::{Object, field_str};
use roostr_core::{Route, Store, StoreEvent, theme};

/// Text previews stop here; the download is always the whole file.
const TEXT_PREVIEW_BYTES: usize = 200 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Image,
    Video,
    Audio,
    Pdf,
    Text,
    None,
}

fn preview_kind(t: &str) -> Kind {
    let t = t.to_lowercase();
    let t = t.split(';').next().unwrap_or("").trim().to_string();
    if t.starts_with("image/") {
        Kind::Image
    } else if t.starts_with("video/") {
        Kind::Video
    } else if t.starts_with("audio/") {
        Kind::Audio
    } else if t == "application/pdf" {
        Kind::Pdf
    } else if t.starts_with("text/") || t == "application/json" || t.ends_with("+json") {
        Kind::Text
    } else {
        Kind::None
    }
}

/// A File object's page (File.svelte).
pub struct FileView {
    object_id: String,
    object: Option<Object>,
    machines: Vec<MachineRow>,
    this_machine: String,
    bytes: Option<Arc<Vec<u8>>>,
    image: Option<Arc<Image>>,
    blob_type: String,
    text: Option<String>,
    loading: bool,
    fetch_error: String,
    /// Bumped on every (re)load: a slower earlier fetch never lands.
    generation: u64,
    loaded_key: String,
    _subs: Vec<Subscription>,
}

impl FileView {
    /// The page for File object `object_id`.
    pub fn new(object_id: String, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = Store::global(cx);
        let subs = vec![
            cx.subscribe(&store, |this: &mut Self, _, ev: &StoreEvent, cx| match ev {
                StoreEvent::Committed(id) if *id == this.object_id => this.reload_object(cx),
                StoreEvent::Reconnected => this.reload_object(cx),
                _ => {}
            }),
        ];
        let mut this = Self {
            object_id,
            object: None,
            machines: Vec::new(),
            this_machine: String::new(),
            bytes: None,
            image: None,
            blob_type: String::new(),
            text: None,
            loading: false,
            fetch_error: String::new(),
            generation: 0,
            loaded_key: String::new(),
            _subs: subs,
        };
        this.reload_object(cx);
        let machines = serving::fetch_machines(cx);
        let me = roostr_core::create::this_machine_id(cx);
        cx.spawn(async move |this, cx| {
            let roster = machines.await.map(|(_, m)| m).unwrap_or_default();
            let me = me.await;
            let _ = this.update(cx, |this, cx| {
                this.machines = roster;
                this.this_machine = me;
                cx.notify();
            });
        })
        .detach();
        this
    }

    fn reload_object(&mut self, cx: &mut Context<Self>) {
        let fetch = Store::global(cx).read(cx).fetch_object(&self.object_id, cx);
        cx.spawn(async move |this, cx| {
            let Ok(object) = fetch.await else { return };
            let _ = this.update(cx, |this, cx| {
                this.object = Some(object);
                this.load(false, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn fields(&self) -> roostr_core::types::Fields {
        self.object
            .as_ref()
            .map(|o| o.fields.clone())
            .unwrap_or_default()
    }

    fn on_blossom(&self) -> bool {
        !field_str(&self.fields(), "blob_sha256").is_empty()
    }

    fn hash(&self) -> String {
        field_str(&self.fields(), "file_hash").to_lowercase()
    }

    /// Fetch the bytes for the current hash (again when `force`).
    fn load(&mut self, force: bool, cx: &mut Context<Self>) {
        let hash = self.hash();
        let key = format!("{hash}|{}", self.on_blossom());
        if !force && key == self.loaded_key {
            return;
        }
        self.loaded_key = key;
        self.generation += 1;
        let mine = self.generation;
        self.bytes = None;
        self.image = None;
        self.blob_type.clear();
        self.text = None;
        self.fetch_error.clear();
        let harness = roostr_core::harness::harness(cx);
        if (harness.is_none() && !self.on_blossom()) || hash.is_empty() {
            self.loading = false;
            return;
        }
        self.loading = true;
        let fields = self.fields();
        let relays = crate::files::relays(cx);
        let mime = field_str(&fields, "file_mime").to_string();
        let task = roostr_core::io::spawn(cx, async move {
            let (bytes, fetched) =
                crate::files::fetch_file_blob(&fields, &relays, harness.as_deref()).await?;
            // The object's MIME wins over whatever the transfer labelled it.
            let kind = if mime.is_empty() { fetched } else { mime };
            let text = (preview_kind(&kind) == Kind::Text).then(|| {
                String::from_utf8_lossy(&bytes[..bytes.len().min(TEXT_PREVIEW_BYTES)]).into_owned()
            });
            anyhow::Ok((bytes, kind, text))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if mine != this.generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok((bytes, kind, text)) => {
                        if preview_kind(&kind) == Kind::Image
                            && let Some(format) = ImageFormat::from_mime_type(&kind)
                        {
                            this.image = Some(Arc::new(Image::from_bytes(format, bytes.clone())));
                        }
                        this.bytes = Some(Arc::new(bytes));
                        this.blob_type = kind;
                        this.text = text;
                    }
                    Err(err) => this.fetch_error = crate::files::view_failure_text(&err),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Pick up what the harness wrote since (error, holders), then try the bytes again.
    fn retry(&mut self, cx: &mut Context<Self>) {
        let fetch = Store::global(cx).read(cx).fetch_object(&self.object_id, cx);
        cx.spawn(async move |this, cx| {
            let fresh = fetch.await.ok();
            let _ = this.update(cx, |this, cx| {
                if let Some(o) = fresh {
                    this.object = Some(o);
                }
                this.load(true, cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// The page's quiet action button. The custom variant's color is the
    /// hover/press border and its foreground the hover/press text, so the
    /// rest state (bg border, muted text) comes from the instance style.
    fn button(id: &'static str, label: &'static str, cx: &App) -> Button {
        Button::new(id)
            .custom(
                ButtonCustomVariant::new(cx)
                    .color(theme::accent())
                    .foreground(theme::fg())
                    .hover(theme::bg())
                    .active(theme::bg()),
            )
            .accessibility_label(label)
            .child(div().text_size(px(12.)).child(label))
            .rounded(px(7.))
            .h_auto()
            .px(px(12.))
            .py(px(5.))
            .bg(theme::bg())
            .border_1()
            .border_color(theme::border())
            .text_color(theme::muted())
            .cursor_pointer()
    }
}

impl Render for FileView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let fields = self.fields();
        let name = field_str(&fields, "name").to_string();
        let hash = self.hash();
        let on_blossom = self.on_blossom();
        let size = fields
            .get("file_size")
            .and_then(|v| v.as_number())
            .unwrap_or(0.);
        let mime = field_str(&fields, "file_mime").to_string();
        let object_error = field_str(&fields, "error").to_string();
        let available_on: Vec<String> = fields
            .get("available_on")
            .map(|v| {
                v.items()
                    .iter()
                    .filter_map(|i| i.as_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let holders: Vec<(String, Option<MachineRow>, String)> = available_on
            .iter()
            .map(|m| {
                (
                    m.clone(),
                    self.machines.iter().find(|r| r.machine_id == *m).cloned(),
                    serving::machine_name(&self.machines, m),
                )
            })
            .collect();
        let fetching_from = if !self.this_machine.is_empty()
            && !available_on.contains(&self.this_machine)
            && !holders.is_empty()
        {
            holders
                .iter()
                .map(|h| h.2.clone())
                .collect::<Vec<_>>()
                .join(" or ")
        } else {
            String::new()
        };
        let shown_type = if mime.is_empty() {
            self.blob_type.clone()
        } else {
            mime
        };
        let kind = preview_kind(&shown_type);
        let machine_icon = {
            let store = Store::global(cx).read(cx);
            roostr_core::objects::object_icon(store, "", "machine", "")
        };
        let has_harness = roostr_core::harness::harness(cx).is_some();
        let muted = |t: String| {
            div()
                .text_size(px(13.))
                .line_height(px(19.5))
                .text_color(theme::muted())
                .child(t)
        };
        let error = |t: String| {
            div()
                .text_size(px(13.))
                .line_height(px(19.5))
                .text_color(theme::red())
                .child(t)
        };

        // A small-caps key, aligned with the value's first line.
        let key = |t: &'static str, line: f32| {
            div()
                .text_size(px(11.))
                .line_height(px(line))
                .text_color(theme::muted())
                .child(t)
                .into_any_element()
        };
        let list = || {
            DescriptionList::horizontal()
                .bordered(false)
                .columns(1)
                .label_width(px(80.))
        };

        let mut facts = list();
        if size > 0. || !hash.is_empty() {
            facts = facts.item(key("SIZE", 19.5), crate::files::human_size(size), 1);
        }
        if !shown_type.is_empty() {
            facts = facts.item(key("TYPE", 19.5), shown_type.clone(), 1);
        }
        if !hash.is_empty() {
            facts = facts.item(
                key("HASH", 19.5),
                div()
                    .id("file-hash")
                    .text_size(px(12.))
                    .font_family(theme::fonts::MONO)
                    .tooltip(roostr_core::ui::tooltip(hash.clone()))
                    .child(hash.chars().take(12).collect::<String>())
                    .into_any_element(),
                1,
            );
        }

        // File.svelte `.file`: `margin: 16px 0 0 48px`, `16px 16px 0` at ≤720px.
        let narrow = roostr_core::layout::is_mobile(window);
        let mut page = div()
            .flex()
            .flex_col()
            .gap(px(20.))
            .mt(px(16.))
            .when(narrow, |d| d.mx(px(16.)))
            .when(!narrow, |d| d.ml(px(48.)))
            .max_w(px(720.))
            .child(
                div()
                    .text_size(px(13.))
                    .line_height(px(19.5))
                    .text_color(theme::muted())
                    .child(facts),
            );
        if !object_error.is_empty() {
            page = page.child(error(object_error.clone()));
        }
        if !has_harness && !on_blossom {
            page = page.child(muted(
                "Start Roostr on this computer to see this file.".into(),
            ));
        } else if hash.is_empty() {
            page = page.child(muted("This file has no bytes yet.".into()));
        } else {
            let mut sec = div().flex().flex_col().gap(px(8.));
            if self.loading {
                sec = sec.child(
                    Attachment::new()
                        .status(AttachmentStatus::Processing)
                        .media(AttachmentMedia::new())
                        .content(
                            AttachmentContent::new().title(
                                AttachmentTitle::new(if fetching_from.is_empty() {
                                    "Loading…".to_string()
                                } else {
                                    format!("Fetching from {fetching_from}…")
                                })
                                .font_weight(FontWeight::NORMAL)
                                .text_color(theme::muted()),
                            ),
                        )
                        .w_full()
                        .text_size(px(13.))
                        .rounded(px(10.))
                        .bg(theme::bg())
                        .border_color(theme::border()),
                );
            } else if !self.fetch_error.is_empty() && self.fetch_error != object_error {
                sec = sec.child(error(self.fetch_error.clone()));
            } else if let Some(bytes) = self.bytes.clone() {
                let preview_frame = || {
                    div()
                        .max_w_full()
                        .border_1()
                        .border_color(theme::border())
                        .rounded(px(10.))
                        .bg(theme::bg())
                        .overflow_hidden()
                };
                match kind {
                    Kind::Image => {
                        if let Some(image) = self.image.clone() {
                            let max_h = window.viewport_size().height * 0.7;
                            sec = sec.child(
                                preview_frame().self_start().max_h(max_h).child(
                                    img(ImageSource::Image(image))
                                        .max_w_full()
                                        .max_h(max_h)
                                        .object_fit(ObjectFit::Contain),
                                ),
                            );
                        }
                    }
                    Kind::Text => {
                        if let Some(text) = self.text.clone() {
                            sec = sec.child(
                                preview_frame()
                                    .id("file-text")
                                    .p(px(12.))
                                    .max_h(window.viewport_size().height * 0.6)
                                    .overflow_y_scroll()
                                    .text_size(px(12.5))
                                    .line_height(px(18.75))
                                    .text_color(theme::fg())
                                    .font_family(theme::fonts::MONO)
                                    .child(text),
                            );
                            if size > TEXT_PREVIEW_BYTES as f64 {
                                sec = sec.child(muted(format!(
                                    "Showing the first {} - download for the rest.",
                                    crate::files::human_size(TEXT_PREVIEW_BYTES as f64)
                                )));
                            }
                        }
                    }
                    Kind::Video | Kind::Audio | Kind::Pdf => {
                        // No media/PDF view in GPUI: open it in the system's player/viewer.
                        let open_bytes = bytes.clone();
                        let open_name = if name.is_empty() {
                            "file".to_string()
                        } else {
                            name.clone()
                        };
                        let label = match kind {
                            Kind::Video => "Play in the system player",
                            Kind::Audio => "Play in the system player",
                            _ => "Open in the system viewer",
                        };
                        sec = sec.child(
                            Attachment::new()
                                .id("file-open")
                                .on_click(move |_, _, cx| {
                                    crate::files::open_externally(
                                        open_bytes.clone(),
                                        &open_name,
                                        cx,
                                    )
                                    .detach_and_log_err(cx)
                                })
                                .media(AttachmentMedia::new().child(Icon::new(match kind {
                                    Kind::Pdf => IconName::FileText,
                                    _ => IconName::Play,
                                })))
                                .content(
                                    AttachmentContent::new().title(
                                        AttachmentTitle::new(label)
                                            .font_weight(FontWeight::NORMAL)
                                            .text_color(theme::fg()),
                                    ),
                                )
                                .w_full()
                                .h_auto()
                                .gap(px(10.))
                                .px(px(14.))
                                .py(px(12.))
                                .rounded(px(10.))
                                .text_size(px(13.)),
                        );
                    }
                    Kind::None => {}
                }
            }
            let mut actions = div().flex().flex_wrap().gap(px(8.));
            if let Some(bytes) = self.bytes.clone() {
                let file_name = if name.is_empty() {
                    "file".to_string()
                } else {
                    name.clone()
                };
                actions = actions.child(Self::button("file-download", "Download", cx).on_click(
                    move |_, _, cx| {
                        crate::files::save_bytes(bytes.as_ref().clone(), file_name.clone(), cx)
                            .detach_and_log_err(cx);
                    },
                ));
            }
            if !self.fetch_error.is_empty() || !object_error.is_empty() {
                let loading = self.loading;
                actions = actions.child(
                    Self::button("file-retry", "Retry", cx)
                        .when(loading, |b| b.opacity(0.5))
                        .when(!loading, |b| {
                            b.on_click(cx.listener(|this, _, _, cx| this.retry(cx)))
                        }),
                );
            }
            sec = sec.child(actions);
            page = page.child(sec);
        }

        let holders_empty = holders.is_empty();
        let stored =
            if holders_empty {
                muted(if on_blossom {
                    "No computer keeps a copy yet - the file is on Blossom".into()
                } else {
                    "No computer holds this file yet".into()
                })
                .into_any_element()
            } else {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .text_size(px(14.))
                    .line_height(px(21.))
                    .text_color(theme::fg())
                    .children(holders.into_iter().enumerate().map(
                        |(i, (machine_id, row, label))| {
                            let mine = machine_id == self.this_machine;
                            div()
                                .flex()
                                .items_baseline()
                                .gap(px(6.))
                                .child(machine_icon.clone())
                                .child(match row {
                                    Some(row) => Link::new(("holder", i))
                                        .text_color(theme::fg())
                                        .on_click(move |_, _, cx| {
                                            roostr_core::nav::navigate(
                                                Route::Object(row.id.clone()),
                                                cx,
                                            )
                                        })
                                        .child(label)
                                        .into_any_element(),
                                    None => div().child(label).into_any_element(),
                                })
                                .when(mine, |d| {
                                    d.child(
                                        div()
                                            .text_size(px(13.))
                                            .text_color(theme::muted())
                                            .child("(this computer)"),
                                    )
                                })
                        },
                    ))
                    .into_any_element()
            };
        let stored_line = if holders_empty { 19.5 } else { 21. };
        page.child(list().item(key("STORED ON", stored_line), stored, 1))
    }
}
