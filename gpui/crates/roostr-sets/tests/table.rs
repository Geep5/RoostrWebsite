//! The query table, headless: rows windowed against the window, the horizontal
//! scrollbar of a too-wide table and its wheel, and the cell editor (ported from
//! roostr-app's `table_rows_window`, `table_hscroll` and `table_cell_editor`).
#![cfg(not(target_family = "wasm"))]

mod support;

use roostr_core::types::{QueryRow, Value};
use roostr_core::{Route, nav};
use roostr_sets::DataViewKind;
use rune::{Key, WheelEvent};
use serde_json::{Value as Json, json};
use support::{App, Vault, object, relation, value};

/// Rows of `type_key` for the query `q1`; type lookups by key for the rest.
fn query_rows(type_key: &'static str) -> Box<dyn Fn(&Vault, &Json) -> Vec<QueryRow> + Send + Sync> {
    Box::new(move |vault, body| {
        let text = body.to_string();
        if text.contains("\"setId\":\"q1\"") {
            return vault.of_type(type_key);
        }
        vault
            .objects
            .lock()
            .iter()
            .filter(|o| {
                !text.contains("setId")
                    && o.type_key != "query"
                    && text.contains(&format!("\"{}\"", o.type_key))
            })
            .map(Vault::row)
            .collect()
    })
}

/// `viewRelations` of `keys`, each `width` wide.
fn columns(keys: &[String], width: i64) -> Value {
    value(
        json!({"valuesValue": {"items": keys.iter().map(|k| json!({"mapValue": {"entries": {
        "key": {"stringValue": k}, "width": {"intValue": width}}}})).collect::<Vec<_>>()}}),
    )
}

/// A big query's table builds only the rows in and near the window. Field
/// report: scrolling a 187-email query lagged - the page scrolls the table,
/// and building every loaded row (a row of cells each) on every change cost
/// too much. Rows far below aren't built until they come near; spacers keep
/// the table's height.
#[test]
fn a_big_querys_table_draws_only_the_rows_near_the_window() {
    const ROWS: usize = 90;
    let mut objects = vec![
        object(
            "type-email",
            "type",
            &[
                ("name", Value::string("Email")),
                ("key", Value::string("email")),
            ],
        ),
        object(
            "q1",
            "query",
            &[
                ("name", Value::string("Support Emails")),
                ("setOf", Value::values(vec![Value::string("email")])),
                ("viewRelations", columns(&["email_from".into()], 150)),
            ],
        ),
    ];
    objects.extend((0..ROWS).map(|i| {
        object(
            &format!("e{i}"),
            "email",
            &[
                ("name", Value::string(format!("Email {i}"))),
                ("email_from", Value::string(format!("p{i}@example.com"))),
            ],
        )
    }));
    let vault = Vault::new(
        objects,
        vec![relation("rel-from", "email_from", "shorttext", "From")],
        query_rows("email"),
    );
    let mut app = App::new(vault, "q1", DataViewKind::Query, (1400., 600.));
    app.until_shown("cell-e0-email_from");
    assert!(
        !app.shown("cell-e89-email_from"),
        "the last row, ~2900pt down, isn't built"
    );
    let first = app.rect("row-e0").expect("the first row");
    // Spacers keep the table's full height: the New Object row sits under all 90.
    let new_row = app.rect("q1/new-object").expect("the New Object row");
    assert!(
        new_row[1] - first[1] >= first[3] * (ROWS as f32 - 1.),
        "the table keeps the height of every row: {first:?} .. {new_row:?}"
    );
    // Scrolled to the end, the last rows are built (and the first aren't).
    for _ in 0..40 {
        app.rt.wheel(700., 300., -400.);
        app.step();
    }
    app.until_shown("cell-e89-email_from");
    assert!(
        !app.shown("cell-e0-email_from"),
        "the first row, far above, isn't built any more"
    );
}

