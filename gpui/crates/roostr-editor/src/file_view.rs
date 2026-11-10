//! A File object's page (`File.svelte`). The object carries the facts
//! (name, hash, size, MIME, which computers hold the bytes) and, for files
//! on Blossom, the blob and its key: those bytes are fetched and decrypted
//! right here. Older files without a blob come from the harness on this
//! computer, which fetches from a peer when it lacks them.
//!
//! Each open page is a [`FileView`] in the [`FileViews`] slot, keyed by
//! object id: [`open_file`] starts it loading, [`file_view`] draws it,
//! [`close_file`] drops it (fetches still running then find nothing).

use std::collections::HashMap;
use std::sync::Arc;

use roostr_core::nav::{Route, navigate};
use roostr_core::serving::{self, MachineRow};
use roostr_core::types::{Fields, Object, field_str};
use roostr_core::{State, StoreEvent};
use roostr_ui::{
    Button, ButtonCustomVariant, DescriptionList, Glyph, Images, Link, ScrollArea, ShimmerText,
    Spinner, Theme, glyph, radius, rems, ty,
};
use rune::{Color, Cursor, El, SurfaceFit, col, el, row, text};

use crate::files;

/// Text previews stop here; the download is always the whole file.
const TEXT_PREVIEW_BYTES: usize = 200 * 1024;

/// The window's height when no layout has been seen yet.
const FALLBACK_WINDOW_H: f32 = 900.;

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

/// Image types the app draws (GPUI's `ImageFormat::from_mime_type`).
pub(crate) fn drawable_image(mime: &str) -> bool {
    matches!(
        mime.to_ascii_lowercase()
            .split(';')
            .next()
            .unwrap_or("")
            .trim(),
        "image/png"
            | "image/jpeg"
            | "image/jpg"
            | "image/webp"
            | "image/gif"
            | "image/svg+xml"
            | "image/bmp"
            | "image/tiff"
            | "image/x-icon"
            | "image/vnd.microsoft.icon"
    )
}

/// The window's size from the last layout (`window.viewport_size()`):
/// every Roostr view ends in `roostr_ui::root`, keyed `kit-root`.
pub(crate) fn window_size(cx: &rune::Cx<State>) -> Option<[f32; 2]> {
    cx.layout_size("kit-root")
}

/// The window's height, [`FALLBACK_WINDOW_H`] before the first layout.
pub(crate) fn window_height(cx: &rune::Cx<State>) -> f32 {
    window_size(cx).map_or(FALLBACK_WINDOW_H, |[_, h]| h)
}

/// Every open File page, by object id.
#[derive(Default)]
pub struct FileViews {
    map: HashMap<String, FileView>,
    /// The store listener is in place.
    listening: bool,
}

/// A File object's page (File.svelte).
#[derive(Default)]
struct FileView {
    object: Option<Object>,
    machines: Vec<MachineRow>,
    this_machine: String,
    bytes: Option<Arc<Vec<u8>>>,
    /// The bytes are in [`Images`] under this uri (a drawable image).
    image: Option<String>,
    blob_type: String,
    text: Option<String>,
    loading: bool,
    fetch_error: String,
    /// Bumped on every (re)load: a slower earlier fetch never lands.
    generation: u64,
    loaded_key: String,
}

impl FileView {
    fn fields(&self) -> Fields {
        self.object
            .as_ref()
            .map(|o| o.fields.clone())
            .unwrap_or_default()
    }

    fn on_blossom(&self) -> bool {
        self.object
            .as_ref()
            .is_some_and(|o| !field_str(&o.fields, "blob_sha256").is_empty())
    }

    fn hash(&self) -> String {
        self.object
            .as_ref()
            .map(|o| field_str(&o.fields, "file_hash").to_lowercase())
            .unwrap_or_default()
    }
}

fn view_mut<'a>(s: &'a mut State, object_id: &str) -> Option<&'a mut FileView> {
    s.slot::<FileViews>().map.get_mut(object_id)
}

