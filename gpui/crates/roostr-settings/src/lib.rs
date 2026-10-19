//! Settings, space management, the install-computer card and the identity
//! gate of the Roostr app (`Settings.svelte`, `SpaceManage.svelte`,
//! `InstallComputer.svelte`, `KeyGate.svelte` / `PairGate.svelte`, `export.ts`,
//! `client-identity.ts`, `client-profile.ts`).

mod export;
mod identity_gate;
mod install;
mod keys;
mod local;
mod profile;
mod settings;
mod space_manage;
mod widgets;

use gpui_kit::App;

pub use identity_gate::IdentityGate;
pub use install::InstallComputer;
pub use settings::{Settings, open_settings};
pub use space_manage::SpaceManage;
/// The settings screens' buttons and text fields, for other gates (the
/// browser build's pairing gate).
pub use widgets::{Btn, FieldStyle, field};

/// Crate setup. The settings screens register no actions or key bindings
/// (the web app has none; Escape in the modals is handled by the views).
pub fn init(_cx: &mut App) {}
