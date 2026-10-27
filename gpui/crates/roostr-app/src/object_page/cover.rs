//! A page's cover ([`roostr_core::cover`]): the banner above the icon and
//! title (the icon overlaps its bottom edge), its actions - desktop: on
//! hover; phone: the ⋯ menu - Reposition, the image chooser behind Add
//! cover / Change cover, and Generate: a spinner on the banner until the
//! new image replaces the cover.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::Icon;
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants as _};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::cover::{self, CoverAction};
use roostr_core::create::{AddFile, add_failure_text};
use roostr_core::nav::{Route, navigate};
use roostr_core::types::Object;
use roostr_core::{Glyph, theme, ui};
use roostr_editor::PickedFile;

use super::ObjectPage;

/// Banner heights: desktop, phone.
const BANNER_H: f32 = 200.;
const BANNER_H_PHONE: f32 = 140.;

/// How far the page icon reaches up into the cover: desktop, phone.
pub(super) const ICON_OVERLAP: f32 = 44.;
pub(super) const ICON_OVERLAP_PHONE: f32 = 30.;

/// The cover's state on one page.
#[derive(Default)]
pub(super) struct CoverUi {
    /// The banner's File and its image (`None` while loading or unreadable).
    banner: Option<(String, Option<Arc<Image>>)>,
    /// The pointer is over the icon and title (desktop: shows Add cover).
    head_hovered: bool,
    /// The pointer is over the banner (desktop: shows its actions).
    banner_hovered: bool,
    reposition: Option<Reposition>,
    chooser: Option<Chooser>,
    /// Asking for a cover failed before any computer saw it.
    ask_error: String,
    /// Where the banner was painted and its image's size, for dragging.
    geometry: Rc<Cell<Geometry>>,
}

#[derive(Clone, Copy, Default)]
struct Geometry {
    bounds: Bounds<Pixels>,
    image: Size<f32>,
}

impl Geometry {
    /// How much taller the image is than the banner once it fills its width.
    fn overflow(self) -> f32 {
        if self.image.width <= 0. || self.image.height <= 0. {
            return 0.;
        }
        let shown = f32::from(self.bounds.size.width) * self.image.height / self.image.width;
        (shown - f32::from(self.bounds.size.height)).max(0.)
    }
}

/// Reposition: the focus being dragged, unsaved.
struct Reposition {
    y: f32,
    /// Pointer y where the drag began, and the focus then.
    drag: Option<(Pixels, f32)>,
}

/// Add cover / Change cover: choose, paste or generate an image.
#[derive(Default)]
struct Chooser {
    adding: bool,
    error: String,
}
/// The banner image positioned like CSS `object-fit: cover` with
/// `object-position: 50% (y * 100)%`.
fn cover_bounds(bounds: Bounds<Pixels>, image: Size<f32>, y: f32) -> Bounds<Pixels> {
    let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let scale = (w / image.width).max(h / image.height);
    let size = size(px(image.width * scale), px(image.height * scale));
    Bounds {
        origin: point(
            bounds.origin.x + (bounds.size.width - size.width) / 2.,
            bounds.origin.y + (bounds.size.height - size.height) * y,
        ),
        size,
    }
}

/// A button laid over the cover image: light, so it reads on any picture.
fn pill(id: &'static str, label: &'static str, cx: &App) -> Button {
    Button::new(id)
        .custom(
            ButtonCustomVariant::new(cx)
                .color(hsla(0., 0., 1., 0.88))
                .foreground(hsla(0., 0., 0.15, 1.))
                .hover(hsla(0., 0., 1., 1.))
                .active(hsla(0., 0., 0.94, 1.)),
        )
        .rounded(px(6.))
        .h(px(26.))
        .px(px(9.))
        .accessibility_label(label)
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .child(label),
        )
}

/// `button` findable by `id` (tests, the inspector).
fn tagged(id: &'static str, button: impl IntoElement) -> Div {
    div().debug_selector(move || id.into()).child(button)
}