/// Open the page of File object `object_id` (once; later calls keep the open
/// one): it reads the object, then its bytes, and the machines that hold them.
pub fn open_file(s: &mut State, object_id: &str) {
    let views = s.slot::<FileViews>();
    if views.map.contains_key(object_id) {
        return;
    }
    views.map.insert(object_id.to_string(), FileView::default());
    if !views.listening {
        views.listening = true;
        s.on_store(on_store);
    }
    reload_object(s, object_id);
    let machines = serving::fetch_machines(s);
    let me = roostr_core::create::this_machine_id(s);
    let oid = object_id.to_string();
    s.rt.spawn(async move |cx| {
        let roster = machines.await.map(|(_, m)| m).unwrap_or_default();
        let me = me.await;
        cx.update(|s| {
            if let Some(v) = view_mut(s, &oid) {
                v.machines = roster;
                v.this_machine = me;
            }
        });
    })
    .detach();
}

/// Close the page of `object_id`.
pub fn close_file(s: &mut State, object_id: &str) {
    s.slot::<FileViews>().map.remove(object_id);
}

/// A commit to an open File (or a reconnect) reads it again.
fn on_store(s: &mut State, event: &StoreEvent) {
    let ids: Vec<String> = match event {
        StoreEvent::Committed(id) => s
            .get::<FileViews>()
            .filter(|v| v.map.contains_key(id))
            .map(|_| vec![id.clone()])
            .unwrap_or_default(),
        StoreEvent::Reconnected => s
            .get::<FileViews>()
            .map(|v| v.map.keys().cloned().collect())
            .unwrap_or_default(),
        StoreEvent::Refreshed | StoreEvent::Live(_) => Vec::new(),
    };
    for id in ids {
        reload_object(s, &id);
    }
}

fn reload_object(s: &mut State, object_id: &str) {
    let fetch = s.store.fetch_object(object_id);
    let oid = object_id.to_string();
    s.rt.spawn(async move |cx| {
        let Ok(object) = fetch.await else { return };
        cx.update(|s| {
            let Some(v) = view_mut(s, &oid) else { return };
            v.object = Some(object);
            load(s, &oid, false);
        });
    })
    .detach();
}

/// Fetch the bytes for the current hash (again when `force`).
fn load(s: &mut State, object_id: &str, force: bool) {
    let harness = s.harness.clone();
    let relays = files::relays(s);
    let rt = s.rt.clone();
    let Some(v) = view_mut(s, object_id) else {
        return;
    };
    let hash = v.hash();
    let on_blossom = v.on_blossom();
    let key = format!("{hash}|{on_blossom}");
    if !force && key == v.loaded_key {
        return;
    }
    v.loaded_key = key;
    v.generation += 1;
    let mine = v.generation;
    v.bytes = None;
    v.image = None;
    v.blob_type.clear();
    v.text = None;
    v.fetch_error.clear();
    if (harness.is_none() && !on_blossom) || hash.is_empty() {
        v.loading = false;
        return;
    }
    v.loading = true;
    let fields = v.fields();
    let mime = field_str(&fields, "file_mime").to_string();
    let task = rt.background(async move {
        let (bytes, fetched) = files::fetch_file_blob(&fields, &relays, harness.as_deref()).await?;
        // The object's MIME wins over whatever the transfer labelled it.
        let kind = if mime.is_empty() { fetched } else { mime };
        let text = (preview_kind(&kind) == Kind::Text).then(|| {
            String::from_utf8_lossy(&bytes[..bytes.len().min(TEXT_PREVIEW_BYTES)]).into_owned()
        });
        anyhow::Ok((bytes, kind, text))
    });
    let oid = object_id.to_string();
    rt.spawn(async move |cx| {
        let result = task.await;
        cx.update(|s| {
            let uri = format!("roostr-file:{oid}");
            let image = match &result {
                Ok((bytes, kind, _))
                    if preview_kind(kind) == Kind::Image && drawable_image(kind) =>
                {
                    Some(bytes.clone())
                }
                _ => None,
            };
            let Some(v) = view_mut(s, &oid) else { return };
            if mine != v.generation {
                return;
            }
            v.loading = false;
            match result {
                Ok((bytes, kind, text)) => {
                    v.image = image.is_some().then(|| uri.clone());
                    v.bytes = Some(Arc::new(bytes));
                    v.blob_type = kind;
                    v.text = text;
                }
                Err(err) => v.fetch_error = files::view_failure_text(&err),
            }
            if let (Some(bytes), Some(images)) = (image, s.get::<Images>()) {
                images.insert(&uri, bytes);
            }
        });
    })
    .detach();
}

