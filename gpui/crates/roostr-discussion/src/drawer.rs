//! The right "Chat & properties" pane (`ConversationDrawer.svelte`). On a
//! desktop the shell lays it out as two columns, Chat then Properties
//! (`PaneColumn`), each with its name and collapse button; on a phone it is
//! one sheet with a Chat / Properties switch, the collapse button and the
//! active tab.

use std::time::Duration;

use gpui_kit::component::Sizable as _;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::button::Button;
use gpui_kit::component::label::Label;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::layout::is_mobile;
use roostr_core::nav::Route;
use roostr_core::{Navigator, PaneState, PaneTab, PaneViews, Store, theme};
use roostr_properties::PropertiesPane;

use crate::discussion::{Discussion, DiscussionOptions};

/// Refetch of the open object while the pane is up (`refreshActive`).
const REFRESH_EVERY: Duration = Duration::from_secs(8);

pub struct ConversationDrawer {
    object_id: String,
    /// False for objects without a discussion (agents, types, sets): properties only.
    chat: bool,
    /// The sheet's picked tab.
    picked: Option<PaneTab>,
    /// Last drawn as the phone's sheet (else as desktop columns).
    in_sheet: bool,
    discussion: Option<Entity<Discussion>>,
    properties: Option<Entity<PropertiesPane>>,
    /// The desktop columns (Chat only with a chat), made when first published.
    columns: Option<(Option<Entity<PaneColumn>>, Entity<PaneColumn>)>,
    refresh_error: String,
    refreshing: bool,
    _timer: Task<()>,
    _subscriptions: [Subscription; 2],
}

