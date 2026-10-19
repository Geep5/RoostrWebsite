//! roostr: the general pasteboard as GPUI's clipboard. The published crate
//! moved plain strings only; this keeps what the desktop platforms keep: a
//! string's metadata (an app's own copy, Roostr's copied blocks), a copied
//! link and images.

use gpui::{ClipboardEntry, ClipboardItem, ClipboardString, Image, ImageFormat};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSArray, NSData, NSDictionary, NSString};
use objc2_ui_kit::UIPasteboard;

/// The pasteboard type of GPUI's plain text.
const TEXT_TYPE: &str = "public.utf8-plain-text";
/// The pasteboard type of a string's metadata, as UTF-8 next to its text.
const METADATA_TYPE: &str = "dev.zed.gpui.clipboard-metadata";
/// Image types a paste reads as they are; any other image (a photo's HEIC)
/// is read as PNG.
const IMAGE_TYPES: [(&str, ImageFormat); 7] = [
    ("public.png", ImageFormat::Png),
    ("public.jpeg", ImageFormat::Jpeg),
    ("com.compuserve.gif", ImageFormat::Gif),
    ("org.webmproject.webp", ImageFormat::Webp),
    ("public.tiff", ImageFormat::Tiff),
    ("com.microsoft.bmp", ImageFormat::Bmp),
    ("public.svg-image", ImageFormat::Svg),
];

fn image_type(format: ImageFormat) -> Option<&'static str> {
    IMAGE_TYPES
        .iter()
        .find(|(_, candidate)| *candidate == format)
        .map(|(uti, _)| *uti)
}

/// Whether the pasteboard holds anything a paste can use. Asking does not
/// read it, so iOS shows no paste prompt.
pub(super) fn has_content() -> bool {
    let pasteboard = UIPasteboard::generalPasteboard();
    unsafe { pasteboard.hasStrings() || pasteboard.hasURLs() || pasteboard.hasImages() }
}

/// Replaces the pasteboard's contents with one item holding every entry.
pub(super) fn write(item: &ClipboardItem) {
    let mut types: Vec<Retained<NSString>> = Vec::new();
    let mut values: Vec<Retained<AnyObject>> = Vec::new();
    if let Some(text) = item.text() {
        types.push(NSString::from_str(TEXT_TYPE));
        values.push(NSString::from_str(&text).into());
    }
    let metadata = item.entries().iter().find_map(|entry| match entry {
        ClipboardEntry::String(string) => string.metadata.as_deref(),
        _ => None,
    });
    if let Some(metadata) = metadata {
        types.push(NSString::from_str(METADATA_TYPE));
        values.push(NSData::with_bytes(metadata.as_bytes()).into());
    }
    for entry in item.entries() {
        if let ClipboardEntry::Image(image) = entry
            && let Some(uti) = image_type(image.format())
        {
            types.push(NSString::from_str(uti));
            values.push(NSData::with_bytes(image.bytes()).into());
        }
    }
    if types.is_empty() {
        return;
    }
    let types: Vec<&NSString> = types.iter().map(|uti| &**uti).collect();
    let item = NSDictionary::from_retained_objects(&types, &values);
    unsafe { UIPasteboard::generalPasteboard().setItems(&NSArray::from_retained_slice(&[item])) };
}

/// The pasteboard's text (or link) with its metadata, and its image. Reading
/// another app's copy outside the edit menu's Paste and ⌘V shows iOS's paste
/// prompt.
pub(super) fn read() -> Option<ClipboardItem> {
    let pasteboard = UIPasteboard::generalPasteboard();
    let mut entries = Vec::new();
    let text = unsafe {
        if pasteboard.hasStrings() {
            pasteboard.string().map(|text| text.to_string())
        } else if pasteboard.hasURLs() {
            pasteboard
                .URL()
                .and_then(|url| url.absoluteString())
                .map(|url| url.to_string())
        } else {
            None
        }
    };
    if let Some(text) = text {
        let metadata = contains(&pasteboard, METADATA_TYPE)
            .then(|| pasteboard.dataForPasteboardType(&NSString::from_str(METADATA_TYPE)))
            .flatten()
            .and_then(|data| String::from_utf8(data.to_vec()).ok());
        entries.push(ClipboardEntry::String(ClipboardString { text, metadata }));
    }
    if unsafe { pasteboard.hasImages() }
        && let Some(image) = read_image(&pasteboard)
    {
        entries.push(ClipboardEntry::Image(image));
    }
    (!entries.is_empty()).then_some(ClipboardItem { entries })
}

fn read_image(pasteboard: &UIPasteboard) -> Option<Image> {
    for (uti, format) in IMAGE_TYPES {
        if contains(pasteboard, uti)
            && let Some(data) = pasteboard.dataForPasteboardType(&NSString::from_str(uti))
        {
            return Some(Image::from_bytes(format, data.to_vec()));
        }
    }
    let image = unsafe { pasteboard.image() }?;
    let png = image.png_representation()?;
    Some(Image::from_bytes(ImageFormat::Png, png.to_vec()))
}

/// Whether the pasteboard has `uti`, without reading it.
fn contains(pasteboard: &UIPasteboard, uti: &str) -> bool {
    pasteboard.containsPasteboardTypes(&NSArray::from_retained_slice(&[NSString::from_str(uti)]))
}
