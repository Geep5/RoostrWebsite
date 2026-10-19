//! This identity's Nostr profile (name + avatar) through the local harness
//! (`client-profile.ts` in local mode: `GET`/`POST /profile`), or, in replica
//! mode, straight from the relays (`engine/profile.ts`). The last answer is
//! kept in a global entity so Settings and the rail avatar share it; observe
//! [`ProfileCache::global`] to re-render when it changes.

use std::sync::Arc;

use anyhow::{Result, anyhow};
use gpui_kit::{App, AppContext as _, Entity, Global, Image, Task};
use serde::Deserialize;
use serde_json::{Value as Json, json};

use crate::harness::{HarnessError, harness};
use crate::io;
use crate::ui::data_url_image;

/// A Nostr kind-0 profile (`Profile` in engine/profile.ts).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Profile {
    #[serde(default)]
    pub name: Option<String>,
    /// An avatar URL or `data:image/…` URL.
    #[serde(default)]
    pub picture: Option<String>,
}

/// The last profile the harness answered, with its decoded avatar.
#[derive(Default)]
pub struct ProfileCache {
    profile: Option<Profile>,
    avatar: Option<Arc<Image>>,
}

struct GlobalProfile(Entity<ProfileCache>);

impl Global for GlobalProfile {}

/// Install the cache and fetch once when this app has a harness or an
/// on-device account. Called by [`crate::init`].
pub fn init(cx: &mut App) {
    let cache = cx.new(|_| ProfileCache::default());
    cx.set_global(GlobalProfile(cache));
    if harness(cx).is_some() || crate::account::account(cx).is_some() {
        cx.spawn(async move |cx| {
            if let Err(error) = cx.update(fetch).await {
                log::debug!("profile: {error:#}");
            }
        })
        .detach();
    }
}

impl ProfileCache {
    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalProfile>().0.clone()
    }

    /// `None` before the first answer.
    pub fn profile(&self) -> Option<&Profile> {
        self.profile.as_ref()
    }

    /// The display name, "" when unset.
    pub fn name(&self) -> &str {
        self.profile
            .as_ref()
            .and_then(|p| p.name.as_deref())
            .unwrap_or("")
    }

    /// The avatar URL/data URL, "" when unset.
    pub fn picture(&self) -> &str {
        self.profile
            .as_ref()
            .and_then(|p| p.picture.as_deref())
            .unwrap_or("")
    }

    /// The avatar decoded, when it is a `data:image/` URL (render web URLs with `img(url)`).
    pub fn avatar(&self) -> Option<Arc<Image>> {
        self.avatar.clone()
    }
}

/// The profile as last fetched or saved, empty before the first answer.
pub fn cached(cx: &App) -> Profile {
    ProfileCache::global(cx)
        .read(cx)
        .profile
        .clone()
        .unwrap_or_default()
}

fn remember(path: &'static str, body: Option<Json>, cx: &mut App) -> Task<Result<Profile>> {
    let harness = harness(cx);
    let account = crate::account::account(cx);
    let request: Task<Result<Profile>> = io::spawn(cx, async move {
        if let Some(account) = account {
            let answer = match body {
                Some(patch) => account.save_profile(patch).await?,
                None => account.profile().await?,
            };
            return Ok(serde_json::from_value::<Profile>(answer)?);
        }
        let harness = harness.ok_or_else(|| anyhow!("No local daemon for this app."))?;
        let answer = match body {
            Some(body) => harness.post_json(path, &body).await,
            None => harness.get_json(path).await,
        };
        let answer = answer.map_err(|e| match e.downcast_ref::<HarnessError>() {
            Some(h) => anyhow!("Local profile unavailable ({})", h.status),
            None => e,
        })?;
        Ok(serde_json::from_value::<Profile>(answer)?)
    });
    cx.spawn(async move |cx| {
        let profile = request.await?;
        cx.update(|cx| {
            ProfileCache::global(cx).update(cx, |cache, cx| {
                if cache.profile.as_ref() != Some(&profile) {
                    cache.avatar = profile.picture.as_deref().and_then(data_url_image);
                    cache.profile = Some(profile.clone());
                    cx.notify();
                }
            })
        });
        Ok(profile)
    })
}

/// `GET /profile`; updates the cache.
pub fn fetch(cx: &mut App) -> Task<Result<Profile>> {
    remember("/profile", None, cx)
}

/// `POST /profile` with a patch (see [`patch`]); resolves to the merged profile and updates the cache.
pub fn save(patch: Json, cx: &mut App) -> Task<Result<Profile>> {
    remember("/profile", Some(patch), cx)
}

/// `{name}` / `{picture}` patches ("" clears).
pub fn patch(key: &str, value: &str) -> Json {
    json!({ key: value })
}
