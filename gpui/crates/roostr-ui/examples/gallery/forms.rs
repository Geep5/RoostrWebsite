//! Gallery section: forms (inputs, textareas, checkboxes, switches, selects, list rows and the
//! command palette).

use roostr_ui::{
    Checkbox, Command, CommandGroup, CommandItem, CommandState, Glyph, IndexPath, Input,
    InputState, KitSize, ListItem, Select, SelectItem, SelectState, Switch, Textarea,
    TextareaState, Theme, checkbox, checkbox_display, glyph,
};
use rune::{El, col, row, text};

use crate::{Gallery, labeled, section};

pub struct State {
    empty: InputState,
    filled: InputState,
    focused: InputState,
    disabled: InputState,
    clean: InputState,
    plain: InputState,
    password: InputState,
    small: InputState,
    large: InputState,
    note: TextareaState,
    submitted: Option<String>,
    checks: [bool; 4],
    done: bool,
    switches: [bool; 4],
    fruit: SelectState,
    search: SelectState,
    shown: SelectState,
    picked: Option<String>,
    rows: usize,
    palette: CommandState,
    shown_palette: CommandState,
    command: Option<IndexPath>,
}

fn fruits() -> Vec<SelectItem> {
    vec![
        SelectItem::new("apple", "Apple").group("Fruit"),
        SelectItem::new("banana", "Banana").group("Fruit"),
        SelectItem::new("cherry", "Cherry").group("Fruit"),
        SelectItem::new("kale", "Kale").group("Leaves"),
        SelectItem::new("leek", "Leek")
            .group("Leaves")
            .disabled(true),
    ]
}

impl Default for State {
    fn default() -> Self {
        let mut focused = InputState::with_value("Focused");
        focused.focus();
        let mut shown = SelectState::new(fruits(), Some(1));
        shown.move_cursor(true);
        let mut shown_palette = CommandState::new();
        shown_palette.set_query("pa");
        Self {
            empty: InputState::new(),
            filled: InputState::with_value("Weekly review"),
            focused,
            disabled: InputState::with_value("Can't touch this"),
            clean: InputState::with_value("Clear me"),
            plain: InputState::new(),
            password: InputState::with_value("hunter2").masked(true),
            small: InputState::with_value("Small"),
            large: InputState::with_value("Large"),
            note: TextareaState::with_value("A note that grows\nwith its text.")
                .auto_grow(2, 5)
                .submit_on_enter(true),
            submitted: None,
            checks: [true, false, true, false],
            done: true,
            switches: [true, false, true, false],
            fruit: SelectState::new(fruits(), None),
            search: SelectState::new(fruits(), Some(3)).searchable(true),
            shown,
            picked: None,
            rows: 0,
            palette: CommandState::new(),
            shown_palette,
            command: None,
        }
    }
}

fn line(children: impl IntoIterator<Item = El>) -> El {
    row().gap(12.0).items_start().flex_wrap().children(children)
}

fn field(t: &Theme, label: &str, e: El) -> El {
    labeled(t, label, col().w(200.0).child(e))
}

fn inputs(s: &Gallery, t: &Theme) -> El {
    let f = &s.forms;
    line([
        field(
            t,
            "placeholder",
            Input::new("in-empty", &f.empty)
                .placeholder("Property name")
                .on_edit(|s: &mut Gallery| &mut s.forms.empty)
                .build(t),
        ),
        field(
            t,
            "filled · Enter submits",
            Input::new("in-filled", &f.filled)
                .on_edit(|s: &mut Gallery| &mut s.forms.filled)
                .on_submit(|s: &mut Gallery, _| {
                    s.forms.submitted = Some(s.forms.filled.value().to_owned())
                })
                .build(t),
        ),
        field(
            t,
            "focused",
            Input::new("in-focused", &f.focused)
                .on_edit(|s: &mut Gallery| &mut s.forms.focused)
                .build(t),
        ),
        field(
            t,
            "disabled",
            Input::new("in-disabled", &f.disabled)
                .disabled(true)
                .on_edit(|s: &mut Gallery| &mut s.forms.disabled)
                .build(t),
        ),
        field(
            t,
            "cleanable",
            Input::new("in-clean", &f.clean)
                .cleanable(true)
                .on_edit(|s: &mut Gallery| &mut s.forms.clean)
                .build(t),
        ),
        field(
            t,
            "appearance(false) + prefix",
            Input::new("in-plain", &f.plain)
                .appearance(false)
                .placeholder("Search…")
                .prefix(glyph(Glyph::KitSearch, 14.0).color(t.kit.muted_foreground))
                .on_edit(|s: &mut Gallery| &mut s.forms.plain)
                .build(t),
        ),
        field(
            t,
            "masked + mask_toggle",
            Input::new("in-password", &f.password)
                .mask_toggle()
                .on_edit(|s: &mut Gallery| &mut s.forms.password)
                .build(t),
        ),
        field(
            t,
            "small",
            Input::new("in-small", &f.small)
                .small()
                .on_edit(|s: &mut Gallery| &mut s.forms.small)
                .build(t),
        ),
        field(
            t,
            "large",
            Input::new("in-large", &f.large)
                .large()
                .on_edit(|s: &mut Gallery| &mut s.forms.large)
                .build(t),
        ),
        field(
            t,
            "textarea · auto_grow(2, 5), Enter submits",
            Textarea::new("in-note", &f.note)
                .placeholder("Describe the cover…")
                .on_edit(|s: &mut Gallery| &mut s.forms.note)
                .on_submit(|s: &mut Gallery, _| {
                    s.forms.submitted = Some(s.forms.note.value().to_owned())
                })
                .build(t),
        ),
        field(
            t,
            "submitted",
            text(f.submitted.clone().unwrap_or_else(|| "—".into()))
                .font(roostr_ui::ty::ui(12.0))
                .color(t.muted),
        ),
    ])
}