impl ObjectPage {
    /// Bring the banner in line with the page's cover.
    pub(super) fn sync_cover(&mut self, object: &Object, cx: &mut Context<Self>) {
        let file = cover::can_have_cover(object)
            .then(|| cover::cover_file(&object.fields))
            .flatten();
        if file.is_none() {
            self.cover.reposition = None;
        }
        if self.cover.banner.as_ref().map(|(f, _)| f) != file.as_ref() {
            self.cover.banner = file.clone().map(|f| (f, None));
            if let Some(file) = file {
                let load = roostr_editor::file_image(&file, cx);
                cx.spawn(async move |this, cx| {
                    let image = load.await.ok();
                    let _ = this.update(cx, |this, cx| {
                        if let Some((shown, slot)) = &mut this.cover.banner
                            && *shown == file
                        {
                            *slot = image;
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
        }
    }

    /// What the phone's ⋯ menu asked of this page's cover.
    pub(super) fn on_cover_ask(&mut self, action: CoverAction, cx: &mut Context<Self>) {
        match action {
            CoverAction::Choose => self.open_chooser(cx),
            CoverAction::Reposition => self.start_reposition(cx),
            CoverAction::Remove => self.remove_cover(cx),
            CoverAction::Generate => self.generate(cx),
        }
    }

    fn open_chooser(&mut self, cx: &mut Context<Self>) {
        self.cover.chooser = Some(Chooser::default());
        cx.notify();
    }

    fn pick_cover(&mut self, cx: &mut Context<Self>) {
        let pick = roostr_editor::pick_file(cx);
        cx.spawn(async move |this, cx| {
            if let Some(file) = pick.await {
                let _ = this.update(cx, |this, cx| this.add_cover(file, cx));
            }
        })
        .detach();
    }

    fn paste_cover(&mut self, cx: &mut Context<Self>) {
        let image = cx.read_from_clipboard().and_then(|item| {
            item.entries().iter().find_map(|e| match e {
                ClipboardEntry::Image(image) => Some(roostr_editor::pasted_image(image)),
                _ => None,
            })
        });
        match image {
            Some(file) => self.add_cover(file, cx),
            None => {
                if let Some(chooser) = &mut self.cover.chooser {
                    chooser.error = "There's no image on the clipboard - copy one first.".into();
                }
                cx.notify();
            }
        }
    }

    /// Add `file` to the page's space the usual way (a File object), then make it the cover.
    fn add_cover(&mut self, file: PickedFile, cx: &mut Context<Self>) {
        let Some(object) = self.object(cx) else {
            return;
        };
        let Some(chooser) = &mut self.cover.chooser else {
            return;
        };
        if !file.mime.starts_with("image/") {
            chooser.error = format!(
                "{} isn't an image - choose a PNG, JPEG, GIF or WebP.",
                file.name
            );
            cx.notify();
            return;
        }
        chooser.adding = true;
        chooser.error.clear();
        cx.notify();
        let AddFile(add) = cx.global::<AddFile>().clone();
        let name = file.name.clone();
        let upload = add(
            file.name,
            file.mime,
            file.bytes,
            cover::space_of(&object),
            cx,
        );
        let page = object.id.clone();
        cx.spawn(async move |this, cx| {
            let result = match upload.await {
                Ok(id) => cx
                    .update(|cx| cover::set_cover(&page, &id, cx))
                    .await
                    .map_err(|e| format!("{e:#}")),
                Err(e) => Err(add_failure_text(&e, &name)),
            };
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => this.cover.chooser = None,
                    Err(e) => {
                        if let Some(chooser) = &mut this.cover.chooser {
                            chooser.adding = false;
                            chooser.error = e;
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn start_reposition(&mut self, cx: &mut Context<Self>) {
        let Some(object) = self.object(cx) else {
            return;
        };
        if cover::cover_file(&object.fields).is_some() {
            self.cover.reposition = Some(Reposition {
                y: cover::cover_y(&object.fields),
                drag: None,
            });
            cx.notify();
        }
    }

    fn save_reposition(&mut self, cx: &mut Context<Self>) {
        if let Some(r) = self.cover.reposition.take() {
            cover::set_y(&self.id, r.y, cx).detach_and_log_err(cx);
        }
        cx.notify();
    }

    fn remove_cover(&mut self, cx: &mut Context<Self>) {
        self.cover.reposition = None;
        cover::remove(&self.id, cx).detach_and_log_err(cx);
        cx.notify();
    }

    /// Ask for a new cover; the banner spins until it replaces the old one.
    fn generate(&mut self, cx: &mut Context<Self>) {
        let Some(object) = self.object(cx) else {
            return;
        };
        self.cover.chooser = None;
        self.cover.ask_error.clear();
        let ask = cover::request(&object, cx);
        cx.spawn(async move |this, cx| {
            if let Err(e) = ask.await {
                let _ = this.update(cx, |this, cx| {
                    this.cover.ask_error = format!("{e:#}");
                    cx.notify();
                });
            }
        })
        .detach();
        cx.notify();
    }

    fn dismiss_failure(&mut self, cx: &mut Context<Self>) {
        self.cover.ask_error.clear();
        if let Some(object) = self.object(cx) {
            cover::dismiss(&object, cx).detach_and_log_err(cx);
        }
        cx.notify();
    }

    // ----------------------------------------------------------------- render

    /// The banner, when the page has a cover.
    pub(super) fn render_cover_banner(
        &mut self,
        object: &Object,
        mobile: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let generating = cover::generating(&object.fields);
        if self.cover.banner.is_none() && !generating {
            return None;
        }
        let image = self.cover.banner.as_ref().and_then(|(_, i)| i.clone());
        let repositioning = self.cover.reposition.as_ref();
        let y = repositioning.map_or_else(|| cover::cover_y(&object.fields), |r| r.y);
        let dragging = repositioning.is_some_and(|r| r.drag.is_some());
        let geometry = self.cover.geometry.clone();
        let radius = if mobile { px(0.) } else { theme::radius::md() };
        let painter = canvas(
            move |bounds, window, cx| {
                let render = image.and_then(|image| image.use_render_image(window, cx));
                let size = render.as_ref().map_or_else(Size::default, |r| {
                    let s = r.size(0);
                    size(s.width.0 as f32, s.height.0 as f32)
                });
                geometry.set(Geometry {
                    bounds,
                    image: size,
                });
                render.map(|r| (r, size))
            },
            move |bounds, render, window, _| {
                if let Some((render, size)) = render {
                    let _ = window.paint_image(
                        bounds,
                        cover_bounds(bounds, size, y),
                        Corners::all(radius),
                        render,
                        0,
                        false,
                    );
                }
            },
        )
        .absolute()
        .inset_0();
        let actions = (!mobile
            && !generating
            && self.cover.banner_hovered
            && self.cover.reposition.is_none())
            .then(|| {
                div()
                    .absolute()
                    .right(px(12.))
                    .bottom(px(12.))
                    .flex()
                    .gap(px(6.))
                    .child(tagged(
                        "cover-change",
                        pill("cover-change", "Change cover", cx)
                            .on_click(cx.listener(|this, _, _, cx| this.open_chooser(cx))),
                    ))
                    .child(tagged(
                        "cover-generate",
                        pill("cover-generate", "Generate", cx)
                            .on_click(cx.listener(|this, _, _, cx| this.generate(cx))),
                    ))
                    .child(tagged(
                        "cover-reposition",
                        pill("cover-reposition", "Reposition", cx)
                            .on_click(cx.listener(|this, _, _, cx| this.start_reposition(cx))),
                    ))
                    .child(tagged(
                        "cover-remove",
                        pill("cover-remove", "Remove", cx)
                            .on_click(cx.listener(|this, _, _, cx| this.remove_cover(cx))),
                    ))
            });
        let spinner = generating.then(|| {
            div()
                .debug_selector(|| "cover-generating".into())
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(hsla(0., 0., 0., 0.25))
                .child(Spinner::new().color(theme::white()))
        });
        let reposition = self.cover.reposition.is_some().then(|| {
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .px(px(10.))
                        .py(px(5.))
                        .rounded(px(6.))
                        .bg(hsla(0., 0., 0., 0.45))
                        .text_size(px(12.))
                        .text_color(theme::white())
                        .child("Drag the image to reposition"),
                )
                .child(
                    div()
                        .absolute()
                        .right(px(12.))
                        .bottom(px(12.))
                        .flex()
                        .gap(px(6.))
                        .child(tagged(
                            "cover-save",
                            pill("cover-save", "Save position", cx)
                                .on_click(cx.listener(|this, _, _, cx| this.save_reposition(cx))),
                        ))
                        .child(tagged(
                            "cover-cancel",
                            pill("cover-cancel", "Cancel", cx).on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.cover.reposition = None;
                                    cx.notify();
                                },
                            )),
                        )),
                )
        });
        let banner = div()
            .id("object-cover")
            .debug_selector(|| "object-cover".into())
            .relative()
            .w_full()
            .h(px(if mobile { BANNER_H_PHONE } else { BANNER_H }))
            .flex_none()
            .overflow_hidden()
            .rounded(radius)
            .bg(theme::hover())
            .when(mobile, |d| d.mt(px(-24.)))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.cover.banner_hovered = *hovered;
                cx.notify();
            }))
            .when(self.cover.reposition.is_some(), |d| {
                d.cursor(if dragging {
                    CursorStyle::ClosedHand
                } else {
                    CursorStyle::OpenHand
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, e: &MouseDownEvent, _, cx| {
                        if let Some(r) = &mut this.cover.reposition {
                            r.drag = Some((e.position.y, r.y));
                            cx.notify();
                        }
                    }),
                )
                .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                    let overflow = this.cover.geometry.get().overflow();
                    if let Some(r) = &mut this.cover.reposition
                        && let Some((start, from)) = r.drag
                        && e.pressed_button == Some(MouseButton::Left)
                        && overflow > 0.
                    {
                        r.y = (from - f32::from(e.position.y - start) / overflow).clamp(0., 1.);
                        cx.notify();
                    }
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseUpEvent, _, cx| {
                        if let Some(r) = &mut this.cover.reposition {
                            r.drag = None;
                            cx.notify();
                        }
                    }),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseUpEvent, _, cx| {
                        if let Some(r) = &mut this.cover.reposition {
                            r.drag = None;
                            cx.notify();
                        }
                    }),
                )
            })
            .child(painter)
            .children(actions)
            .children(spinner)
            .children(reposition);
        Some(banner.into_any_element())
    }

    /// Hover tracking for the icon and title: a page without a cover offers Add cover there.
    pub(super) fn cover_head_hover(
        &self,
        cx: &mut Context<Self>,
    ) -> impl Fn(&bool, &mut Window, &mut App) + 'static {
        cx.listener(|this, hovered: &bool, _, cx| {
            if this.cover.head_hovered != *hovered {
                this.cover.head_hovered = *hovered;
                cx.notify();
            }
        })
    }

