//! Embed processor detection + URL transforms (`src/lib/embed.ts`, from
//! Anytype's lib/util/embed.ts): domain patterns, youtube path extraction
//! incl. shorts and `?t=` start offsets, youtube-nocookie embed host.

use std::sync::LazyLock;

use regex::Regex;
use url::Url;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Processor {
    Youtube,
    Vimeo,
    Spotify,
}

impl Processor {
    /// The name stored in the block meta (`processor`).
    pub fn key(self) -> &'static str {
        match self {
            Self::Youtube => "youtube",
            Self::Vimeo => "vimeo",
            Self::Spotify => "spotify",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "youtube" => Some(Self::Youtube),
            "vimeo" => Some(Self::Vimeo),
            "spotify" => Some(Self::Spotify),
            _ => None,
        }
    }

    /// Display name for the native embed card.
    pub fn label(self) -> &'static str {
        match self {
            Self::Youtube => "YouTube",
            Self::Vimeo => "Vimeo",
            Self::Spotify => "Spotify",
        }
    }
}

static REGEXPS: LazyLock<Vec<(Processor, Regex)>> = LazyLock::new(|| {
    [
        (
            Processor::Youtube,
            r"youtube\.com|youtu\.be|youtube-nocookie\.com",
        ),
        (Processor::Vimeo, r"vimeo\.com"),
        (Processor::Spotify, r"spotify\.com|open\.spotify\.com"),
    ]
    .into_iter()
    .map(|(p, domains)| {
        (
            p,
            Regex::new(&format!(r"(?i)://([^.]*\.)?({domains})([/\?#:]|$)"))
                .expect("valid embed regex"),
        )
    })
    .collect()
});

/// `getProcessorByUrl`: first matching domain, with per-site limits (youtube
/// channel/hashtag pages are not embeddable).
pub fn processor_for_url(url: &str) -> Option<Processor> {
    for (processor, re) in REGEXPS.iter() {
        if !re.is_match(url) {
            continue;
        }
        if *processor == Processor::Youtube {
            let info = Url::parse(url).ok()?;
            if info.path().starts_with("/@") || info.path().contains("/hashtag/") {
                return None;
            }
        }
        return Some(*processor);
    }
    None
}

static YT_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^.*(youtu.be/|v/|u/\w/|embed/|watch\?v=|&v=)([^#&?]*).*").expect("valid regex")
});
static YT_TIME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\?t=|&t=)(\d+)").expect("valid regex"));

/// `getYoutubePath`: video id + optional `?start=`.
pub fn youtube_path(url: &str) -> String {
    let url = url.replacen("/shorts/", "/watch?v=", 1);
    let Some(id) = YT_PATH
        .captures(&url)
        .and_then(|c| c.get(2))
        .map(|m| m.as_str().to_string())
    else {
        return String::new();
    };
    if id.is_empty() {
        return String::new();
    }
    match YT_TIME.captures(&url).and_then(|c| c.get(2)) {
        Some(t) if !t.as_str().is_empty() => format!("{id}?start={}", t.as_str()),
        _ => id,
    }
}

/// `getEmbedUrl`: the player address for a raw URL.
pub fn embed_url(processor: Processor, url: &str) -> String {
    match processor {
        Processor::Youtube => format!(
            "https://www.youtube-nocookie.com/embed/{}",
            youtube_path(url)
        ),
        Processor::Vimeo => Url::parse(url)
            .map(|u| format!("https://player.vimeo.com/video{}", u.path()))
            .unwrap_or_else(|_| url.to_string()),
        Processor::Spotify => Url::parse(url)
            .map(|u| format!("https://open.spotify.com/embed{}", u.path()))
            .unwrap_or_else(|_| url.to_string()),
    }
}

/// A pasted string that is exactly one http(s) URL.
pub fn is_single_url(text: &str) -> bool {
    let t = text.trim();
    (t.starts_with("http://") || t.starts_with("https://"))
        && t.len() > t.find("://").map_or(0, |i| i + 3)
        && !t.chars().any(char::is_whitespace)
}

/// What the native card shows in place of the web's player iframe: the
/// player's still and its title.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Poster {
    pub thumb: Option<String>,
    pub title: Option<String>,
}

/// The still and title for an embed, from the provider's oEmbed endpoint
/// (Vimeo falls back to its simple API, which also serves videos oEmbed
/// refuses). Run it through [`roostr_core::io::spawn`].
pub async fn fetch_poster(processor: Processor, url: &str) -> Poster {
    async fn get(endpoint: String) -> Option<serde_json::Value> {
        let response = roostr_core::http::get(
            &endpoint,
            &[],
            Some(std::time::Duration::from_secs(10)),
            10 * 1024 * 1024,
        )
        .await
        .ok()?;
        response
            .is_success()
            .then(|| response.json().ok())
            .flatten()
    }
    let oembed = async |base: &str| match Url::parse_with_params(base, [("url", url)]) {
        Ok(u) => get(u.to_string()).await,
        Err(_) => None,
    };
    let text = |v: &serde_json::Value, key: &str| {
        v.get(key)
            .and_then(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    match processor {
        Processor::Youtube => {
            let vid = youtube_path(url);
            let vid = vid.split('?').next().unwrap_or_default();
            Poster {
                thumb: Some(format!("https://img.youtube.com/vi/{vid}/hqdefault.jpg")),
                title: oembed("https://www.youtube.com/oembed")
                    .await
                    .and_then(|v| text(&v, "title")),
            }
        }
        Processor::Spotify => oembed("https://open.spotify.com/oembed")
            .await
            .map(|v| Poster {
                thumb: text(&v, "thumbnail_url"),
                title: text(&v, "title"),
            })
            .unwrap_or_default(),
        Processor::Vimeo => {
            if let Some(v) = oembed("https://vimeo.com/api/oembed.json").await {
                return Poster {
                    thumb: text(&v, "thumbnail_url"),
                    title: text(&v, "title"),
                };
            }
            let id = Url::parse(url)
                .ok()
                .and_then(|u| {
                    u.path_segments()?
                        .rev()
                        .find(|s| !s.is_empty())
                        .map(str::to_string)
                })
                .filter(|id| id.bytes().all(|b| b.is_ascii_digit()));
            let Some(id) = id else {
                return Poster::default();
            };
            get(format!("https://vimeo.com/api/v2/video/{id}.json"))
                .await
                .and_then(|v| v.get(0).cloned())
                .map(|v| Poster {
                    thumb: text(&v, "thumbnail_large"),
                    title: text(&v, "title"),
                })
                .unwrap_or_default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_processors() {
        assert_eq!(
            processor_for_url("https://www.youtube.com/watch?v=abc&t=42"),
            Some(Processor::Youtube)
        );
        assert_eq!(processor_for_url("https://youtube.com/@channel"), None);
        assert_eq!(
            processor_for_url("https://vimeo.com/123"),
            Some(Processor::Vimeo)
        );
        assert_eq!(processor_for_url("https://example.com"), None);
        assert_eq!(
            youtube_path("https://www.youtube.com/watch?v=abc&t=42"),
            "abc?start=42"
        );
        assert_eq!(youtube_path("https://youtube.com/shorts/xyz"), "xyz");
        assert!(is_single_url(" https://a.b/c "));
        assert!(!is_single_url("https://a.b/c d"));
    }
}
