//! Credential cells and their Reconnect popover over the fake vault and a fake harness
//! (a local HTTP server answering this computer's id and the credential actions by writing
//! the credential in the vault, as the real harness does).

mod support;

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, LazyLock};

use parking_lot::Mutex;
use roostr_core::State;
use roostr_core::daemon::DaemonConfig;
use roostr_core::harness::Harness;
use roostr_core::types::{QueryRow, Value};
use roostr_properties::{CredentialCell, CredentialTarget, credential_cell_badge};
use roostr_ui::ThemePref;
use rune::{Cx, El, col, text};
use serde_json::{Value as Json, json};
use support::{App, Vault, object};

const MACHINE: &str = "m1";

/// Vaults by credential id (each test uses its own ids), for the fake harness to write.
static VAULTS: LazyLock<Mutex<HashMap<String, Arc<Vault>>>> = LazyLock::new(Default::default);
/// Every credential action the fake harness ran: (action, credential id).
static ACTIONS: LazyLock<Mutex<Vec<(String, String)>>> = LazyLock::new(Default::default);

/// Starts the fake harness once (and points HOME at a temp dir so the harness session file
/// stays out of the real config); its base URL.
static HARNESS_URL: LazyLock<String> = LazyLock::new(|| {
    let home = std::env::temp_dir().join(format!("roostr-creds-home-{}", std::process::id()));
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake harness");
    let url = format!("http://{}", listener.local_addr().expect("addr"));
    // SAFETY: set before any harness exists, from the first test that needs one.
    unsafe {
        std::env::set_var("HOME", &home);
        std::env::set_var("ROOSTR_HARNESS", &url);
    }
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));
            let mut first = String::new();
            if reader.read_line(&mut first).is_err() {
                continue;
            }
            let mut length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some((k, v)) = line.split_once(':')
                    && k.eq_ignore_ascii_case("content-length")
                {
                    length = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0; length];
            let _ = reader.read_exact(&mut body);
            let path = first.split_whitespace().nth(1).unwrap_or("").to_string();
            let answer = answer(&path, &body);
            let bytes = answer.to_string();
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{bytes}",
                bytes.len()
            );
        }
    });
    url
});

fn answer(path: &str, body: &[u8]) -> Json {
    match path {
        "/api/local-auth/session" => json!({ "token": "t", "expiresAt": 4.0e12 }),
        "/machine" => json!({ "id": MACHINE }),
        _ => {
            let Some(action) = path.strip_prefix("/credentials/") else {
                return json!({ "error": "unknown" });
            };
            let id = serde_json::from_slice::<Json>(body)
                .ok()
                .and_then(|b| b["id"].as_str().map(str::to_string))
                .unwrap_or_default();
            ACTIONS.lock().push((action.to_string(), id.clone()));
            if let Some(vault) = VAULTS.lock().get(&id)
                && let Some(o) = vault.objects.lock().iter_mut().find(|o| o.id == id)
            {
                o.fields.insert("status".into(), Value::string("active"));
                o.fields.insert("error".into(), Value::string(""));
                o.fields.insert(
                    "checked_at".into(),
                    Value::int(chrono::Utc::now().timestamp_millis()),
                );
                o.updated_at += 1;
            }
            json!({ "status": "active" })
        }
    }
}

fn harness() -> Arc<Harness> {
    let url = HARNESS_URL.clone();
    Arc::new(Harness::new(DaemonConfig {
        base_url: url,
        token: "test".into(),
        token_source: "test".into(),
    }))
}

/// This computer's machine object and a signed-out Google credential `id` it serves.
fn vault(id: &str) -> Vault {
    Vault::new(
        vec![
            object(
                "machine-1",
                "machine",
                &[
                    ("machine_id", Value::string(MACHINE)),
                    ("name", Value::string("Mac")),
                ],
            ),
            object(
                id,
                "credential",
                &[
                    ("name", Value::string("Gmail")),
                    ("status", Value::string("needs_auth")),
                    ("service", Value::string("google-account")),
                    ("account", Value::string("me@example.com")),
                    ("served_by", Value::string(MACHINE)),
                ],
            ),
        ],
        vec![],
    )
}

fn row(s: &State, id: &str) -> QueryRow {
    s.store
        .object(id)
        .map(|o| QueryRow {
            id: o.id.clone(),
            type_key: o.type_key.clone(),
            fields: o.fields.clone(),
            ..QueryRow::default()
        })
        .unwrap_or_default()
}

fn cell_view(s: &State, cx: &mut Cx<State>) -> El {
    let t = roostr_ui::theme(ThemePref::Dark, cx);
    let mut layer = vec![];
    let row = row(s, "cred-a");
    let (badge, _, _) = credential_cell_badge(&row, s);
    let body = col().padding(20.0).child(
        CredentialCell::new(
            "cell",
            row,
            CredentialTarget::Credential("cred-a".into()),
            "Reconnect",
        )
        .child(text(badge).key("badge"))
        .build(s, cx, t, &mut layer),
    );
    roostr_ui::root(body, layer)
}

/// Polls until the executor wakes once more (a worker thread answering).
fn wake(app: &mut App) {
    let n = std::cell::Cell::new(0);
    app.until("a wake", |_| {
        n.set(n.get() + 1);
        n.get() > 1
    });
}

fn texts(app: &App) -> Vec<String> {
    app.rt
        .elements()
        .into_iter()
        .filter_map(|e| e.text)
        .collect()
}

#[test]
fn a_cell_opens_its_reconnect_popover_and_reconnect_runs_the_credential_action() {
    let app_vault = vault("cred-a");
    let mut app = App::new(app_vault, cell_view);
    VAULTS.lock().insert("cred-a".into(), app.vault.clone());
    app.s().harness = Some(harness());
    app.s().store.ensure_object("cred-a");
    app.until("the credential", |s| s.store.object("cred-a").is_some());
    assert!(texts(&app).contains(&"Signed out - reconnect".to_string()));

    app.click("cell");
    assert!(app.shown("cred-pop"));
    let run = "cred-pop/cred-block-cred-a/run";
    let mut tries = 0;
    while !app.shown(run) {
        tries += 1;
        assert!(tries < 200, "no Reconnect button: {:?}", texts(&app));
        wake(&mut app);
    }
    assert!(texts(&app).iter().any(|t| t == "Reconnect Gmail"));

    app.click(run);
    app.until("the connect action", |s| {
        s.store
            .object("cred-a")
            .is_some_and(|o| o.field_str("status") == "active")
    });
    assert!(
        ACTIONS
            .lock()
            .contains(&("connect".to_string(), "cred-a".to_string()))
    );
    let mut tries = 0;
    while !app.shown("cred-pop/cred-block-cred-a/connected") {
        tries += 1;
        assert!(tries < 200, "not connected: {:?}", texts(&app));
        wake(&mut app);
    }
    assert!(texts(&app).contains(&"Connected".to_string()));

    // The outside-press backdrop closes it.
    app.click("cred-backdrop");
    assert!(!app.shown("cred-pop"));
}
