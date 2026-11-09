//! A page's cover ([`roostr_core::cover`]): the banner above the icon and
//! title (the icon overlaps its bottom edge) and, as Anytype, one Cover
//! button (desktop: on hover; phone: the ⋯ menu) opening the cover menu -
//! Upload / Generate tabs, and Reposition / Remove for a page with a cover.
//! Generating spins on the banner until the new image replaces the cover.

use roostr_core::State;
use roostr_core::cover;
use roostr_core::create::add_failure_text;
use roostr_core::nav::{Route, navigate};
use roostr_core::types::Object;
use roostr_editor::PickedFile;
use roostr_ui::overlay::{anchored, dismissable};
use roostr_ui::ty::Weights as _;
use roostr_ui::{
    Glyph, Images, Placement, Spinner, Textarea, TextareaState, Theme, color, glyph, radius,
    shadow, ty,
};
use rune::{Cursor, Drag, El, Phase, Tx, col, el, row, text};

use super::{page, page_mut};

/// Banner heights: desktop, phone.
const BANNER_H: f32 = 200.;
const BANNER_H_PHONE: f32 = 140.;

/// How far the page icon reaches up into the cover: desktop, phone.
pub(super) const ICON_OVERLAP: f32 = 44.;
pub(super) const ICON_OVERLAP_PHONE: f32 = 30.;

/// Width of the cover menu (Anytype's `menuBlockCover`).
const MENU_W: f32 = 408.;

/// The key of the button that opened the cover menu (Cover / Add cover):
/// the floating menu opens under it.
const TRIGGER: &str = "cover-trigger";