impl ConversationDrawer {
    /// The pane for `object_id`; `chat` false shows Properties only.
    pub fn new(object_id: String, chat: bool, _: &mut Window, cx: &mut Context<Self>) -> Self {
        Store::global(cx).update(cx, |s, cx| s.ensure_object(&object_id, cx));
        let timer = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(REFRESH_EVERY).await;
                if this.update(cx, |this, cx| this.refresh_active(cx)).is_err() {
                    return;
                }
            }
        });
        // Escape anywhere that nothing else handled, on this object's page.
        // Sheet: back to Chat, else close. Desktop: collapse a column.
        let keys = cx.observe_keystrokes(|this, event, window, cx| {
            if event.keystroke.key != "escape"
                || event.action.is_some()
                || event.keystroke.modifiers.modified()
                || !this.is_shown(cx)
            {
                return;
            }
            if !is_mobile(window) {
                PaneState::global(cx).update(cx, |p, cx| p.collapse_last(cx));
            } else if this.chat && this.tab() == PaneTab::Properties {
                this.picked = Some(PaneTab::Chat);
                cx.notify();
            } else {
                PaneState::global(cx).update(cx, |p, cx| p.set_sheet(false, cx));
            }
        });
        // A collapsed column's view goes (the web unmounts it, so a returning
        // chat lands at its newest message).
        let pane = cx.observe(&PaneState::global(cx), |this, _, cx| {
            if !this.in_sheet {
                this.drop_collapsed(cx);
            }
        });
        Self {
            object_id,
            chat,
            picked: None,
            in_sheet: false,
            discussion: None,
            properties: None,
            columns: None,
            refresh_error: String::new(),
            refreshing: false,
            _timer: timer,
            _subscriptions: [keys, pane],
        }
    }

    /// What the shell shows for this pane: this view as the phone's sheet,
    /// a view per desktop column.
    pub fn views(this: &Entity<Self>, cx: &mut App) -> PaneViews {
        let (chat, columns) = {
            let drawer = this.read(cx);
            (drawer.chat, drawer.columns.clone())
        };
        let (chat_column, properties) = columns.unwrap_or_else(|| {
            let columns = (
                chat.then(|| cx.new(|cx| PaneColumn::new(this, PaneTab::Chat, cx))),
                cx.new(|cx| PaneColumn::new(this, PaneTab::Properties, cx)),
            );
            this.update(cx, |drawer, _| drawer.columns = Some(columns.clone()));
            columns
        });
        PaneViews {
            sheet: this.clone().into(),
            chat: chat_column.map(Into::into),
            properties: properties.into(),
        }
    }

    /// This object's page is the one showing.
    fn is_shown(&self, cx: &App) -> bool {
        matches!(Navigator::global(cx).read(cx).route(), Route::Object(id) if *id == self.object_id)
    }

    /// The sheet's tab: Chat when the object has one and nothing else was
    /// picked; never a Chat tab without a chat.
    fn tab(&self) -> PaneTab {
        match self.picked {
            Some(PaneTab::Chat) if !self.chat => PaneTab::Properties,
            Some(tab) => tab,
            None if self.chat => PaneTab::Chat,
            None => PaneTab::Properties,
        }
    }

    fn ensure_view(&mut self, tab: PaneTab, window: &mut Window, cx: &mut Context<Self>) {
        match tab {
            PaneTab::Chat => {
                if self.discussion.is_none() {
                    let id = self.object_id.clone();
                    self.discussion = Some(cx.new(|cx| {
                        Discussion::new(
                            id,
                            DiscussionOptions {
                                full: true,
                                page_mode: false,
                                thread_id: crate::chat::DISCUSSION.into(),
                                embedded: true,
                            },
                            window,
                            cx,
                        )
                    }));
                }
            }
            PaneTab::Properties => {
                if self.properties.is_none() {
                    let id = self.object_id.clone();
                    self.properties = Some(cx.new(|cx| PropertiesPane::new(id, window, cx)));
                }
            }
        }
    }

    fn drop_view(&mut self, tab: PaneTab) {
        match tab {
            PaneTab::Chat => self.discussion = None,
            PaneTab::Properties => self.properties = None,
        }
    }

    /// Desktop: no view for a collapsed column.
    fn drop_collapsed(&mut self, cx: &App) {
        let pane = PaneState::global(cx).read(cx);
        if !pane.chat_open {
            self.discussion = None;
        }
        if !pane.props_open {
            self.properties = None;
        }
    }

    fn pick(&mut self, tab: PaneTab, cx: &mut Context<Self>) {
        self.picked = Some(tab);
        cx.notify();
    }

    /// Refetch the object; a failure shows above the tab with a Refresh button.
    fn refresh_active(&mut self, cx: &mut Context<Self>) {
        if self.refreshing {
            return;
        }
        self.refreshing = true;
        let store = Store::global(cx);
        let task = store.read(cx).fetch_object(&self.object_id, cx);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.refreshing = false;
                match result {
                    Ok(fresh) => {
                        this.refresh_error.clear();
                        let store = Store::global(cx);
                        let stale = store
                            .read(cx)
                            .object(&this.object_id)
                            .is_none_or(|cached| *cached != fresh);
                        if stale {
                            store.update(cx, |s, cx| s.reload_object(&this.object_id, cx));
                        }
                    }
                    Err(error) => this.refresh_error = error.to_string(),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// A Chat / Properties tab of the sheet's switch: its icon, then its name.
    fn render_tab(tab: PaneTab, cx: &App) -> Tab {
        let label = tab.name();
        Tab::new()
            .aria_label(label)
            .debug_selector(move || format!("pane-tab-{label}"))
            .child(div().mr(px(5.)).text_size(px(13.)).child(tab.icon(cx)))
            .child(
                div()
                    .text_size(px(12.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(label),
            )
    }

    /// The `»` button: closes the sheet, or collapses a desktop column.
    fn render_collapse(&self, column: Option<PaneTab>) -> impl IntoElement {
        let (id, tip): (SharedString, &'static str) = match column {
            None => ("collapse".into(), "Collapse"),
            Some(PaneTab::Chat) => ("pane-collapse-Chat".into(), "Collapse chat"),
            Some(PaneTab::Properties) => ("pane-collapse-Properties".into(), "Collapse properties"),
        };
        let selector = id.clone();
        Button::new(ElementId::Name(id))
            .debug_selector(move || selector.to_string())
            .outline()
            .small()
            .flex_none()
            .size(px(26.))
            .p_0()
            .rounded(px(7.))
            .text_color(theme::muted())
            .tooltip(tip)
            .on_click(move |_, _, cx| {
                PaneState::global(cx).update(cx, |p, cx| match column {
                    None => p.set_sheet(false, cx),
                    Some(tab) => p.set_open(tab, false, cx),
                })
            })
            .child(div().text_size(px(14.)).child("»"))
    }

    /// A collapsed column folded into the open one's header: its icon and
    /// name, sliding in from the right as the column slides shut; a click
    /// expands it again.
    fn render_folded(tab: PaneTab, cx: &App) -> impl IntoElement {
        let name = tab.name();
        Button::new(ElementId::Name(format!("pane-folded-{name}").into()))
            .debug_selector(move || format!("pane-folded-{name}"))
            .outline()
            .small()
            .flex_none()
            .h(px(26.))
            .px(px(8.))
            .rounded(px(7.))
            .text_color(theme::muted())
            .tooltip(format!("Show {}", name.to_lowercase()))
            .on_click(move |_, _, cx| {
                PaneState::global(cx).update(cx, |p, cx| p.set_open(tab, true, cx))
            })
            .child(div().text_size(px(12.)).child(tab.icon(cx)))
            .child(div().text_size(px(12.)).child(name))
            .with_animation(
                ElementId::Name(format!("pane-fold-in-{name}").into()),
                Animation::new(Duration::from_millis(220)).with_easing(ease_out_quint()),
                |d, t| d.opacity(t).ml(px(24. * (1. - t))),
            )
    }

    /// A header row; the divider is inset like the object header's, not edge to edge.
    fn render_head(children: impl IntoIterator<Item = AnyElement>) -> Div {
        div()
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(10.))
            .h(px(theme::HEADER_H))
            .px(px(14.))
            .pt(px(18.))
            .pb(px(8.))
            .children(children)
            .child(
                div()
                    .absolute()
                    .left(px(14.))
                    .right(px(14.))
                    .bottom_0()
                    .h(px(1.))
                    .bg(theme::border()),
            )
    }

    fn render_error(&self, cx: &mut Context<Self>) -> Option<Div> {
        (!self.refresh_error.is_empty()).then(|| {
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .px(px(14.))
                .my(px(12.))
                .child(
                    Alert::warning("refresh-error", self.refresh_error.clone())
                        .xsmall()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(12.))
                        .text_color(theme::orange()),
                )
                .child(
                    Button::new("refresh")
                        .outline()
                        .xsmall()
                        .label("Refresh")
                        .on_click(cx.listener(|this, _, _, cx| this.refresh_active(cx))),
                )
        })
    }

    fn render_body(&self, tab: PaneTab) -> AnyElement {
        match (tab, &self.discussion, &self.properties) {
            (PaneTab::Chat, Some(discussion), _) => div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(discussion.clone())
                .into_any_element(),
            (PaneTab::Properties, _, Some(properties)) => div()
                .id("props")
                .flex_1()
                .min_h_0()
                .overflow_x_hidden()
                .overflow_y_scroll()
                .px(px(14.))
                .py(px(8.))
                .child(properties.clone())
                .into_any_element(),
            _ => div().into_any_element(),
        }
    }

    /// A desktop column: its icon and name, its collapse button, its view.
    /// A refresh failure shows in the first expanded column.
    fn render_column(
        &mut self,
        tab: PaneTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.in_sheet = false;
        self.drop_collapsed(cx);
        self.ensure_view(tab, window, cx);
        let (first, folded) = {
            let pane = PaneState::global(cx).read(cx);
            let other = tab.other();
            (
                tab == PaneTab::Properties || !pane.is_open(PaneTab::Properties),
                pane.has(other) && !pane.is_open(other),
            )
        };
        let name = tab.name();
        let mut head = vec![
            div()
                .flex_none()
                .text_size(px(13.))
                .child(tab.icon(cx))
                .into_any_element(),
            Label::new(name)
                .flex_1()
                .min_w_0()
                .text_size(px(12.5))
                .font_weight(FontWeight::SEMIBOLD)
                .into_any_element(),
        ];
        // The other column, collapsed, folds into this header: a click opens it again.
        if folded {
            head.push(Self::render_folded(tab.other(), cx).into_any_element());
        }
        head.push(self.render_collapse(Some(tab)).into_any_element());
        let head = Self::render_head(head);
        div()
            .debug_selector(move || format!("pane-col-{name}"))
            .size_full()
            .flex()
            .flex_col()
            .text_color(theme::fg())
            .child(head)
            .when(first, |d| d.children(self.render_error(cx)))
            .child(self.render_body(tab))
            .into_any_element()
    }
}

/// The phone's sheet.
impl Render for ConversationDrawer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.in_sheet = true;
        let tab = self.tab();
        self.ensure_view(tab, window, cx);
        self.drop_view(tab.other());
        // The Chat / Properties switch: the kit's segmented tab bar.
        let shown: Vec<PaneTab> = if self.chat {
            vec![PaneTab::Chat, PaneTab::Properties]
        } else {
            vec![PaneTab::Properties]
        };
        let selected = shown.iter().position(|t| *t == tab).unwrap_or(0);
        let tabs = TabBar::new("pane-tabs")
            .segmented()
            .small()
            .flex_none()
            .bg(theme::panel())
            .border_1()
            .border_color(theme::border())
            .rounded(px(9.))
            .p(px(2.))
            .children(shown.iter().map(|t| Self::render_tab(*t, cx)))
            .selected_index(selected)
            .on_click(cx.listener(move |this, ix: &usize, _, cx| {
                if let Some(tab) = shown.get(*ix) {
                    this.pick(*tab, cx);
                }
            }));
        let head = Self::render_head([
            tabs.into_any_element(),
            self.render_collapse(None).into_any_element(),
        ]);
        div()
            .size_full()
            .flex()
            .flex_col()
            .text_color(theme::fg())
            .child(head)
            .children(self.render_error(cx))
            .child(self.render_body(tab))
    }
}

/// One desktop column of a [`ConversationDrawer`].
struct PaneColumn {
    drawer: WeakEntity<ConversationDrawer>,
    tab: PaneTab,
    _drawer_changed: Subscription,
}

impl PaneColumn {
    fn new(drawer: &Entity<ConversationDrawer>, tab: PaneTab, cx: &mut Context<Self>) -> Self {
        Self {
            drawer: drawer.downgrade(),
            tab,
            _drawer_changed: cx.observe(drawer, |_, _, cx| cx.notify()),
        }
    }
}

impl Render for PaneColumn {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = self.tab;
        self.drawer
            .update(cx, |drawer, cx| drawer.render_column(tab, window, cx))
            .unwrap_or_else(|_| div().into_any_element())
    }
}
