//! Embed and bookmark blocks. Rune has no web view: an embed (YouTube,
//! Vimeo, Spotify) is a card with the player's still and title that opens
//! the URL in the browser, sized like the iframe (16:9, Spotify 152 px); a
//! bookmark is a favicon, title and URL card. Embed stills are fetched once
//! per URL into `Editor::posters` by [`Editor::ensure_posters`].

use std::collections::{BTreeMap, HashSet};

use roostr_core::State;
use roostr_ui::ty::Weights as _;
use roostr_ui::{Images, Theme, color, ty};
use rune::{Color, Cursor, El, SurfaceFit, Tx, col, el, row, text};

use crate::editor::{Editor, update_editor};
use crate::embed::{self, Poster, Processor};

impl Editor {
    /// Start fetching the poster of every embed block whose URL has none
    /// yet (each URL once; `None` in `posters` while the fetch runs). Call
    /// it after the blocks change (a load, a refresh, a commit): views only
    /// read `posters`.
    pub(crate) fn ensure_posters(&mut self, s: &mut State) {
        let mut wanted: Vec<(Processor, String)> = Vec::new();
        let mut seen = HashSet::new();
        for b in &self.blocks {
            let Some(custom) = &b.content.custom else {
                continue;
            };
            if custom.content_type != "embed" {
                continue;
            }
            let Some(meta) = &custom.meta else { continue };
            let Some(processor) = meta.get("processor").and_then(|p| Processor::from_key(p)) else {
                continue;
            };
            let url = meta.get("url").cloned().unwrap_or_default();
            if !self.posters.contains_key(&url) && seen.insert(url.clone()) {
                wanted.push((processor, url));
            }
        }
        for (processor, url) in wanted {
            self.poster(processor, url, s);
        }
    }

    /// Fetch the poster for an embed URL in the background (once per URL).
    pub(crate) fn poster(&mut self, processor: Processor, url: String, s: &mut State) {
        if self.posters.contains_key(&url) {
            return;
        }
        self.posters.insert(url.clone(), None);
        let oid = self.object_id.clone();
        let fetch_url = url.clone();
        let fetch =
            s.rt.background(async move { embed::fetch_poster(processor, &fetch_url).await });
        s.rt.spawn(async move |cx| {
            let poster = fetch.await;
            update_editor(&cx, &oid, |e, _| {
                e.posters.insert(url, Some(poster));
            });
        })
        .detach();
    }
}

/// Open `url` in the browser, logging a refused link.
fn open(url: &str, tx: &mut Tx) {
    if let Err(error) = tx.open_url(url) {
        log::warn!("could not open {url}: {error}");
    }
}

/// A remote image (a still, a favicon) once the app has fetched it.
fn remote_image(s: &State, url: &str, size: f32) -> Option<rune::Image> {
    s.get::<Images>()?.image(url, size)
}

/// The round ▶ on an embed card.
fn play(size: f32, bg: Color, fg: Color) -> El {
    row()
        .size(size, size)
        .radius(size / 2.)
        .fill(bg)
        .center()
        .child(text("▶").font(ty::ui(size * 0.4)).color(fg))
}