/// The cover's state on one page.
#[derive(Default)]
pub(super) struct CoverUi {
    /// The banner's File and whether its image is in [`Images`] (under [`image_uri`]).
    banner: Option<(String, Banner)>,
    /// The pointer is over the icon and title (desktop: shows Add cover).
    pub(super) head_hovered: bool,
    /// The pointer is over the banner (desktop: shows its actions).
    banner_hovered: bool,
    reposition: Option<Reposition>,
    chooser: Option<Chooser>,
    /// Generate's prompt box, while open.
    prompt: Option<TextareaState>,
    /// Asking for a cover failed before any computer saw it.
    ask_error: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Banner {
    Loading,
    Ready,
    Unreadable,
}

/// Reposition: the focus being dragged, unsaved.
struct Reposition {
    y: f32,
    /// The focus when the drag began.
    drag: Option<f32>,
}

/// Add cover / Change cover: Anytype's cover menu - its tabs over the way
/// to set one (an image, or a generated one).
#[derive(Default)]
struct Chooser {
    tab: Tab,
    adding: bool,
    error: String,
}

/// The cover menu's tabs (Anytype's head row).
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Tab {
    #[default]
    Upload,
    Generate,
}

/// Where a File's bytes are kept in [`Images`].
fn image_uri(file_id: &str) -> String {
    format!("roostr-file:{file_id}")
}

/// The banner image positioned like CSS `object-fit: cover` with
/// `object-position: 50% (y * 100)%`: `[x, y, w, h]` inside a banner of `banner` `[w, h]`.
pub(super) fn cover_rect(banner: [f32; 2], image: [f32; 2], y: f32) -> [f32; 4] {
    let [w, h] = banner;
    let scale = (w / image[0]).max(h / image[1]);
    let (iw, ih) = (image[0] * scale, image[1] * scale);
    [(w - iw) / 2., (h - ih) * y, iw, ih]
}

/// How much taller the image is than the banner once it fills its width.
pub(super) fn overflow(banner: [f32; 2], image: [f32; 2]) -> f32 {
    if image[0] <= 0. || image[1] <= 0. {
        return 0.;
    }
    (banner[0] * image[1] / image[0] - banner[1]).max(0.)
}

/// The decoded size of the banner's image, once it decoded.
fn image_size(s: &State, file: &str) -> Option<[f32; 2]> {
    let image = s.get::<Images>()?.image(&image_uri(file), BANNER_H)?;
    let [w, h] = image.size()?;
    Some([w as f32, h as f32])
}

/// Bring the banner in line with the page's cover (store listener, page sync).
pub(super) fn sync(s: &mut State, id: &str, object: &Object) {
    let file = cover::can_have_cover(object)
        .then(|| cover::cover_file(&object.fields))
        .flatten();
    let Some(page) = page_mut(s, id) else {
        return;
    };
    if file.is_none() {
        page.cover.reposition = None;
    }
    if page.cover.banner.as_ref().map(|(f, _)| f) == file.as_ref() {
        return;
    }
    let Some(file) = file else {
        page.cover.banner = None;
        return;
    };
    let cached = s
        .get::<Images>()
        .and_then(|images| images.image(&image_uri(&file), BANNER_H))
        .is_some();
    let page = page_mut(s, id).expect("checked above");
    if cached {
        page.cover.banner = Some((file, Banner::Ready));
        return;
    }
    page.cover.banner = Some((file.clone(), Banner::Loading));
    let load = roostr_editor::file_image(&file, s);
    let id = id.to_string();
    s.rt.spawn(async move |cx| {
        let loaded = load.await;
        cx.update(|s| {
            let state = match loaded {
                Ok((bytes, _mime)) => match s.get::<Images>() {
                    Some(images) => {
                        images.insert(&image_uri(&file), bytes);
                        Banner::Ready
                    }
                    None => Banner::Unreadable,
                },
                Err(error) => {
                    log::warn!("cover {file}: {error:#}");
                    Banner::Unreadable
                }
            };
            if let Some(page) = page_mut(s, &id)
                && let Some((shown, slot)) = &mut page.cover.banner
                && *shown == file
            {
                *slot = state;
            }
        });
    })
    .detach();
}

/// The phone's ⋯ menu asked for this page's cover menu.
pub(super) fn on_ask(s: &mut State, id: &str) {
    open_chooser(s, id);
}

fn open_chooser(s: &mut State, id: &str) {
    if let Some(page) = page_mut(s, id) {
        page.cover.chooser = Some(Chooser::default());
    }
}

/// The Cover / Add cover button: opens the menu, or closes it when open.
fn toggle_chooser(s: &mut State, id: &str) {
    let open = page(s, id).is_some_and(|p| p.cover.chooser.is_some());
    if open {
        close_chooser(s, id);
    } else {
        open_chooser(s, id);
    }
}

fn close_chooser(s: &mut State, id: &str) {
    if let Some(page) = page_mut(s, id) {
        page.cover.chooser = None;
        page.cover.prompt = None;
    }
}

/// A press outside the menu closes it, unless an image is being added.
fn dismiss_chooser(s: &mut State, id: &str) {
    let busy = page(s, id)
        .and_then(|p| p.cover.chooser.as_ref())
        .is_some_and(|c| c.adding);
    if !busy {
        close_chooser(s, id);
    }
}

fn pick_cover(s: &mut State, id: &str) {
    let pick = roostr_editor::pick_file(s);
    let id = id.to_string();
    s.rt.spawn(async move |cx| {
        if let Some(file) = pick.await {
            cx.update(|s| add_cover(s, &id, file));
        }
    })
    .detach();
}

fn paste_cover(id: &str, tx: &mut Tx) {
    let id = id.to_string();
    tx.read_clipboard(move |s: &mut State, content, _| match content.image() {
        Some(image) => add_cover(s, &id, roostr_editor::pasted_image(image)),
        None => {
            if let Some(chooser) = page_mut(s, &id).and_then(|p| p.cover.chooser.as_mut()) {
                chooser.error = "There's no image on the clipboard - copy one first.".into();
            }
        }
    });
}

/// Add `file` to the page's space the usual way (a File object), then make it the cover.
fn add_cover(s: &mut State, id: &str, file: PickedFile) {
    let Some(object) = s.store.object(id) else {
        return;
    };
    let add = s.add_file.clone();
    let Some(chooser) = page_mut(s, id).and_then(|p| p.cover.chooser.as_mut()) else {
        return;
    };
    if !file.mime.starts_with("image/") {
        chooser.error = format!(
            "{} isn't an image - choose a PNG, JPEG, GIF or WebP.",
            file.name
        );
        return;
    }
    let Some(add) = add else {
        chooser.error = "This app can't add files.".into();
        return;
    };
    chooser.adding = true;
    chooser.error.clear();
    let name = file.name.clone();
    let upload = (add.0)(
        file.name,
        file.mime,
        file.bytes,
        cover::space_of(&object),
        s,
    );
    let id = id.to_string();
    s.rt.spawn(async move |cx| {
        let result = match upload.await {
            Ok(file_id) => {
                let set = cx.update(|s| cover::set_cover(&id, &file_id, s));
                set.await.map_err(|e| format!("{e:#}"))
            }
            Err(e) => Err(add_failure_text(&e, &name)),
        };
        cx.update(|s| {
            let Some(page) = page_mut(s, &id) else {
                return;
            };
            match result {
                Ok(()) => page.cover.chooser = None,
                Err(e) => {
                    if let Some(chooser) = &mut page.cover.chooser {
                        chooser.adding = false;
                        chooser.error = e;
                    }
                }
            }
        });
    })
    .detach();
}

fn start_reposition(s: &mut State, id: &str) {
    let Some(object) = s.store.object(id) else {
        return;
    };
    if cover::cover_file(&object.fields).is_some()
        && let Some(page) = page_mut(s, id)
    {
        page.cover.reposition = Some(Reposition {
            y: cover::cover_y(&object.fields),
            drag: None,
        });
    }
}

fn save_reposition(s: &mut State, id: &str) {
    if let Some(r) = page_mut(s, id).and_then(|p| p.cover.reposition.take()) {
        cover::set_y(id, r.y, s).detach_and_log_err(&s.rt);
    }
}

fn remove_cover(s: &mut State, id: &str) {
    if let Some(page) = page_mut(s, id) {
        page.cover.reposition = None;
    }
    cover::remove(id, s).detach_and_log_err(&s.rt);
}

/// The cover menu on its Generate tab: the person writes what the cover should show.
fn open_prompt(s: &mut State, id: &str) {
    let Some(page) = page_mut(s, id) else {
        return;
    };
    page.cover.chooser.get_or_insert_with(Chooser::default).tab = Tab::Generate;
    if page.cover.prompt.is_none() {
        let mut input = TextareaState::new().auto_grow(2, 6).submit_on_enter(true);
        input.focus();
        page.cover.prompt = Some(input);
    }
}

/// Paint the prompt box's words as the new cover; the banner spins until
/// the image replaces the old one.
fn generate(s: &mut State, id: &str) {
    let text = page(s, id)
        .and_then(|p| p.cover.prompt.as_ref())
        .map(|p| p.value().trim().to_string())
        .unwrap_or_default();
    if !text.is_empty() {
        ask_cover(s, id, &text);
    }
}

/// Ask for a cover - `prompt` painted as written, "" chosen from the
/// page - and close the prompt box.
fn ask_cover(s: &mut State, id: &str, prompt: &str) {
    let Some(object) = s.store.object(id) else {
        return;
    };
    if let Some(page) = page_mut(s, id) {
        page.cover.prompt = None;
        page.cover.chooser = None;
        page.cover.ask_error.clear();
    }
    let ask = cover::request(&object, prompt, s);
    let id = id.to_string();
    s.rt.spawn(async move |cx| {
        if let Err(e) = ask.await {
            cx.update(|s| {
                if let Some(page) = page_mut(s, &id) {
                    page.cover.ask_error = format!("{e:#}");
                }
            });
        }
    })
    .detach();
}

fn dismiss_failure(s: &mut State, id: &str) {
    if let Some(page) = page_mut(s, id) {
        page.cover.ask_error.clear();
    }
    if let Some(object) = s.store.object(id) {
        cover::dismiss(&object, s).detach_and_log_err(&s.rt);
    }
}

/// Reposition's drag: the image follows the pointer vertically.
fn drag_banner(s: &mut State, id: &str, d: &Drag) {
    let file = page(s, id).and_then(|p| p.cover.banner.as_ref().map(|(f, _)| f.clone()));
    let image = file.and_then(|f| image_size(s, &f)).unwrap_or_default();
    let overflow = overflow([d.w, d.h], image);
    let Some(r) = page_mut(s, id).and_then(|p| p.cover.reposition.as_mut()) else {
        return;
    };
    match d.phase {
        Phase::Down => r.drag = Some(r.y),
        Phase::Move => {
            if let Some(from) = r.drag
                && overflow > 0.
            {
                r.y = (from - d.dy / overflow).clamp(0., 1.);
            }
        }
        Phase::Up => r.drag = None,
    }
}

// ------------------------------------------------------------------ render

/// What the cover's views need from the page.
pub(super) struct Ctx<'a> {
    pub s: &'a State,
    pub cx: &'a mut rune::Cx<State>,
    pub t: &'static Theme,
    pub id: &'a str,
    pub object: &'a Object,
    pub mobile: bool,
}