/// Pick up what the harness wrote since (error, holders), then try the bytes again.
fn retry(s: &mut State, object_id: &str) {
    let fetch = s.store.fetch_object(object_id);
    let oid = object_id.to_string();
    s.rt.spawn(async move |cx| {
        let fresh = fetch.await.ok();
        cx.update(|s| {
            let Some(v) = view_mut(s, &oid) else { return };
            if let Some(o) = fresh {
                v.object = Some(o);
            }
            load(s, &oid, true);
        });
    })
    .detach();
}

/// The page's quiet action button. The custom variant's color is the
/// hover/press border and its foreground the hover/press text, so the
/// rest state (bg border, muted text) comes from the instance style.
fn button(key: String, label: &'static str, t: &Theme) -> Button {
    Button::new(key)
        .custom(
            ButtonCustomVariant::new(t)
                .color(t.accent)
                .foreground(t.fg)
                .hover(t.bg)
                .active(t.bg),
        )
        .accessibility_label(label)
        .label(label)
        .text_size(12.)
        .radius(7.)
        .h_auto()
        .px(12.)
        .py(5.)
        .bg(t.bg)
        .border_w(1.)
        .border_color(t.border)
        .text_color(t.muted)
        .pointer()
}

/// gpui-kit's medium `Attachment` in its processing state: a spinner in the
/// media square and the title shimmering, on `bg` with a `t.border` edge.
pub(crate) fn loading_card(key: String, title: String, corner: f32, bg: Color, t: &Theme) -> El {
    let font = ty::ui(13.).lh(13. * 1.25);
    row()
        .key(key.clone())
        .w_full()
        .min_w(0.)
        .h(rems(3.5))
        .items_center()
        .gap(rems(0.625))
        .pl(rems(0.5))
        .pr(rems(0.75))
        .radius(corner)
        .border(1., t.border)
        .border_layout()
        .fill(bg)
        .child(
            row()
                .size(rems(2.375), rems(2.375))
                .center()
                .radius(radius::KIT)
                .fill(t.kit.muted)
                .child(
                    Spinner::new()
                        .size(roostr_ui::KitSize::Small)
                        .color(t.kit.primary)
                        .build(t),
                ),
        )
        .child(
            col().grow(1.).shrink().min_w(0.).child(
                ShimmerText::new(format!("{key}-title"), title)
                    .font(font)
                    .color(t.muted)
                    .build(t),
            ),
        )
}

/// gpui-kit's medium `Attachment` that opens something: `icon` in the media
/// square, `title` (and a smaller `description`) beside it, padded 12 × 14
/// with a 10 gap, its hover fill the kit's muted at half strength. The
/// caller adds the press.
pub(crate) fn open_card(
    key: String,
    icon: Glyph,
    title: String,
    description: Option<&str>,
    corner: f32,
    t: &Theme,
) -> El {
    let lh = 1.25;
    let mut content = col().grow(1.).shrink().min_w(0.).gap(rems(0.125)).child(
        text(title)
            .font(ty::ui(13.).lh(13. * lh))
            .color(t.fg)
            .truncate(),
    );
    if let Some(description) = description {
        content = content.child(
            text(description.to_string())
                .font(ty::ui(11.).lh(11. * lh))
                .color(t.kit.muted_foreground)
                .truncate(),
        );
    }
    row()
        .key(key)
        .role("button")
        .w_full()
        .min_w(0.)
        .items_center()
        .gap(10.)
        .px(14.)
        .py(12.)
        .radius(corner)
        .border(1., t.kit.border)
        .border_layout()
        .fill(t.kit.background)
        .hover(t.kit.muted.alpha(t.kit.muted.a * 0.5))
        .cursor(Cursor::Pointer)
        .child(
            row()
                .size(rems(2.375), rems(2.375))
                .center()
                .radius(radius::KIT)
                .fill(t.kit.muted)
                .child(glyph(icon, rems(1.25)).color(t.kit.foreground)),
        )
        .child(content)
}