/// A web embed (YouTube, Vimeo, Spotify) as a card that opens the URL.
pub(crate) fn embed_view(
    s: &State,
    _cx: &mut rune::Cx<State>,
    t: &Theme,
    e: &Editor,
    meta: &BTreeMap<String, String>,
    block_id: &str,
) -> El {
    let url = meta.get("url").cloned().unwrap_or_default();
    let processor = meta.get("processor").and_then(|p| Processor::from_key(p));
    let poster: Poster = processor
        .and_then(|_| e.posters.get(&url).cloned().flatten())
        .unwrap_or_default();
    let label = processor.map_or("Embed", |p| p.label());
    let title = poster.title.clone().unwrap_or_else(|| url.clone());
    let open_url = url.clone();
    let card = el()
        .key(format!("embed-{block_id}"))
        .role("link")
        .grow(1.)
        .shrink()
        .min_w(0.)
        .my(4.)
        .radius(10.)
        .clip_rounded(10.)
        .cursor(Cursor::Pointer)
        .on_press_tx(move |_: &mut State, tx: &mut Tx| open(&open_url, tx));
    let black = Color::BLACK;
    if processor == Some(Processor::Spotify) {
        // Spotify's compact player: cover art, title, provider, play.
        let mut art = el()
            .size(120., 120.)
            .radius(8.)
            .clip_rounded(8.)
            .fill(t.hover);
        if let Some(image) = poster
            .thumb
            .as_deref()
            .and_then(|thumb| remote_image(s, thumb, 120.))
        {
            art = art.child(
                el().w_full()
                    .h_full()
                    .radius(8.)
                    .pixels_fit(image.pixels(), SurfaceFit::Fill),
            );
        }
        return card.child(
            row()
                .h(152.)
                .fill(t.panel)
                .items_center()
                .gap(16.)
                .padding(16.)
                .child(art)
                .child(
                    col()
                        .grow(1.)
                        .shrink()
                        .min_w(0.)
                        .h_full()
                        .justify_between()
                        .child(
                            col()
                                .gap(4.)
                                .child(text(title).font(ty::ui(15.).bold()).color(t.fg).truncate())
                                .child(text(label).font(ty::ui(13.)).color(t.muted)),
                        )
                        .child(row().justify_end().child(play(32., t.white, black))),
                ),
        );
    }
    let mut player = el().w_full().aspect_ratio(16. / 9.).fill(black);
    if let Some(image) = poster
        .thumb
        .as_deref()
        .and_then(|thumb| remote_image(s, thumb, 480.))
    {
        player = player.child(
            el().pin(Some(0.), Some(0.), Some(0.), Some(0.))
                .radius(10.)
                .pixels_fit(image.pixels(), SurfaceFit::Fill),
        );
    }
    let scrim = color::hsla(0., 0., 0., 0.6);
    player = player.child(
        col()
            .pin(Some(0.), Some(0.), Some(0.), Some(0.))
            .center()
            .gap(8.)
            .child(play(56., scrim, t.white))
            .child(
                // `max-width: 90%`: the pill shrinks to its text within it.
                row().w_frac(0.9).justify_center().child(
                    el().shrink()
                        .min_w(0.)
                        .px(10.)
                        .py(4.)
                        .radius(6.)
                        .fill(scrim)
                        .child(
                            text(format!("{label} · {title}"))
                                .font(ty::ui(12.))
                                .color(t.white)
                                .truncate(),
                        ),
                ),
            ),
    );
    card.child(player)
}

/// A bookmark card: favicon, title (else host), URL; opens in the browser.
pub(crate) fn bookmark_view(
    s: &State,
    _cx: &mut rune::Cx<State>,
    t: &Theme,
    meta: &BTreeMap<String, String>,
    block_id: &str,
) -> El {
    let url = meta.get("url").cloned().unwrap_or_default();
    let host = url::Url::parse(&url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| url.clone());
    let title = meta
        .get("title")
        .cloned()
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| host.clone());
    let favicon = format!("https://www.google.com/s2/favicons?domain={host}&sz=32");
    let mut icon = el().size(20., 20.).radius(4.).clip_rounded(4.);
    if let Some(image) = remote_image(s, &favicon, 20.) {
        icon = icon.pixels_fit(image.pixels(), SurfaceFit::Fit);
    }
    let open_url = url.clone();
    row()
        .key(format!("bookmark-{block_id}"))
        .role("link")
        .grow(1.)
        .shrink()
        .min_w(0.)
        .items_center()
        .gap(10.)
        .border(1., t.border)
        .border_layout()
        .radius(10.)
        .px(12.)
        .py(10.)
        .my(4.)
        .cursor(Cursor::Pointer)
        .hover(t.hover)
        .on_press_tx(move |_: &mut State, tx: &mut Tx| open(&open_url, tx))
        .child(icon)
        .child(
            col()
                .min_w(0.)
                .shrink()
                .child(
                    text(title)
                        .font(ty::ui(13.).weight(550.))
                        .color(t.fg)
                        .wrap(),
                )
                .child(text(url).font(ty::ui(11.)).color(t.muted).truncate()),
        )
}