/// A button laid over the cover image: light, so it reads on any picture.
fn pill(key: &str, label: &str, t: &Theme) -> roostr_ui::Button {
    roostr_ui::Button::new(key)
        .custom(
            roostr_ui::ButtonCustomVariant::new(t)
                .color(color::hsla(0., 0., 1., 0.88))
                .foreground(color::hsla(0., 0., 0.15, 1.))
                .hover(color::hsla(0., 0., 1., 1.))
                .active(color::hsla(0., 0., 0.94, 1.)),
        )
        .radius(6.)
        .h(26.)
        .px(9.)
        .accessibility_label(label)
        .label(label)
        .text_size(12.)
        .weight(ty::MEDIUM)
}

/// The banner, when the page has a cover.
pub(super) fn banner(c: &mut Ctx) -> Option<El> {
    let (s, t, id) = (c.s, c.t, c.id.to_string());
    let ui = &super::page(s, &id)?.cover;
    let generating = cover::generating(&c.object.fields);
    if ui.banner.is_none() && !generating {
        return None;
    }
    let y = ui
        .reposition
        .as_ref()
        .map_or_else(|| cover::cover_y(&c.object.fields), |r| r.y);
    let dragging = ui.reposition.as_ref().is_some_and(|r| r.drag.is_some());
    let radius = if c.mobile { 0. } else { radius::MD };
    let height = if c.mobile { BANNER_H_PHONE } else { BANNER_H };

    let mut banner = el()
        .key("object-cover")
        .w_full()
        .h(height)
        .radius(radius)
        .clip_rounded(radius)
        .fill(t.hover);
    if c.mobile {
        banner = banner.mt(-24.);
    }
    let image = ui.banner.as_ref().and_then(|(file, state)| {
        (*state == Banner::Ready)
            .then(|| s.get::<Images>()?.image(&image_uri(file), height))
            .flatten()
    });
    if let Some(image) = image {
        let painted = match (c.cx.layout_size("object-cover"), image.size()) {
            (Some(size), Some([iw, ih])) => {
                let [x, top, w, h] = cover_rect(size, [iw as f32, ih as f32], y);
                el().pin(Some(x), Some(top), None, None)
                    .size(w, h)
                    .pixels(image.pixels())
            }
            _ => el()
                .pin(Some(0.), Some(0.), Some(0.), Some(0.))
                .pixels_fit(image.pixels(), rune::SurfaceFit::Fill),
        };
        banner = banner.child(painted);
    }
    let repositioning = ui.reposition.is_some();
    if !generating && !repositioning {
        // A tap on the cover opens its menu: the phone has no hover to show
        // the Cover button (and a press on it closes the menu).
        let toggle = id.clone();
        banner = banner
            .cursor(Cursor::Pointer)
            .on_press(move |s: &mut State| toggle_chooser(s, &toggle));
    }
    let hover = id.clone();
    banner = banner.on_hover_change(move |s: &mut State, inside, _: &mut Tx| {
        if let Some(page) = page_mut(s, &hover) {
            page.cover.banner_hovered = inside;
        }
    });
    if repositioning {
        let drag = id.clone();
        banner = banner
            .cursor(if dragging {
                Cursor::Grabbing
            } else {
                Cursor::Grab
            })
            .on_drag(move |s: &mut State, d: &mut Drag, _: &mut Tx| drag_banner(s, &drag, d));
    }
    let actions =
        !c.mobile && !generating && (ui.banner_hovered || ui.chooser.is_some()) && !repositioning;
    if actions {
        // One Cover button, as Anytype: everything else is in its menu.
        let toggle = id.clone();
        banner = banner.child(
            row().pin(None, None, Some(12.), Some(12.)).gap(6.).child(
                el().key(TRIGGER).child(
                    pill("cover-change", "Cover", t)
                        .on_press(move |s: &mut State| toggle_chooser(s, &toggle))
                        .build(t),
                ),
            ),
        );
    }
    if generating {
        banner = banner.child(
            row()
                .key("cover-generating")
                .pin(Some(0.), Some(0.), Some(0.), Some(0.))
                .center()
                .fill(color::hsla(0., 0., 0., 0.25))
                .child(Spinner::new().color(t.white).build(t)),
        );
    }
    if repositioning {
        let (save, cancel) = (id.clone(), id.clone());
        banner = banner.child(
            row()
                .pin(Some(0.), Some(0.), Some(0.), Some(0.))
                .center()
                .child(
                    el().px(10.)
                        .py(5.)
                        .radius(6.)
                        .fill(color::hsla(0., 0., 0., 0.45))
                        .child(
                            text("Drag the image to reposition")
                                .font(ty::ui(12.))
                                .color(t.white),
                        ),
                )
                .child(
                    row()
                        .pin(None, None, Some(12.), Some(12.))
                        .gap(6.)
                        .child(
                            pill("cover-save", "Save position", t)
                                .on_press(move |s: &mut State| save_reposition(s, &save))
                                .build(t),
                        )
                        .child(
                            pill("cover-cancel", "Cancel", t)
                                .on_press(move |s: &mut State| {
                                    if let Some(page) = page_mut(s, &cancel) {
                                        page.cover.reposition = None;
                                    }
                                })
                                .build(t),
                        ),
                ),
        );
    }
    Some(banner)
}

