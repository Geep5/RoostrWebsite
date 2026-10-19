//! `imageToAvatar` (engine/profile.ts): the avatar a profile picture upload
//! saves. The profile itself is `roostr_core::profile`.

use std::path::Path;

use anyhow::{Result, anyhow};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

/// A square 256 px JPEG (quality 0.85) data URL of the image's centred
/// square. Blocking.
pub fn image_to_avatar(path: &Path) -> Result<String> {
    let img = image::open(path).map_err(|_| anyhow!("unreadable image"))?;
    let side = img.width().min(img.height());
    let square = img
        .crop_imm(
            (img.width() - side) / 2,
            (img.height() - side) / 2,
            side,
            side,
        )
        .resize_exact(256, 256, image::imageops::FilterType::Triangle)
        .to_rgb8();
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85).encode_image(&square)?;
    Ok(format!("data:image/jpeg;base64,{}", STANDARD.encode(out)))
}