    /// Desktop, no cover, pointer over the icon and title: Add cover.
    pub(super) fn render_add_cover(
        &self,
        object: &Object,
        mobile: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let offer = !mobile
            && self.cover.head_hovered
            && self.cover.banner.is_none()
            && self.cover.chooser.is_none()
            && cover::can_have_cover(object);
        offer.then(|| {
            div()
                .absolute()
                .top(px(4.))
                .right_0()
                .child(tagged(
                    "cover-add",
                    Button::new("cover-add")
                        .ghost()
                        .h(px(26.))
                        .px(px(8.))
                        .text_color(theme::muted())
                        .accessibility_label("Add cover")
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .text_size(px(13.))
                                .child(Icon::new(Glyph::Plus).size(px(14.)))
                                .child("Add cover"),
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.open_chooser(cx))),
                ))
                .into_any_element()
        })
    }

    /// The chooser, and why the last Generate failed, under the title.
    pub(super) fn render_cover_cards(
        &mut self,
        object: &Object,
        mobile: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .when(mobile, |d| d.px(px(16.)))
            .children(self.render_chooser(object, cx))
            .children(self.render_failure(object, cx))
    }

    fn render_chooser(&self, object: &Object, cx: &mut Context<Self>) -> Option<Div> {
        let chooser = self.cover.chooser.as_ref()?;
        let title = if cover::cover_file(&object.fields).is_some() {
            "Change cover"
        } else {
            "Add cover"
        };
        let disabled = chooser.adding;
        Some(
            card("cover-chooser")
                .child(card_title(
                    title,
                    "cover-chooser-close",
                    cx.listener(|this, _, _, cx| {
                        this.cover.chooser = None;
                        cx.notify();
                    }),
                ))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(8.))
                        .when(roostr_editor::CAN_PICK_FILES, |d| {
                            d.child(tagged(
                                "cover-pick",
                                ui::disabled(
                                    ui::outline_button("cover-pick", "Choose an image…"),
                                    disabled,
                                )
                                .on_click(cx.listener(|this, _, _, cx| this.pick_cover(cx))),
                            ))
                        })
                        .child(tagged(
                            "cover-paste",
                            ui::disabled(
                                ui::outline_button("cover-paste", "Paste image"),
                                disabled,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.paste_cover(cx))),
                        ))
                        .child(tagged(
                            "cover-chooser-generate",
                            ui::disabled(
                                ui::outline_button("cover-chooser-generate", "Generate"),
                                disabled,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.generate(cx))),
                        )),
                )
                .when(chooser.adding, |d| d.child(small_note("Adding the image…")))
                .when(!chooser.error.is_empty(), |d| {
                    d.child(small_note(chooser.error.clone()).text_color(theme::red()))
                }),
        )
    }

    /// A failed Generate: OpenAI's (or the app's) words, the credential when
    /// the key is the trouble, and a close button.
    fn render_failure(&self, object: &Object, cx: &mut Context<Self>) -> Option<Div> {
        let failed = cover::generation(&object.fields)
            .filter(|g| g.status == Some(cover::Status::Failed));
        let error = if self.cover.ask_error.is_empty() {
            failed.as_ref().map(|g| {
                if g.error.is_empty() {
                    "The cover couldn't be made.".to_string()
                } else {
                    g.error.clone()
                }
            })?
        } else {
            self.cover.ask_error.clone()
        };
        let credential = failed
            .filter(|g| g.needs_key && !g.credential.is_empty())
            .map(|g| g.credential);
        Some(
            card("cover-failed")
                .child(card_title(
                    "Couldn't make a cover",
                    "cover-failed-close",
                    cx.listener(|this, _, _, cx| this.dismiss_failure(cx)),
                ))
                .child(
                    small_note(error)
                        .debug_selector(|| "cover-error".into())
                        .text_color(theme::red()),
                )
                .when_some(credential, |d, id| {
                    d.child(tagged(
                        "cover-open-credential",
                        ui::outline_button("cover-open-credential", "Open the OpenAI credential")
                            .on_click(move |_, _, cx| navigate(Route::Object(id.clone()), cx)),
                    ))
                }),
        )
    }
}