/// Desktop, no cover, pointer over the icon and title: Add cover.
pub(super) fn add_cover_button(c: &Ctx) -> Option<El> {
    let ui = &super::page(c.s, c.id)?.cover;
    let offer = !c.mobile
        && (ui.head_hovered || ui.chooser.is_some())
        && ui.banner.is_none()
        && cover::can_have_cover(c.object);
    let t = c.t;
    let id = c.id.to_string();
    offer.then(|| {
        el().pin(None, Some(4.), Some(0.), None).child(
            el().key(TRIGGER).child(
                roostr_ui::Button::new("cover-add")
                    .ghost()
                    .h(26.)
                    .px(8.)
                    .text_color(t.muted)
                    .accessibility_label("Add cover")
                    .child(
                        row()
                            .center_y()
                            .gap(6.)
                            .child(glyph(Glyph::Plus, 14.).color(t.muted))
                            .child(text("Add cover").font(ty::ui(13.)).color(t.muted)),
                    )
                    .on_press(move |s: &mut State| toggle_chooser(s, &id))
                    .build(t),
            ),
        )
    })
}

/// The cover menu and why the last Generate failed. On desktop the menu
/// floats under the button that opened it (as Anytype's, centred on it);
/// on the phone, opened from the ⋯ menu, it sits under the title.
pub(super) fn cards(c: &mut Ctx, layer: &mut Vec<El>) -> El {
    let float = !c.mobile && c.cx.layout(TRIGGER).is_some();
    let has_cover = cover::cover_file(&c.object.fields).is_some();
    let mut column = col().gap(8.);
    if c.mobile {
        column = column.px(16.);
    }
    if let Some(menu) = chooser(c, has_cover, float) {
        // A press outside closes it, as a menu (not while an image is being added).
        let id = c.id.to_string();
        let dismiss = move |s: &mut State| dismiss_chooser(s, &id);
        if float {
            let menu = dismissable(menu, "cover-chooser", dismiss, layer);
            layer.push(anchored(menu, TRIGGER, Placement::Bottom, 6.));
        } else {
            column = column.child(menu.modal(true, dismiss));
        }
    }
    if let Some(failure) = failure(c) {
        column = column.child(failure);
    }
    column
}