fn checkboxes(s: &Gallery, t: &Theme) -> El {
    let f = &s.forms;
    let sizes = [
        ("xsmall", KitSize::XSmall),
        ("small", KitSize::Small),
        ("medium", KitSize::Medium),
        ("large", KitSize::Large),
    ];
    let mut by_size = row().gap(16.0).items_start();
    for (i, (name, size)) in sizes.into_iter().enumerate() {
        by_size = by_size.child(
            Checkbox::new(format!("cb-{name}"))
                .label(name)
                .size(size)
                .checked(f.checks[i])
                .on_change(move |s: &mut Gallery, v| s.forms.checks[i] = v)
                .build(t),
        );
    }
    let states = row()
        .gap(16.0)
        .items_start()
        .child(
            Checkbox::new("cb-off")
                .label("Disabled")
                .disabled(true)
                .build(t),
        )
        .child(
            Checkbox::new("cb-on")
                .label("Disabled, checked")
                .checked(true)
                .disabled(true)
                .build(t),
        )
        .child(
            Checkbox::new("cb-tip")
                .label("Ask agents to respond")
                .xsmall()
                .checked(f.checks[0])
                .tooltip("Request a reply")
                .on_change(|s: &mut Gallery, v| s.forms.checks[0] = v)
                .build(t),
        );
    let roostr = row()
        .gap(16.0)
        .center_y()
        .child(labeled(
            t,
            "checkbox()",
            checkbox("cb-roostr", f.done)
                .on_change(|s: &mut Gallery, v| s.forms.done = v)
                .build(t),
        ))
        .child(labeled(
            t,
            "checkbox_display()",
            checkbox_display("cb-display", f.done).build(t),
        ))
        .child(labeled(
            t,
            "unchecked",
            checkbox_display("cb-display-off", false).build(t),
        ));
    col().gap(12.0).child(by_size).child(states).child(roostr)
}

fn switches(s: &Gallery, t: &Theme) -> El {
    let f = &s.forms;
    let sizes = [
        ("small", KitSize::Small),
        ("medium", KitSize::Medium),
        ("large", KitSize::Large),
    ];
    let mut r = row().gap(16.0).items_start();
    for (i, (name, size)) in sizes.into_iter().enumerate() {
        r = r.child(
            Switch::new(format!("sw-{name}"))
                .label(name)
                .size(size)
                .checked(f.switches[i])
                .on_change(move |s: &mut Gallery, v| s.forms.switches[i] = v)
                .build(t),
        );
    }
    r.child(
        Switch::new("sw-off")
            .label("Disabled")
            .disabled(true)
            .build(t),
    )
    .child(
        Switch::new("sw-on")
            .label("Disabled, on")
            .checked(true)
            .disabled(true)
            .build(t),
    )
    .child(
        Switch::new("sw-color")
            .label("Colour")
            .small()
            .color(t.green)
            .checked(f.switches[3])
            .on_change(|s: &mut Gallery, v| s.forms.switches[3] = v)
            .build(t),
    )
}

