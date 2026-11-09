//! Settings, space management, the install-computer card and the identity
//! gate of the Roostr app (`Settings.svelte`, `SpaceManage.svelte`,
//! `InstallComputer.svelte`, `KeyGate.svelte` / `PairGate.svelte`, `export.ts`,
//! `client-identity.ts`, `client-profile.ts`), drawn with Rune over
//! [`roostr_core::State`].
//!
//! The app calls [`register`] once, draws [`settings_modal`] into its root
//! layer every frame, and opens Settings with [`open_settings`]. Every window
//! picks its palette with [`theme_pref`].

mod export;
mod identity_gate;
mod install;
mod keys;
mod local;
mod profile;
mod section_nav;
mod settings;
mod space_manage;
mod widgets;

use roostr_core::{NavEvent, Route, State};
use roostr_ui::ThemePref;
use rune::Tx;

pub use identity_gate::{IdentityGate, identity_gate};
pub use install::{InstallComputer, install_computer, start_install_poll, stop_install_poll};
pub use settings::{SettingsModal, close_settings, open_settings, settings_modal, settings_open};
pub use space_manage::{open_space_manage, space_manage};
/// The settings screens' buttons and text fields, for other gates (the
/// browser build's pairing gate).
pub use widgets::{Btn, Field, FieldStyle, Span, danger, h3, hint, rich, spinner};

/// Crate setup, once at startup: the listeners behind Settings' device list
/// and commit-on-blur fields, the space settings' store sync, and the nav
/// listener that prepares the space home (install card poll, space
/// settings) and space object pages.
pub fn register(s: &mut State) {
    s.on_store(settings::on_store);
    s.on_store(space_manage::on_store);
    s.on_poll(settings::on_poll);
    s.on_nav(on_nav);
    // The window may already show a page.
    prepare_route(s);
}

fn on_nav(s: &mut State, _: &NavEvent) {
    prepare_route(s);
}

/// The space home polls for a computer and shows the active space's
/// settings; a space's object page shows its settings.
fn prepare_route(s: &mut State) {
    match s.nav.route().clone() {
        Route::Home => {
            install::start_install_poll(s);
            let space = s.nav.space().to_string();
            if !space.is_empty() && s.store.space(&space).is_some() {
                open_space_manage(s, &space);
            }
        }
        Route::Object(id) => {
            install::stop_install_poll(s);
            if s.store.space(&id).is_some() {
                open_space_manage(s, &id);
            }
        }
        _ => install::stop_install_poll(s),
    }
}

/// The colour theme this device shows (Settings › Appearance): Dark, today's
/// look, until another is picked. Views pass it to [`roostr_ui::theme`].
pub fn theme_pref(s: &State) -> ThemePref {
    ThemePref::from_name(s.ui.theme_name())
}

/// Picks and remembers the colour theme; colours glide to it.
pub fn set_theme_pref(s: &mut State, tx: &mut Tx, pref: ThemePref) {
    if theme_pref(s) == pref {
        return;
    }
    s.ui.set_theme_name(pref.name());
    roostr_ui::switch_motion(tx);
}