/// Anytype's cover menu (`menuBlockCover`): 408px wide, a head row of
/// tabs (the active one in the primary text colour) over a hairline, then
/// the tab's body. Upload: the large drop-zone tile ("Choose Image") and
/// the secondary line under it; Generate: the prompt and its buttons.
fn chooser(c: &Ctx, has_cover: bool, float: bool) -> Option<El> {
    let (t, id) = (c.t, c.id);
    let ui = &super::page(c.s, id)?.cover;
    let chooser = ui.chooser.as_ref()?;
    let busy = chooser.adding;
    let head_tab = |key: &str, label: &str, active: bool| {
        el().key(key).cursor(Cursor::Pointer).child(
            text(label)
                .font(ty::ui(14.).medium())
                .color(if active { t.fg } else { t.muted }),
        )
    };
    let upload = id.to_string();
    let generate_tab = id.to_string();
    let mut head = row()
        .gap(16.)
        .px(14.)
        .pt(3.)
        .pb(10.)
        .border_sides([0., 0., 1., 0.], t.border)
        .child(
            head_tab("cover-tab-upload", "Upload", chooser.tab == Tab::Upload).on_press(
                move |s: &mut State| {
                    if let Some(c) = page_mut(s, &upload).and_then(|p| p.cover.chooser.as_mut()) {
                        c.tab = Tab::Upload;
                    }
                },
            ),
        )
        .child(
            head_tab(
                "cover-chooser-generate",
                "Generate",
                chooser.tab == Tab::Generate,
            )
            .on_press(move |s: &mut State| open_prompt(s, &generate_tab)),
        );
    if has_cover {
        // The cover's own actions, at the head's right (a page with a cover).
        let action = |key: &str, label: &str| {
            el().key(key)
                .cursor(Cursor::Pointer)
                .child(text(label).font(ty::ui(14.).medium()).color(t.muted))
        };
        let (reposition, remove) = (id.to_string(), id.to_string());
        head = head
            .child(el().grow(1.))
            .child(
                action("cover-reposition", "Reposition").on_press(move |s: &mut State| {
                    close_chooser(s, &reposition);
                    start_reposition(s, &reposition);
                }),
            )
            .child(
                action("cover-remove", "Remove").on_press(move |s: &mut State| {
                    close_chooser(s, &remove);
                    remove_cover(s, &remove);
                }),
            );
    }
    let body = match chooser.tab {
        Tab::Upload => {
            // `.dropzone`: radius 8, highlight fill, centred icon and label, 200px tall.
            let mut zone = col()
                .key("cover-pick")
                .min_h(200.)
                .center()
                .gap(6.)
                .radius(8.)
                .fill(t.hl_med)
                .child(glyph(Glyph::Download, 28.).color(t.muted))
                .child(
                    text(if busy {
                        "Adding the image…"
                    } else if roostr_editor::CAN_PICK_FILES {
                        "Choose Image"
                    } else {
                        "Paste an image"
                    })
                    .font(ty::ui(14.))
                    .color(t.fg),
                );
            if !busy {
                let pick = id.to_string();
                zone = zone.cursor(Cursor::Pointer).hover(t.hl_light).on_press_tx(
                    move |s: &mut State, tx: &mut Tx| {
                        if roostr_editor::CAN_PICK_FILES {
                            pick_cover(s, &pick)
                        } else {
                            paste_cover(&pick, tx)
                        }
                    },
                );
            }
            let mut body = col().gap(16.).padding(16.).child(zone);
            // Without a file dialog (the web) the tile itself pastes.
            if roostr_editor::CAN_PICK_FILES {
                // `.urlSection`: a small secondary label over the line's control.
                let mut line = row()
                    .key("cover-paste")
                    .h(36.)
                    .center_y()
                    .gap(8.)
                    .px(12.)
                    .radius(12.)
                    .fill(t.hl_light)
                    .child(glyph(Glyph::Copy, 16.).color(t.muted))
                    .child(text("Paste image").font(ty::ui(14.)).color(t.muted));
                if !busy {
                    let paste = id.to_string();
                    line = line
                        .cursor(Cursor::Pointer)
                        .hover(t.hl_med)
                        .on_press_tx(move |_: &mut State, tx: &mut Tx| paste_cover(&paste, tx));
                }
                body = body.child(
                    col()
                        .gap(6.)
                        .child(
                            text("Or paste an image you copied")
                                .font(ty::ui(12.))
                                .color(t.muted),
                        )
                        .child(line),
                );
            }
            if !chooser.error.is_empty() {
                body = body.child(small_note(&chooser.error, t.red));
            }
            body
        }
        Tab::Generate => {
            let empty = ui
                .prompt
                .as_ref()
                .is_none_or(|p| p.value().trim().is_empty());
            let mut body = col().key("cover-prompt").gap(12.).padding(16.);
            if let Some(prompt) = &ui.prompt {
                let (edit, submit) = (id.to_string(), id.to_string());
                body = body.child(
                    el().radius(12.).fill(t.hl_light).px(12.).py(8.).child(
                        Textarea::new("cover-prompt-input", prompt)
                            .appearance(false)
                            .text_size(14.)
                            .placeholder("Describe the cover…")
                            .on_edit(move |s: &mut State| {
                                page_mut(s, &edit)
                                    .expect("the prompt's page")
                                    .cover
                                    .prompt
                                    .get_or_insert_with(TextareaState::new)
                            })
                            .on_submit(move |s: &mut State, _: &mut Tx| generate(s, &submit))
                            .build(t),
                    ),
                );
            }
            let (go, choose) = (id.to_string(), id.to_string());
            body.child(
                text("Describe the cover, or let the page choose one.")
                    .font(ty::ui(12.))
                    .color(t.muted),
            )
            .child(
                row()
                    .gap(8.)
                    .child(
                        roostr_ui::disabled(
                            roostr_ui::primary_button("cover-prompt-generate", "Generate"),
                            empty,
                        )
                        .on_press(move |s: &mut State| generate(s, &go))
                        .build(t),
                    )
                    .child(
                        roostr_ui::outline_button("cover-prompt-choose", "Choose for me")
                            .on_press(move |s: &mut State| ask_cover(s, &choose, ""))
                            .build(t),
                    ),
            )
        }
    };
    let mut menu = col()
        .key("cover-chooser")
        .w(MENU_W)
        .max_w(f32::INFINITY)
        .pt(10.)
        .fill(t.panel)
        .border(1., t.border)
        .radius(radius::MENU)
        .box_shadow(shadow::menu())
        .child(head)
        .child(body);
    if !float {
        menu = menu.mb(12.);
    }
    Some(menu)
}

