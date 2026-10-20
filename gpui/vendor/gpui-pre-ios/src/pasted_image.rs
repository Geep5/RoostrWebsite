//! roostr: which representation of a pasteboard item a paste reads as its
//! image. Outside `ios` so the host's tests run it.

use gpui::ImageFormat;

/// Image types a paste reads as they are: formats GPUI draws, on the phone,
/// the desktop and in the browser alike.
pub(crate) const AS_IS: [(&str, ImageFormat); 7] = [
    ("public.png", ImageFormat::Png),
    ("public.jpeg", ImageFormat::Jpeg),
    ("com.compuserve.gif", ImageFormat::Gif),
    ("org.webmproject.webp", ImageFormat::Webp),
    ("com.microsoft.bmp", ImageFormat::Bmp),
    ("com.microsoft.ico", ImageFormat::Ico),
    ("public.svg-image", ImageFormat::Svg),
];

/// Image types UIKit decodes but GPUI (or a browser other than Safari) does
/// not - a photo's HEIC above all, which Safari's "Copy" puts on the
/// pasteboard as it is: a paste redraws them as PNG or JPEG
/// ([`converted_format`]). UIKit's `hasImages` is false for HEIC, so a type
/// is looked for here rather than asked of it.
pub(crate) const CONVERTED: [&str; 6] = [
    "public.heic",
    "public.heif",
    "public.avif",
    "public.tiff",
    "public.jpeg-2000",
    "com.apple.icns",
];

/// How a paste reads a pasteboard item's image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PastedImage {
    /// The data under this type, as it is.
    AsIs(&'static str, ImageFormat),
    /// The data under this type, decoded by UIKit and re-encoded.
    Convert(&'static str),
}

/// The image representation to read from an item holding `types`: one GPUI
/// draws, else one to convert; `None` when the item holds no image.
pub(crate) fn pasted_image<T: AsRef<str>>(types: &[T]) -> Option<PastedImage> {
    let has = |uti: &str| types.iter().any(|t| t.as_ref() == uti);
    AS_IS
        .iter()
        .find(|(uti, _)| has(uti))
        .map(|&(uti, format)| PastedImage::AsIs(uti, format))
        .or_else(|| {
            CONVERTED
                .iter()
                .find(|uti| has(uti))
                .map(|&uti| PastedImage::Convert(uti))
        })
}

/// What a converted image is encoded as: PNG keeps its transparency; an
/// opaque one - a photo - is a JPEG, as the Photos app's own Copy makes it.
pub(crate) fn converted_format(has_alpha: bool) -> ImageFormat {
    if has_alpha {
        ImageFormat::Png
    } else {
        ImageFormat::Jpeg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_formats_are_read_as_they_are() {
        // The Photos app's Copy: a JPEG.
        assert_eq!(
            pasted_image(&["public.jpeg"]),
            Some(PastedImage::AsIs("public.jpeg", ImageFormat::Jpeg))
        );
        // Safari's Copy of a PNG: the image and its address.
        assert_eq!(
            pasted_image(&["public.png", "public.url"]),
            Some(PastedImage::AsIs("public.png", ImageFormat::Png))
        );
        // An app offering several: the first GPUI draws, never a conversion.
        assert_eq!(
            pasted_image(&["public.heic", "public.tiff", "public.jpeg"]),
            Some(PastedImage::AsIs("public.jpeg", ImageFormat::Jpeg))
        );
        assert_eq!(
            pasted_image(&["public.tiff", "public.png"]),
            Some(PastedImage::AsIs("public.png", ImageFormat::Png))
        );
    }

    #[test]
    fn heic_and_tiff_are_converted() {
        // Safari's Copy of a HEIC image (UIKit's hasImages is false for it).
        assert_eq!(
            pasted_image(&["public.heic", "public.url"]),
            Some(PastedImage::Convert("public.heic"))
        );
        assert_eq!(
            pasted_image(&["public.tiff"]),
            Some(PastedImage::Convert("public.tiff"))
        );
        assert_eq!(converted_format(false), ImageFormat::Jpeg);
        assert_eq!(converted_format(true), ImageFormat::Png);
    }

    #[test]
    fn text_and_links_hold_no_image() {
        assert_eq!(pasted_image::<&str>(&[]), None);
        assert_eq!(
            pasted_image(&["public.utf8-plain-text", "dev.zed.gpui.clipboard-metadata"]),
            None
        );
        assert_eq!(pasted_image(&["public.url"]), None);
    }
}
