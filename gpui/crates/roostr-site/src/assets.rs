//! The site's images (the web app's `static/` files these pages use), bundled into the binary.
//! The landing hero is an animated WebP: every frame is decoded here and played through one
//! [`rune::Pixels`] on the state's executor, as the browser plays the file.

use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use image::codecs::webp::WebPDecoder;
use image::{AnimationDecoder as _, Rgba};
use roostr_core::State;
use roostr_rt::Task;

/// `static/logo.png` (the join page brand).
pub(crate) static LOGO: &[u8] = include_bytes!("../../../../static/logo.png");
/// The GitHub mark (`<svg viewBox="0 0 16 16">`, Octicons) in the button's cream.
pub(crate) static GITHUB_MARK: &[u8] = include_bytes!("../assets/github.svg");
/// `static/roostr-anim.webp` (landing hero, 60 frames of 900×900).
static ANIM: &[u8] = include_bytes!("../../../../static/roostr-anim.webp");
/// `static/roostr-poster.webp` (landing hero under reduced motion, and if the animation fails).
static POSTER: &[u8] = include_bytes!("../../../../static/roostr-poster.webp");

/// One decoded frame: straight-alpha sRGB pixels and how long it shows.
pub(crate) struct Frame {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    delay: Duration,
}

/// The landing hero (`#rooster`): what it draws, and the animation playing into it.
#[derive(Default)]
pub(crate) struct Hero {
    /// The image the hero element draws.
    pub(crate) pixels: rune::Pixels,
    /// What the hero shows: `Some(true)` the poster (reduced motion), `Some(false)` the
    /// animation, `None` nothing yet (or stopped).
    shown: Option<bool>,
    /// Every frame of the animation, once decoded.
    frames: Option<Arc<[Frame]>>,
    /// Decoding the animation, then playing it; dropping it stops both.
    play: Option<Task<()>>,
}

/// The hero is on screen: show the poster when `reduced` (reduced motion), else play the
/// animation. Its first frame stands in at once while the rest decode off the main thread
/// (the poster if it fails to decode).
pub(crate) fn show_hero(s: &mut State, reduced: bool) {
    let hero = s.slot::<Hero>();
    if hero.shown == Some(reduced) {
        return;
    }
    hero.shown = Some(reduced);
    hero.play = None;
    if reduced {
        show_still(&hero.pixels, POSTER);
        return;
    }
    let pixels = hero.pixels.clone();
    if let Some(frames) = hero.frames.clone() {
        let task = play(s, pixels, frames);
        s.slot::<Hero>().play = Some(task);
        return;
    }
    if pixels.size() == (0, 0) {
        show_still(&pixels, ANIM);
    }
    let decode = s.rt.background(async { decode_frames(ANIM) });
    let task = s.rt.spawn(async move |cx| {
        let frames = match decode.await {
            Ok(frames) if !frames.is_empty() => Arc::<[Frame]>::from(frames),
            result => {
                if let Err(error) = result {
                    log::warn!("could not decode the landing animation: {error}");
                }
                show_still(&pixels, POSTER);
                return;
            }
        };
        let play = cx.update(|s| {
            s.slot::<Hero>().frames = Some(frames.clone());
            play(s, pixels, frames)
        });
        play.await;
    });
    s.slot::<Hero>().play = Some(task);
}

/// Before the hero first draws: its still (the poster when `reduced`, else the animation's
/// first frame), so the first frame of the page already shows the rooster. A no-op once the
/// hero has an image.
pub(crate) fn prime_hero(hero: &Hero, reduced: bool) {
    if hero.pixels.size() == (0, 0) {
        show_still(&hero.pixels, if reduced { POSTER } else { ANIM });
    }
}

/// The landing is gone: stop the animation (its frames stay decoded for the next visit).
pub(crate) fn stop_hero(s: &mut State) {
    let hero = s.slot::<Hero>();
    hero.play = None;
    hero.shown = None;
}

/// Loop `frames` into `pixels`, each for its delay, until the task is dropped.
fn play(s: &State, pixels: rune::Pixels, frames: Arc<[Frame]>) -> Task<()> {
    s.rt.spawn(async move |cx| {
        let mut ix = 0;
        loop {
            let frame = &frames[ix];
            pixels.set(frame.width, frame.height, frame.rgba.clone());
            cx.timer(frame.delay).await;
            ix = (ix + 1) % frames.len();
        }
    })
}

