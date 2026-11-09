//! This identity's Nostr profile (name + avatar) through the local harness
//! (`client-profile.ts` in local mode: `GET`/`POST /profile`), or, in replica
//! mode, straight from the relays (`engine/profile.ts`). The last answer is
//! kept in [`crate::State::profile`] so Settings and the rail avatar share it.

use std::sync::Arc;

use anyhow::{Result, anyhow};
use roostr_rt::Task;
use serde::Deserialize;
use serde_json::{Value as Json, json};

use crate::State;
use crate::harness::HarnessError;
use crate::objects::{DataImage, data_url_image};

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
    avatar: Option<Arc<DataImage>>,
}

impl ProfileCache {
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

    /// The avatar's bytes, when it is a `data:image/` URL (load web URLs by address).
    pub fn avatar(&self) -> Option<Arc<DataImage>> {
        self.avatar.clone()
    }
}

/// The profile as last fetched or saved, empty before the first answer.
pub fn cached(s: &State) -> Profile {
    s.profile.profile.clone().unwrap_or_default()
}

fn remember(path: &'static str, body: Option<Json>, s: &mut State) -> Task<Result<Profile>> {
    let harness = s.harness.clone();
    let account = crate::account::account(s);
    let request: Task<Result<Profile>> = s.rt.background(async move {
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
    s.rt.spawn(async move |cx| {
        let profile = request.await?;
        cx.update(|s| {
            let cache = &mut s.profile;
            if cache.profile.as_ref() != Some(&profile) {
                cache.avatar = profile.picture.as_deref().and_then(data_url_image);
                cache.profile = Some(profile.clone());
            }
        });
        Ok(profile)
    })
}

/// `GET /profile`; updates the cache.
pub fn fetch(s: &mut State) -> Task<Result<Profile>> {
    remember("/profile", None, s)
}

/// `POST /profile` with a patch (see [`patch`]); resolves to the merged profile and updates the cache.
pub fn save(patch: Json, s: &mut State) -> Task<Result<Profile>> {
    remember("/profile", Some(patch), s)
}

/// `{name}` / `{picture}` patches ("" clears).
pub fn patch(key: &str, value: &str) -> Json {
    json!({ key: value })
}