/// A bordered card in the page's flow.
fn card(id: &'static str) -> Div {
    div()
        .debug_selector(move || id.into())
        .flex()
        .flex_col()
        .gap(px(10.))
        .p(px(12.))
        .mb(px(12.))
        .rounded(px(10.))
        .border_1()
        .border_color(theme::border())
        .bg(theme::bg())
}

/// A card's heading with its close button.
fn card_title(
    title: &'static str,
    close: &'static str,
    on_close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::fg())
                .child(title),
        )
        .child(tagged(
            close,
            ui::icon_button(close, Glyph::BadgeX)
                .accessibility_label("Close")
                .tooltip("Close")
                .on_click(on_close),
        ))
}

/// 12px muted text that wraps.
fn small_note(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(12.))
        .line_height(relative(1.4))
        .text_color(theme::muted())
        .child(text.into())
}

#[cfg(test)]
mod tests {
    // Explicit imports: the kit's glob would shadow `#[test]` with GPUI's.
    use gpui_kit::{Bounds, point, px, size};

    use super::{Geometry, cover_bounds};

    #[test]
    fn the_cover_fills_the_banner_and_slides_with_its_focus() {
        let banner = Bounds {
            origin: point(px(10.), px(20.)),
            size: size(px(800.), px(200.)),
        };
        // A square image fills the width: 800 tall, 600 of it hidden.
        let square = size(100., 100.);
        let top = cover_bounds(banner, square, 0.);
        assert_eq!(top.size, size(px(800.), px(800.)));
        assert_eq!(top.origin, point(px(10.), px(20.)));
        assert_eq!(cover_bounds(banner, square, 1.).origin.y, px(20. - 600.));
        assert_eq!(cover_bounds(banner, square, 0.5).origin.y, px(20. - 300.));
        let g = Geometry {
            bounds: banner,
            image: square,
        };
        assert_eq!(g.overflow(), 600.);
        // A very wide image fills the height and centres sideways: nothing to drag.
        let wide = size(1000., 100.);
        let b = cover_bounds(banner, wide, 0.);
        assert_eq!(b.size, size(px(2000.), px(200.)));
        assert_eq!(b.origin.x, px(10. - 600.));
        let g = Geometry {
            bounds: banner,
            image: wide,
        };
        assert_eq!(g.overflow(), 0.);
    }
}