fn selects(s: &Gallery, t: &Theme, layer: &mut Vec<El>) -> El {
    let f = &s.forms;
    let closed = SelectState::new(fruits(), Some(0));
    let empty = SelectState::new(fruits(), None);
    let mut open = Select::new("sel-shown", &f.shown)
        .menu_width(200.0)
        .on_state(|s: &mut Gallery| &mut s.forms.shown);
    let mut search_open =
        Select::new("sel-shown-search", &f.shown.clone().searchable(true)).menu_width(200.0);
    line([
        field(
            t,
            "closed · placeholder",
            Select::new("sel-empty", &empty).build(t, layer),
        ),
        field(
            t,
            "closed · chosen",
            Select::new("sel-closed", &closed).build(t, layer),
        ),
        field(
            t,
            "disabled",
            Select::new("sel-disabled", &closed)
                .disabled(true)
                .build(t, layer),
        ),
        field(
            t,
            "cleanable",
            Select::new("sel-clean", &closed)
                .cleanable(true)
                .build(t, layer),
        ),
        field(
            t,
            "live",
            Select::new("sel-live", &f.fruit)
                .on_state(|s: &mut Gallery| &mut s.forms.fruit)
                .on_change(|s: &mut Gallery, v: Option<&str>| s.forms.picked = v.map(str::to_owned))
                .build(t, layer),
        ),
        field(
            t,
            "live · searchable",
            Select::new("sel-search", &f.search)
                .search_placeholder("Search fruit")
                .on_state(|s: &mut Gallery| &mut s.forms.search)
                .on_change(|s: &mut Gallery, v: Option<&str>| s.forms.picked = v.map(str::to_owned))
                .build(t, layer),
        ),
        labeled(t, "open surface", open.build_surface(t)),
        labeled(t, "open surface · searchable", search_open.build_surface(t)),
        field(
            t,
            "picked",
            text(f.picked.clone().unwrap_or_else(|| "—".into()))
                .font(roostr_ui::ty::ui(12.0))
                .color(t.muted),
        ),
    ])
}

fn list_items(s: &Gallery, t: &Theme) -> El {
    let rows = col()
        .w(220.0)
        .gap(2.0)
        .child(
            ListItem::new("li-plain")
                .label(format!("Pressed {} times", s.forms.rows))
                .on_press(|s: &mut Gallery| s.forms.rows += 1)
                .build(t),
        )
        .child(
            ListItem::new("li-selected")
                .label("Selected")
                .selected(true)
                .build(t),
        )
        .child(
            ListItem::new("li-check")
                .label("Confirmed")
                .check_icon(Glyph::Check)
                .confirmed(true)
                .build(t),
        )
        .child(
            ListItem::new("li-secondary")
                .label("Right-clicked")
                .secondary_selected(true)
                .build(t),
        )
        .child(
            ListItem::new("li-disabled")
                .label("Disabled")
                .disabled(true)
                .build(t),
        )
        .child(
            ListItem::new("li-roostr")
                .label("vrow")
                .radius(6.0)
                .px(8.0)
                .py(6.0)
                .text_size(13.0)
                .pointer()
                .suffix(text("⌘K").font(roostr_ui::ty::ui(11.0)).color(t.muted))
                .build(t),
        );
    rows
}

fn palette(state: &CommandState, key: &str, t: &Theme) -> Command {
    Command::new(key, state)
        .placeholder("Search types…")
        .group(
            CommandGroup::new()
                .label("Types")
                .item(CommandItem::new().label("Page").icon(Glyph::FileText))
                .item(
                    CommandItem::new()
                        .label("Person")
                        .icon(Glyph::User)
                        .keywords(["contact"]),
                )
                .item(
                    CommandItem::new()
                        .label("Project")
                        .icon(Glyph::BookOpen)
                        .disabled(true),
                ),
        )
        .separator()
        .group(
            CommandGroup::new()
                .label("Actions")
                .item(CommandItem::new().label("New page").checked(true))
                .item(
                    CommandItem::new().label("Paste as page").child(
                        row()
                            .gap(8.0)
                            .center_y()
                            .child(glyph(Glyph::KitCopy, 14.0).color(t.kit.muted_foreground))
                            .child(
                                text("Paste as page")
                                    .font(roostr_ui::ty::ui(13.0))
                                    .color(t.kit.popover_foreground),
                            ),
                    ),
                ),
        )
}

fn commands(s: &Gallery, t: &Theme) -> El {
    let f = &s.forms;
    line([
        labeled(
            t,
            "live",
            col().w(260.0).child(
                palette(&f.palette, "cmd-live", t)
                    .on_state(|s: &mut Gallery| &mut s.forms.palette)
                    .on_confirm(|s: &mut Gallery, p, _| s.forms.command = Some(p))
                    .build(t),
            ),
        ),
        labeled(
            t,
            "query \"pa\"",
            col()
                .w(260.0)
                .child(palette(&f.shown_palette, "cmd-shown", t).build(t)),
        ),
        labeled(
            t,
            "confirmed",
            text(f.command.map_or_else(
                || "—".into(),
                |p| format!("section {} row {}", p.section, p.row),
            ))
            .font(roostr_ui::ty::ui(12.0))
            .color(t.muted),
        ),
    ])
}

pub fn view(s: &Gallery, t: &Theme, layer: &mut Vec<El>) -> El {
    section(
        t,
        "FORMS",
        col()
            .gap(20.0)
            .child(labeled(t, "Input / Textarea", inputs(s, t)))
            .child(labeled(t, "Checkbox", checkboxes(s, t)))
            .child(labeled(t, "Switch", switches(s, t)))
            .child(labeled(t, "Select", selects(s, t, layer)))
            .child(labeled(t, "ListItem", list_items(s, t)))
            .child(labeled(t, "Command", commands(s, t))),
    )
}