/// Show the first frame of `bytes` (a WebP) in `pixels`.
fn show_still(pixels: &rune::Pixels, bytes: &[u8]) {
    match decode(bytes, 1) {
        Ok(mut frames) if !frames.is_empty() => {
            let f = frames.swap_remove(0);
            pixels.set(f.width, f.height, f.rgba);
        }
        Ok(_) => log::warn!("the landing image has no frames"),
        Err(error) => log::warn!("could not decode the landing image: {error}"),
    }
}

/// Every frame of `bytes`.
fn decode_frames(bytes: &[u8]) -> Result<Vec<Frame>, image::ImageError> {
    decode(bytes, usize::MAX)
}

/// Decode a WebP to straight-alpha RGBA frames, keeping its animation (up to `max_frames`).
fn decode(bytes: &[u8], max_frames: usize) -> Result<Vec<Frame>, image::ImageError> {
    let mut decoder = WebPDecoder::new(Cursor::new(bytes))?;
    if !decoder.has_animation() {
        let image = image::DynamicImage::from_decoder(decoder)?.into_rgba8();
        return Ok(vec![Frame {
            width: image.width(),
            height: image.height(),
            rgba: image.into_raw(),
            delay: Duration::ZERO,
        }]);
    }
    decoder.set_background_color(Rgba([0, 0, 0, 0]))?;
    let mut frames = Vec::new();
    for frame in decoder.into_frames().take(max_frames) {
        let frame = frame?;
        let delay = Duration::from(frame.delay());
        let buffer = frame.into_buffer();
        frames.push(Frame {
            width: buffer.width(),
            height: buffer.height(),
            rgba: buffer.into_raw(),
            delay,
        });
    }
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hero shows the first frame at once, plays every frame once they decode, and stops
    /// when the landing is left.
    #[test]
    fn the_hero_plays_and_stops() {
        let prefs =
            std::env::temp_dir().join(format!("roostr-site-unit-{}.json", std::process::id()));
        // SAFETY: the only test in this binary that reads the environment.
        unsafe { std::env::set_var("ROOSTR_UI_STATE", prefs) };
        let mut s = State::unopened();
        crate::register(&mut s);
        roostr_core::nav::navigate(
            roostr_core::nav::Route::Site(roostr_core::nav::SitePage::Landing),
            &mut s,
        );
        let (woke, wakes) = std::sync::mpsc::channel::<()>();
        let woke = parking_lot::Mutex::new(woke);
        s.rt.set_waker(Arc::new(move || {
            let _ = woke.lock().send(());
        }));
        show_hero(&mut s, false);
        assert_eq!(
            s.slot::<Hero>().pixels.size(),
            (900, 900),
            "the first frame stands in"
        );
        while s.slot::<Hero>().frames.is_none() {
            s.poll();
            if s.slot::<Hero>().frames.is_none() {
                wakes.recv().expect("the decode wakes the executor");
            }
        }
        assert_eq!(s.slot::<Hero>().frames.as_ref().map(|f| f.len()), Some(60));
        assert!(s.slot::<Hero>().play.is_some(), "playing");
        roostr_core::nav::navigate(
            roostr_core::nav::Route::Site(roostr_core::nav::SitePage::Guide),
            &mut s,
        );
        s.poll();
        assert!(s.slot::<Hero>().play.is_none(), "stopped off the landing");
        // Back on the landing it plays the decoded frames again, without decoding.
        show_hero(&mut s, false);
        assert!(s.slot::<Hero>().play.is_some());
    }

    #[test]
    fn the_bundled_images_decode() {
        let cache = rune::ImageCache::new(std::sync::Arc::new(|| {}));
        assert!(cache.load(LOGO).wait().is_ok());
        assert!(cache.svg(GITHUB_MARK, 17.0, 2.0).wait().is_ok());
    }

    #[test]
    fn the_hero_animation_has_its_sixty_frames() {
        let frames = decode_frames(ANIM).expect("decodes");
        assert_eq!(frames.len(), 60);
        assert!(frames.iter().all(|f| f.width == 900 && f.height == 900));
        assert!(frames.iter().all(|f| f.delay > Duration::ZERO));
        let poster = decode(POSTER, 1).expect("poster decodes");
        assert_eq!(poster.len(), 1);
    }
}