#[test]
fn on_a_phone_a_finger_dragged_up_the_rows_scrolls_and_selects_nothing() {
    use rune::TouchPhase;
    let mut objects = vec![
        object(
            "type-email",
            "type",
            &[
                ("name", Value::string("Email")),
                ("key", Value::string("email")),
            ],
        ),
        object(
            "q1",
            "query",
            &[
                ("name", Value::string("Support Emails")),
                ("setOf", Value::values(vec![Value::string("email")])),
            ],
        ),
    ];
    objects.extend((0..40).map(|i| {
        object(
            &format!("e{i}"),
            "email",
            &[("name", Value::string(format!("Email {i}")))],
        )
    }));
    let vault = Vault::new(objects, Vec::new(), query_rows("email"));
    let mut app = App::new(vault, "q1", DataViewKind::Query, (390., 844.));
    app.until_shown("row-e5");
    let before = app.rect("row-e5").expect("a row");
    let (x, y) = (before[0] + 60., before[1] + before[3] / 2.);
    let mut t = 0.0;
    app.rt.touch(1, TouchPhase::Started, x, y, t);
    for i in 1..=12 {
        t += 1.0 / 60.0;
        app.rt.touch(1, TouchPhase::Moved, x, y - 25. * i as f32, t);
        app.step();
    }
    t += 0.2;
    app.rt.touch(1, TouchPhase::Ended, x, y - 300., t);
    for _ in 0..30 {
        app.step();
    }
    let after = app.rect("row-e5").expect("the row");
    assert!(
        after[1] < before[1] - 150.,
        "the page scrolled: {before:?} -> {after:?}"
    );
    assert!(!app.shown("q1/marquee"), "no rubber band");
    assert!(!app.shown("q1/sel-bar"), "nothing selected");
}

fn wide_vault() -> Vault {
    const ROWS: usize = 3;
    const COLS: usize = 12;
    let keys: Vec<String> = (0..COLS).map(|c| format!("p{c}")).collect();
    let mut objects = vec![
        object(
            "type-email",
            "type",
            &[
                ("name", Value::string("Email")),
                ("key", Value::string("email")),
            ],
        ),
        object(
            "q1",
            "query",
            &[
                ("name", Value::string("Support Emails")),
                ("setOf", Value::values(vec![Value::string("email")])),
                ("viewRelations", columns(&keys, 200)),
            ],
        ),
    ];
    objects.extend((0..ROWS).map(|i| {
        let name = Value::string(format!("Email {i}"));
        let mut pairs: Vec<(&str, Value)> = vec![("name", name)];
        pairs.extend(keys.iter().map(|k| (k.as_str(), Value::string("value"))));
        object(&format!("e{i}"), "email", &pairs)
    }));
    let relations = (0..COLS)
        .map(|c| {
            relation(
                &format!("rel-p{c}"),
                &format!("p{c}"),
                "shorttext",
                &format!("Property {c}"),
            )
        })
        .collect();
    Vault::new(objects, relations, query_rows("email"))
}

/// A table wider than its pane shows a horizontal scrollbar along its bottom
/// (with room for it under the last row); one that fits shows none. The
/// wheel over that scrollbar scrolls the table sideways; over the rows it
/// leaves the table where it is.
#[test]
fn a_too_wide_table_scrolls_sideways_from_its_scrollbar() {
    // Twelve 200pt columns: wider than a 1000pt window, narrower than a 4000pt one.
    let mut app = App::new(wide_vault(), "q1", DataViewKind::Query, (4000., 700.));
    app.until_shown("cell-e0-p0");
    assert!(
        !app.shown("q1/set-table-scrollbar"),
        "a table that fits has no scrollbar"
    );
    app.rt.set_viewport((1000., 700.), 2.0);
    for _ in 0..4 {
        app.step();
    }
    assert!(
        app.shown("q1/set-table-scrollbar"),
        "a table wider than its pane shows one"
    );
    let scroller = app.rect("q1/table-scroll").expect("the table's viewport");
    let last = app.rect("cell-e2-p0").expect("the last row");
    assert!(
        scroller[1] + scroller[3] >= last[1] + last[3] + 9.,
        "room for the scrollbar under the last row: {scroller:?} vs {last:?}"
    );
    let col_x = |app: &App| app.rect("cell-e0-p3").expect("a cell")[0];
    let before = col_x(&app);
    let wheel = |app: &mut App, x: f32, y: f32| {
        app.rt.wheel_event(WheelEvent::new(0., -120.).at(x, y));
        app.step();
        app.step();
    };
    wheel(&mut app, last[0] + last[2] / 2., last[1] + last[3] / 2.);
    assert_eq!(
        col_x(&app),
        before,
        "a wheel over the rows leaves the table where it is"
    );
    wheel(
        &mut app,
        scroller[0] + scroller[2] / 2.,
        scroller[1] + scroller[3] - 4.,
    );
    assert!(
        col_x(&app) < before,
        "a wheel over the scrollbar moves the columns left"
    );
    app.rt.set_viewport((4000., 700.), 2.0);
    for _ in 0..4 {
        app.step();
    }
    assert!(
        !app.shown("q1/set-table-scrollbar"),
        "and it goes when the table fits again"
    );
}

