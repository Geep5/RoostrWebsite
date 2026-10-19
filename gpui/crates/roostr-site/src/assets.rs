//! The site's images (the web app's `static/` files these pages use),
//! bundled into the binary and decoded off the main thread. Animated WebP
//! keeps every frame, so `img` plays it like the browser does.

use std::io::Cursor;
use std::sync::{Arc, LazyLock};

use gpui_kit::*;
use image::codecs::webp::WebPDecoder;
use image::{AnimationDecoder as _, Frame, Rgba};

/// A bundled image.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SiteImage {
    /// `static/logo.png` (the join page brand).
    Logo,
    /// `static/roostr-anim.webp` (landing hero, animated).
    RoosterAnim,
    /// The first frame of `roostr-anim.webp`: decodes in a fraction of the
    /// whole animation's time, so it stands in while that loads.
    RoosterFirstFrame,
    /// `static/roostr-poster.webp` (landing hero under reduced motion).
    RoosterPoster,
}

impl SiteImage {
    fn bytes(self) -> &'static [u8] {
        match self {
            Self::Logo => include_bytes!("../../../../static/logo.png"),
            Self::RoosterAnim | Self::RoosterFirstFrame => {
                include_bytes!("../../../../static/roostr-anim.webp")
            }
            Self::RoosterPoster => include_bytes!("../../../../static/roostr-poster.webp"),
        }
    }

    /// An `img` source that decodes in the background and re-renders the
    /// requesting view once ready.
    pub(crate) fn source(self) -> ImageSource {
        ImageSource::Custom(Arc::new(move |window, cx| {
            window.use_asset::<SiteImageLoader>(&self, cx)
        }))
    }
}

struct SiteImageLoader;

impl Asset for SiteImageLoader {
    type Source = SiteImage;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    // An `async fn` would capture the `&mut App` borrow; the trait wants a
    // `'static` future.
    #[allow(clippy::manual_async_fn)]
    fn load(source: SiteImage, _: &mut App) -> impl Future<Output = Self::Output> + Send + 'static {
        async move {
            let started = web_time::Instant::now();
            let max_frames = if source == SiteImage::RoosterFirstFrame {
                1
            } else {
                usize::MAX
            };
            let result =
                decode(source.bytes(), max_frames).map(|frames| Arc::new(RenderImage::new(frames)));
            match &result {
                Ok(_) => log::debug!("decoded {source:?} in {:?}", started.elapsed()),
                Err(error) => log::warn!("could not decode {source:?}: {error}"),
            }
            result
        }
    }
}

/// Decode to BGRA frames (GPUI's sprite layout), keeping WebP animation (up
/// to `max_frames` frames).
fn decode(bytes: &[u8], max_frames: usize) -> Result<Vec<Frame>, ImageCacheError> {
    let error = |e: image::ImageError| ImageCacheError::Other(Arc::new(anyhow::Error::new(e)));
    let mut frames = Vec::new();
    if image::guess_format(bytes).map_err(error)? == image::ImageFormat::WebP {
        let mut decoder = WebPDecoder::new(Cursor::new(bytes)).map_err(error)?;
        if decoder.has_animation() {
            let _ = decoder.set_background_color(Rgba([0, 0, 0, 0]));
            for frame in decoder.into_frames().take(max_frames) {
                frames.push(frame.map_err(error)?);
            }
        } else {
            let image = image::DynamicImage::from_decoder(decoder).map_err(error)?;
            frames.push(Frame::new(image.into_rgba8()));
        }
    } else {
        frames.push(Frame::new(
            image::load_from_memory(bytes).map_err(error)?.into_rgba8(),
        ));
    }
    for frame in &mut frames {
        for pixel in frame.buffer_mut().chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
    }
    Ok(frames)
}

/// The GitHub mark (`<svg viewBox="0 0 16 16">`, Octicons) in the button's
/// cream. GPUI rasterizes SVG images at their nominal size, so the file is
/// drawn at 2x (34px) and shown at 17px.
pub(crate) static GITHUB_MARK: LazyLock<Arc<Image>> = LazyLock::new(|| {
    Arc::new(Image::from_bytes(
        ImageFormat::Svg,
        include_bytes!("../assets/github.svg").to_vec(),
    ))
});