/// The page of File object `object_id` (open it first with [`open_file`]).
pub fn file_view(
    s: &State,
    cx: &mut rune::Cx<State>,
    t: &Theme,
    _layer: &mut Vec<El>,
    object_id: &str,
) -> El {
    let page_key = format!("file-{object_id}");
    let Some(v) = s.get::<FileViews>().and_then(|f| f.map.get(object_id)) else {
        return col().key(page_key);
    };
    let fields = v.fields();
    let name = field_str(&fields, "name").to_string();
    let hash = v.hash();
    let on_blossom = v.on_blossom();
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
                v.machines.iter().find(|r| r.machine_id == *m).cloned(),
                serving::machine_name(&v.machines, m),
            )
        })
        .collect();
    let fetching_from = if !v.this_machine.is_empty()
        && !available_on.contains(&v.this_machine)
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
        v.blob_type.clone()
    } else {
        mime
    };
    let kind = preview_kind(&shown_type);
    let machine_icon = roostr_core::objects::object_icon(&s.store, "", "machine", "");
    let has_harness = s.harness.is_some();
    let window = window_size(cx);
    let window_h = window.map_or(FALLBACK_WINDOW_H, |[_, h]| h);
    let page_w = cx.layout_size(&page_key).map_or(720., |[w, _]| w);
    let muted = |t_: String| text(t_).font(ty::ui(13.).lh(19.5)).color(t.muted).wrap();
    let error = |t_: String| text(t_).font(ty::ui(13.).lh(19.5)).color(t.red).wrap();

    // A small-caps key, aligned with the value's first line.
    let key = |t_: &'static str, line: f32| text(t_).font(ty::ui(11.).lh(line)).color(t.muted);
    let list = || {
        DescriptionList::horizontal()
            .bordered(false)
            .columns(1)
            .label_width(80.)
    };

    let mut facts = list();
    if size > 0. || !hash.is_empty() {
        facts = facts.item(key("SIZE", 19.5), muted(files::human_size(size)), 1);
    }
    if !shown_type.is_empty() {
        facts = facts.item(key("TYPE", 19.5), muted(shown_type.clone()), 1);
    }
    if !hash.is_empty() {
        let short = hash.chars().take(12).collect::<String>();
        facts = facts.item(
            key("HASH", 19.5),
            roostr_ui::tooltip(
                el().key(format!("{page_key}-hash"))
                    .child(text(short).font(ty::mono(12.).lh(19.5)).color(t.muted)),
                hash.clone(),
            ),
            1,
        );
    }

    // File.svelte `.file`: `margin: 16px 0 0 48px`, `16px 16px 0` at ≤720px.
    let narrow = window.is_some_and(|[w, _]| roostr_ui::layout::is_mobile(w));
    let mut page = col()
        .key(page_key.clone())
        .gap(20.)
        .mt(16.)
        .max_w(720.)
        .child(facts.build(t));
    page = if narrow { page.mx(16.) } else { page.ml(48.) };
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
        let mut sec = col().gap(8.);
        if v.loading {
            sec = sec.child(loading_card(
                format!("{page_key}-loading"),
                if fetching_from.is_empty() {
                    "Loading…".to_string()
                } else {
                    format!("Fetching from {fetching_from}…")
                },
                10.,
                t.bg,
                t,
            ));
        } else if !v.fetch_error.is_empty() && v.fetch_error != object_error {
            sec = sec.child(error(v.fetch_error.clone()));
        } else if let Some(bytes) = v.bytes.clone() {
            let preview_frame = || {
                col()
                    .max_w(page_w)
                    .border(1., t.border)
                    .border_layout()
                    .radius(10.)
                    .clip_rounded(10.)
                    .fill(t.bg)
            };
            match kind {
                Kind::Image => {
                    let image = v
                        .image
                        .as_deref()
                        .and_then(|uri| Some((uri, s.get::<Images>()?)))
                        .and_then(|(uri, images)| images.image(uri, window_h * 0.7));
                    if let Some(image) = image {
                        let max_h = window_h * 0.7;
                        // `max-width: 100%; max-height: 70vh; object-fit: contain`
                        // inside the frame's 1px border.
                        let room = (page_w - 2.).max(1.);
                        let (iw, ih) = image
                            .size()
                            .map(|[w, h]| (w as f32, h as f32))
                            .unwrap_or((room, room * 0.6));
                        let mut dw = iw.min(room);
                        let mut dh = dw * ih / iw.max(1.);
                        if dh > max_h {
                            dh = max_h;
                            dw = dh * iw / ih.max(1.);
                        }
                        sec = sec.child(
                            preview_frame().self_start().child(
                                el().size(dw, dh)
                                    .pixels_fit(image.pixels(), SurfaceFit::Fit),
                            ),
                        );
                    }
                }
                Kind::Text => {
                    if let Some(body) = v.text.clone() {
                        sec = sec.child(
                            preview_frame().w_full().child(
                                ScrollArea::vertical(
                                    format!("{page_key}-text"),
                                    col().padding(12.).child(
                                        text(body)
                                            .font(ty::mono(12.5).lh(18.75))
                                            .color(t.fg)
                                            .wrap(),
                                    ),
                                )
                                .w_full()
                                .max_h(window_h * 0.6)
                                .build(t),
                            ),
                        );
                        if size > TEXT_PREVIEW_BYTES as f64 {
                            sec = sec.child(muted(format!(
                                "Showing the first {} - download for the rest.",
                                files::human_size(TEXT_PREVIEW_BYTES as f64)
                            )));
                        }
                    }
                }
                Kind::Video | Kind::Audio | Kind::Pdf => {
                    // Open it in the system's player/viewer, as the GPUI app did.
                    let open_bytes = bytes.clone();
                    let open_name = if name.is_empty() {
                        "file".to_string()
                    } else {
                        name.clone()
                    };
                    let label = match kind {
                        Kind::Video | Kind::Audio => "Play in the system player",
                        _ => "Open in the system viewer",
                    };
                    let icon = match kind {
                        Kind::Pdf => Glyph::FileText,
                        _ => Glyph::Play,
                    };
                    sec = sec.child(
                        open_card(
                            format!("{page_key}-open"),
                            icon,
                            label.to_string(),
                            None,
                            10.,
                            t,
                        )
                        .on_press(move |s: &mut State| {
                            files::open_externally(open_bytes.clone(), &open_name, s)
                                .detach_and_log_err(&s.rt)
                        }),
                    );
                }
                Kind::None => {}
            }
        }
        let mut actions = row().flex_wrap().gap(8.);
        if let Some(bytes) = v.bytes.clone() {
            let file_name = if name.is_empty() {
                "file".to_string()
            } else {
                name.clone()
            };
            actions = actions.child(
                button(format!("{page_key}-download"), "Download", t)
                    .on_press_tx(move |_: &mut State, tx: &mut rune::Tx| {
                        files::save_bytes(bytes.as_ref().clone(), file_name.clone(), tx)
                    })
                    .build(t),
            );
        }
        if !v.fetch_error.is_empty() || !object_error.is_empty() {
            let oid = object_id.to_string();
            actions = actions.child(
                button(format!("{page_key}-retry"), "Retry", t)
                    .disabled(v.loading)
                    .on_press(move |s: &mut State| retry(s, &oid))
                    .build(t),
            );
        }
        sec = sec.child(actions);
        page = page.child(sec);
    }

    let holders_empty = holders.is_empty();
    let stored = if holders_empty {
        muted(if on_blossom {
            "No computer keeps a copy yet - the file is on Blossom".into()
        } else {
            "No computer holds this file yet".into()
        })
    } else {
        let line = ty::ui(14.).lh(21.);
        let images = s.get::<Images>();
        let mut rows = col().gap(4.);
        for (i, (machine_id, machine, label)) in holders.into_iter().enumerate() {
            let mine = machine_id == v.this_machine;
            let icon = match images {
                Some(images) => roostr_ui::icon_text(&machine_icon, 14., line, images),
                None => text(machine_icon.clone()).font(line),
            };
            let mut holder = row()
                .items_center()
                .gap(6.)
                .child(icon)
                .child(match machine {
                    Some(machine) => Link::new(format!("{page_key}-holder-{i}"))
                        .label(label)
                        .color(t.fg)
                        .text_size(14.)
                        .on_press(move |s: &mut State| {
                            navigate(Route::Object(machine.id.clone()), s)
                        })
                        .build(t),
                    None => text(label).font(line).color(t.fg),
                });
            if mine {
                holder = holder.child(text("(this computer)").font(ty::ui(13.)).color(t.muted));
            }
            rows = rows.child(holder);
        }
        rows
    };
    let stored_line = if holders_empty { 19.5 } else { 21. };
    page.child(
        list()
            .item(key("STORED ON", stored_line), stored, 1)
            .build(t),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previews_follow_the_mime_type() {
        assert!(preview_kind("image/png") == Kind::Image);
        assert!(preview_kind("Text/Plain; charset=utf-8") == Kind::Text);
        assert!(preview_kind("application/ld+json") == Kind::Text);
        assert!(preview_kind("application/pdf") == Kind::Pdf);
        assert!(preview_kind("audio/mpeg") == Kind::Audio);
        assert!(preview_kind("video/mp4") == Kind::Video);
        assert!(preview_kind("application/zip") == Kind::None);
        assert!(drawable_image("image/jpeg"));
        assert!(!drawable_image("image/heic"));
    }
}