const PUBLISHER: &str = "pub-1";
const CELL: &str = "cell-pub-1-main_games";
const NAME: &str = "name-pub-1";

/// One space: Game and Game Publisher types, two games here and one named
/// like Tanks in another space, a publisher whose Main games (no type limit,
/// any number) holds TYR, and a query of publishers showing Main games.
fn publishers() -> Vault {
    let mut elsewhere = object("g9", "game", &[("name", Value::string("Tanks Elsewhere"))]);
    elsewhere
        .fields
        .insert("channel".into(), Value::string("sp-2"));
    let objects = vec![
        object(
            "type-game",
            "type",
            &[
                ("name", Value::string("Game")),
                ("key", Value::string("game")),
            ],
        ),
        object(
            "type-pub",
            "type",
            &[
                ("name", Value::string("Game Publisher")),
                ("key", Value::string("game_publisher")),
            ],
        ),
        object("g1", "game", &[("name", Value::string("TYR"))]),
        object("g2", "game", &[("name", Value::string("Tanks"))]),
        elsewhere,
        object(
            PUBLISHER,
            "game_publisher",
            &[
                ("name", Value::string("Stoke")),
                (
                    "main_games",
                    Value::values(vec![Value::link("g1", "main_games")]),
                ),
            ],
        ),
        object(
            "q1",
            "query",
            &[
                ("name", Value::string("Publishers")),
                (
                    "setOf",
                    Value::values(vec![Value::string("game_publisher")]),
                ),
                ("viewRelations", columns(&["main_games".into()], 150)),
            ],
        ),
    ];
    let relations = vec![
        relation("rel-games", "main_games", "object", "Main Games"),
        relation("rel-priority", "priority", "shorttext", "Priority"),
    ];
    let on_query = Box::new(|vault: &Vault, body: &Json| {
        let text = body.to_string();
        vault
            .objects
            .lock()
            .iter()
            .filter(|o| {
                (text.contains("\"setId\":\"q1\"") && o.type_key == "game_publisher")
                    || (!text.contains("setId")
                        && o.type_key != "query"
                        && text.contains(&format!("\"{}\"", o.type_key)))
            })
            .map(Vault::row)
            .collect()
    });
    Vault::new(objects, relations, on_query)
}

