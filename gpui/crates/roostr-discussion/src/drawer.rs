//! The right "Chat & properties" pane (`ConversationDrawer.svelte`): a
//! Chat / Properties tab switch, the collapse button, and the active tab.

use std::time::Duration;

use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use roostr_core::objects::type_icon;
use roostr_core::{PaneState, Store, theme};
use roostr_properties::PropertiesPane;

use crate::discussion::{Discussion, DiscussionOptions};

/// Refetch of the open object while the pane is up (`refreshActive`).
const REFRESH_EVERY: Duration = Duration::from_secs(8);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Chat,
    Props,
}

pub struct ConversationDrawer {
    object_id: String,
    /// False for objects without a discussion (agents, types, sets): properties only.
    chat: bool,
    picked: Option<Tab>,
    discussion: Option<Entity<Discussion>>,
    properties: Option<Entity<PropertiesPane>>,
    refresh_error: String,
    refreshing: bool,
    _timer: Task<()>,
    _keys: Subscription,
}

impl ConversationDrawer {
    /// The pane for `object_id`; `chat` false shows Properties only.
    pub fn new(object_id: String, chat: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Store::global(cx).update(cx, |s, cx| s.ensure_object(&object_id, cx));
        let timer = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(REFRESH_EVERY).await;
                if this.update(cx, |this, cx| this.refresh_active(cx)).is_err() {
                    return;
                }
            }
        });
        // Escape anywhere that nothing else handled: back to Chat, else collapse.
        let keys = cx.observe_keystrokes(|this, event, _, cx| {
            if event.keystroke.key == "escape"
                && event.action.is_none()
                && !event.keystroke.modifiers.modified()
            {
                if this.chat && this.tab() == Tab::Props {
                    this.picked = Some(Tab::Chat);
                    cx.notify();
                } else {
                    PaneState::global(cx).update(cx, |p, cx| p.set_open(false, cx));
                }
            }
        });
        let mut this = Self {
            object_id,
            chat,
            picked: None,
            discussion: None,
            properties: None,
            refresh_error: String::new(),
            refreshing: false,
            _timer: timer,
            _keys: keys,
        };
        this.ensure_tab_view(window, cx);
        this
    }

    /// Chat when the object has one and nothing else was picked; never a Chat tab without a chat.
    fn tab(&self) -> Tab {
        match self.picked {
            Some(Tab::Chat) if !self.chat => Tab::Props,
            Some(tab) => tab,
            None if self.chat => Tab::Chat,
            None => Tab::Props,
        }
    }

    /// The active tab's view exists; the other one is dropped (the web
    /// unmounts it, so a returning chat lands at its newest message).
    fn ensure_tab_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.tab() {
            Tab::Chat => {
                self.properties = None;
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
            Tab::Props => {
                self.discussion = None;
                if self.properties.is_none() {
                    let id = self.object_id.clone();
                    self.properties = Some(cx.new(|cx| PropertiesPane::new(id, window, cx)));
                }
            }
        }
    }

    fn pick(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        self.picked = Some(tab);
        self.ensure_tab_view(window, cx);
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

    fn render_tab(
        &self,
        tab: Tab,
        icon: String,
        label: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let active = self.tab() == tab;
        div()
            .id(label)
            .debug_selector(move || format!("pane-tab-{label}"))
            .flex()
            .items_center()
            .gap(px(5.))
            .rounded(px(7.))
            .px(px(10.))
            .py(px(4.))
            .text_size(px(12.5))
            .font_weight(FontWeight::SEMIBOLD)
            .cursor_pointer()
            .text_color(if active { theme::fg() } else { theme::muted() })
            .when(active, |d| d.bg(theme::hover()))
            .hover(|s| s.text_color(theme::fg()))
            .on_click(cx.listener(move |this, _, window, cx| this.pick(tab, window, cx)))
            .child(div().text_size(px(13.)).child(icon))
            .child(label)
    }
}

impl Render for ConversationDrawer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Keep the views in step if `chat` changed under a picked Chat tab.
        if (self.tab() == Tab::Chat && self.discussion.is_none())
            || (self.tab() == Tab::Props && self.properties.is_none())
        {
            self.ensure_tab_view(window, cx);
        }
        let space = roostr_core::nav::Navigator::global(cx)
            .read(cx)
            .space()
            .to_string();
        let chat_icon = {
            let store = Store::global(cx);
            let icon = type_icon(store.read(cx), "chat", &space);
            if icon.is_empty() {
                "•".to_string()
            } else {
                icon
            }
        };
        let tabs = div()
            .flex()
            .flex_none()
            .gap(px(2.))
            .bg(theme::panel())
            .border_1()
            .border_color(theme::border())
            .rounded(px(9.))
            .p(px(2.))
            .when(self.chat, |d| {
                d.child(self.render_tab(Tab::Chat, chat_icon, "Chat", cx))
            })
            .child(self.render_tab(Tab::Props, "🧩".into(), "Properties", cx));
        let close = div()
            .id("collapse")
            .flex_none()
            .size(px(26.))
            .flex()
            .items_center()
            .justify_center()
            .border_1()
            .border_color(theme::border())
            .rounded(px(7.))
            .text_size(px(14.))
            .text_color(theme::muted())
            .cursor_pointer()
            .hover(|s| s.text_color(theme::fg()).border_color(theme::muted()))
            .tooltip(|window, cx| Tooltip::new("Collapse").build(window, cx))
            .on_click(|_, _, cx| PaneState::global(cx).update(cx, |p, cx| p.set_open(false, cx)))
            .child("»");
        // The divider is inset like the object header's, not edge to edge.
        let head = div()
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(10.))
            .px(px(14.))
            .py(px(12.))
            .child(tabs)
            .child(close)
            .child(
                div()
                    .absolute()
                    .left(px(14.))
                    .right(px(14.))
                    .bottom_0()
                    .h(px(1.))
                    .bg(theme::border()),
            );

        let error = (!self.refresh_error.is_empty()).then(|| {
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(4.))
                .px(px(14.))
                .my(px(12.))
                .text_size(px(12.))
                .text_color(theme::orange())
                .child(self.refresh_error.clone())
                .child(
                    div()
                        .id("refresh")
                        .px(px(6.))
                        .py(px(1.))
                        .border_1()
                        .border_color(theme::border())
                        .rounded(px(4.))
                        .text_color(theme::fg())
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| this.refresh_active(cx)))
                        .child("Refresh"),
                )
        });

        let body: AnyElement = match (self.tab(), &self.discussion, &self.properties) {
            (Tab::Chat, Some(discussion), _) => div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(discussion.clone())
                .into_any_element(),
            (Tab::Props, _, Some(properties)) => div()
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
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .text_color(theme::fg())
            .child(head)
            .children(error)
            .child(body)
    }
}
