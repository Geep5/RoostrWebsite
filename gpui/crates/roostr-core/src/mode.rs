//! Where the vault lives: the glon-odin daemon on this computer (the desktop
//! default, the browser's paired mode) or this device's own replica (the
//! browser default, like roostr.space/app; on the desktop by choice). The
//! choice is kept per device and applied at the next start.

/// The backend the app runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Daemon,
    Replica,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Daemon => "daemon",
            Mode::Replica => "replica",
        }
    }

    pub fn parse(text: &str) -> Option<Mode> {
        match text.trim() {
            "daemon" => Some(Mode::Daemon),
            "replica" => Some(Mode::Replica),
            _ => None,
        }
    }
}

/// `localStorage` key of the browser's choice.
#[cfg(target_family = "wasm")]
const STORAGE_KEY: &str = "roostr-gpui.mode";

#[cfg(not(target_family = "wasm"))]
fn choice_file() -> Option<std::path::PathBuf> {
    Some(dirs::config_dir()?.join("Roostr").join("mode"))
}

/// The mode this device chose last, if it ever chose.
pub fn saved() -> Option<Mode> {
    #[cfg(not(target_family = "wasm"))]
    {
        Mode::parse(&std::fs::read_to_string(choice_file()?).ok()?)
    }
    #[cfg(target_family = "wasm")]
    {
        let storage = web_sys::window()?.local_storage().ok()??;
        Mode::parse(&storage.get_item(STORAGE_KEY).ok()??)
    }
}

/// Remember `mode` for the next start.
pub fn save(mode: Mode) {
    #[cfg(not(target_family = "wasm"))]
    if let Some(path) = choice_file() {
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&path, mode.as_str()));
        if let Err(error) = written {
            log::error!(
                "cannot save the backend choice to {}: {error}",
                path.display()
            );
        }
    }
    #[cfg(target_family = "wasm")]
    if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = storage.set_item(STORAGE_KEY, mode.as_str());
    }
}

/// The mode of the running app: a replica holds its account on the device.
pub fn current(cx: &gpui_kit::App) -> Mode {
    if crate::account::account(cx).is_some() {
        Mode::Replica
    } else {
        Mode::Daemon
    }
}

/// Command-line flags that pick a mode (`roostr --replica`).
pub const REPLICA_FLAG: &str = "--replica";
pub const DAEMON_FLAG: &str = "--daemon";

/// Switch to `mode` now: remembered, then the page reloads (browser) or the
/// app relaunches itself in that mode (desktop).
pub fn switch(mode: Mode, cx: &mut gpui_kit::App) {
    save(mode);
    #[cfg(target_family = "wasm")]
    {
        let _ = cx;
        if let Some(window) = web_sys::window() {
            let _ = window.location().reload();
        }
    }
    #[cfg(not(target_family = "wasm"))]
    {
        let flag = match mode {
            Mode::Daemon => DAEMON_FLAG,
            Mode::Replica => REPLICA_FLAG,
        };
        let args = std::env::args()
            .skip(1)
            .filter(|a| a != REPLICA_FLAG && a != DAEMON_FLAG);
        let relaunched = std::env::current_exe()
            .and_then(|exe| std::process::Command::new(exe).arg(flag).args(args).spawn());
        match relaunched {
            Ok(_) => cx.quit(),
            Err(error) => log::error!("cannot relaunch Roostr: {error}"),
        }
    }
}