/// The games the publisher's Main games links, as the backend holds them.
fn main_games(vault: &Vault) -> Vec<String> {
    vault
        .field(PUBLISHER, "main_games")
        .map(|v| {
            v.items()
                .iter()
                .filter_map(|i| i.link_value.as_ref().map(|l| l.target_id.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// A table cell's link editor can be used. A query's table wider than its
/// page column is clipped there: a cell near that edge used to open its
/// editor at the cell, mostly past the clip, and picking from its link menu
/// closed the editor before the pick was saved. The editor now opens inside
/// the table's visible width with its menu in its own flow, and a second
/// game is added to the first. Pills open their objects; the Name column
/// stays put over the cells scrolled under it; ← / → scroll the table under
/// the pointer, never while a text field has the keys.
#[test]
fn a_cell_editor_at_the_page_columns_edge_opens_where_it_can_be_used() {
    // A page column narrower than the table (Name 180 + Main games 150 + "+").
    let mut app = App::new(publishers(), "q1", DataViewKind::Query, (380., 900.));
    app.until_shown(CELL);
    // A linked game shows as its own pill (emoji and name), not a comma list.
    let pill = "pill-pub-1-main_games-g1";
    assert!(app.shown(pill), "TYR is a pill in the Main games cell");
    // Clicking the pill opens the game, not the cell's editor.
    app.click(pill);
    assert!(
        !app.shown("pop-pub-1-main_games/link-object"),
        "no cell editor from a pill"
    );
    assert_eq!(app.route(), Route::Object("g1".into()));
    nav::navigate(Route::Object("q1".into()), app.s());
    app.step();

    // With the pointer over the table and nothing focused, → scrolls it
    // sideways, ← back.
    let cell = app.rect(CELL).expect("the Main games cell");
    let name = app.rect(NAME).expect("the publisher's Name cell");
    let before = cell[0];
    app.rt.pointer_move(cell[0] + 8., cell[1] + cell[3] / 2.);
    app.step();
    window_key(&mut app, Key::Right);
    let scrolled = app.rect(CELL).unwrap()[0];
    assert!(
        scrolled < before,
        "→ scrolled the table: {before} → {scrolled}"
    );
    // The Name column stays where it was, over the cells scrolled under it: a
    // press on it opens the publisher, never the editor of a cell beneath.
    let pinned = app.rect(NAME).unwrap();
    assert_eq!(
        pinned[0] + pinned[2],
        name[0] + name[2],
        "the Name column stays put while the table scrolls"
    );
    assert!(
        scrolled < name[0] + name[2],
        "Main games scrolled under the Name column: {scrolled} < {}",
        name[0] + name[2]
    );
    app.click_at(name[0] + name[2] - 4., cell[1] + cell[3] / 2.);
    assert!(
        !app.shown("pop-pub-1-main_games/link-object"),
        "a press on the pinned Name opened no cell editor"
    );
    assert_eq!(
        app.route(),
        Route::Object(PUBLISHER.into()),
        "a press on the pinned Name opens its row's object"
    );
    nav::navigate(Route::Object("q1".into()), app.s());
    app.step();
    app.rt.pointer_move(cell[0] + 8., cell[1] + cell[3] / 2.);
    app.step();
    window_key(&mut app, Key::Left);
    assert_eq!(app.rect(CELL).unwrap()[0], before, "← scrolled it back");

    // A press on the cell beside its pill opens its editor; a cell that
    // already links something opens on its chips: + adds another.
    let cell = app.rect(CELL).unwrap();
    // Its padding, left of the pill (the pinned "+" column covers its right end).
    app.click_at(cell[0] + 4., cell[1] + cell[3] / 2.);
    app.until_shown("pop-pub-1-main_games/link-object");
    app.click("pop-pub-1-main_games/link-object");
    app.until_shown("pop-pub-1-main_games/obj-menu");
    let menu = app.rect("pop-pub-1-main_games/obj-menu").unwrap();
    let cell = app.rect(CELL).unwrap();
    let clip = app.rect("q1/table-scroll").unwrap();
    // The cell sits at the clipped edge; its picker opens inside the table's
    // visible width, pulled left of the cell.
    assert!(
        menu[0] + menu[2] > cell[0],
        "{menu:?} reaches the cell {cell:?}"
    );
    assert!(menu[0] < cell[0], "{menu:?} pulled left of {cell:?}");
    assert!(
        menu[0] >= clip[0] - 0.5,
        "{menu:?} inside the column {clip:?}"
    );

    // The second game is offered and added after TYR.
    app.type_text("tan");
    app.step();
    // Typing in the link search: → at the end of the text belongs to the
    // field, never scrolls the table under the pointer.
    app.rt.pointer_move(cell[0] + 8., cell[1] + cell[3] / 2.);
    window_key(&mut app, Key::Right);
    assert_eq!(
        app.rect(CELL).unwrap()[0],
        cell[0],
        "→ while typing left the table where it was"
    );
    // The cell's picker offers the row's own space only.
    assert!(
        !app.shown("pop-pub-1-main_games/cand-g9"),
        "a game from another space is offered"
    );
    app.until_shown("pop-pub-1-main_games/cand-g2");
    app.click("pop-pub-1-main_games/cand-g2");
    let vault = app.vault.clone();
    app.until("the second game saved", move |_| {
        main_games(&vault).len() == 2
    });
    assert_eq!(main_games(&app.vault), ["g1", "g2"]);
}

/// A key the window's focused element didn't take, handed to the data views
/// as the app's window `on_key` does.
fn window_key(app: &mut App, key: Key) {
    app.rt.on_key = Some(|s, ev, tx| roostr_sets::on_key(s, ev, tx));
    app.key(key);
    app.step();
}