/// A failed Generate: OpenAI's (or the app's) words, the credential when
/// the key is the trouble, and a close button.
fn failure(c: &Ctx) -> Option<El> {
    let t = c.t;
    let ui = &super::page(c.s, c.id)?.cover;
    let failed =
        cover::generation(&c.object.fields).filter(|g| g.status == Some(cover::Status::Failed));
    let error = if ui.ask_error.is_empty() {
        failed.as_ref().map(|g| {
            if g.error.is_empty() {
                "The cover couldn't be made.".to_string()
            } else {
                g.error.clone()
            }
        })?
    } else {
        ui.ask_error.clone()
    };
    let credential = failed
        .filter(|g| g.needs_key && !g.credential.is_empty())
        .map(|g| g.credential);
    let close = c.id.to_string();
    let mut card = col()
        .key("cover-failed")
        .gap(10.)
        .padding(12.)
        .mb(12.)
        .radius(10.)
        .border(1., t.border)
        .fill(t.bg)
        .child(
            row()
                .center_y()
                .justify_between()
                .child(
                    text("Couldn't make a cover")
                        .font(ty::ui(13.).semibold())
                        .color(t.fg),
                )
                .child(
                    roostr_ui::icon_button("cover-failed-close", Glyph::BadgeX)
                        .accessibility_label("Close")
                        .tooltip("Close")
                        .on_press(move |s: &mut State| dismiss_failure(s, &close))
                        .build(t),
                ),
        )
        .child(small_note(&error, t.red).key("cover-error"));
    if let Some(credential) = credential {
        card = card.child(
            roostr_ui::outline_button("cover-open-credential", "Open the OpenAI credential")
                .on_press(move |s: &mut State| navigate(Route::Object(credential.clone()), s))
                .build(t),
        );
    }
    Some(card)
}

/// 12px text that wraps.
fn small_note(line: &str, color: rune::Color) -> El {
    text(line.to_owned())
        .font(ty::ui(12.).lh(12. * 1.4))
        .color(color)
        .wrap()
}

#[cfg(test)]
mod tests {
    use super::{cover_rect, overflow};

    #[test]
    fn the_cover_fills_the_banner_and_slides_with_its_focus() {
        let banner = [800., 200.];
        // A square image fills the width: 800 tall, 600 of it hidden.
        let square = [100., 100.];
        assert_eq!(cover_rect(banner, square, 0.), [0., 0., 800., 800.]);
        assert_eq!(cover_rect(banner, square, 1.)[1], -600.);
        assert_eq!(cover_rect(banner, square, 0.5)[1], -300.);
        assert_eq!(overflow(banner, square), 600.);
        // A very wide image fills the height and centres sideways: nothing to drag.
        let wide = [1000., 100.];
        assert_eq!(cover_rect(banner, wide, 0.), [-600., 0., 2000., 200.]);
        assert_eq!(overflow(banner, wide), 0.);
    }
}
