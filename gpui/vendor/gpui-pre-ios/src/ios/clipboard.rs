//! roostr: the general pasteboard as GPUI's clipboard. The published crate
//! moved plain strings only; this keeps what the desktop platforms keep: a
//! string's metadata (an app's own copy, Roostr's copied blocks), a copied
//! link and images - every item's, so a copy of several photos pastes them
//! all.

use std::ptr::NonNull;

use block2::RcBlock;
use gpui::{ClipboardEntry, ClipboardItem, ClipboardString, Image, ImageFormat};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AllocAnyThread as _, Message as _};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGImage, CGImageAlphaInfo};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSIndexSet, NSString};
use objc2_ui_kit::{
    UIGraphicsImageRenderer, UIGraphicsImageRendererContext, UIGraphicsImageRendererFormat,
    UIGraphicsImageRendererFormatRange, UIImage, UIPasteboard,
};

use crate::pasted_image::{AS_IS, PastedImage, converted_format, pasted_image};

/// The pasteboard type of GPUI's plain text.
const TEXT_TYPE: &str = "public.utf8-plain-text";
/// The pasteboard type of a string's metadata, as UTF-8 next to its text.
const METADATA_TYPE: &str = "dev.zed.gpui.clipboard-metadata";
/// The quality a converted photo is encoded at.
const JPEG_QUALITY: f64 = 0.9;

/// The pasteboard type an image is copied under.
fn image_type(format: ImageFormat) -> Option<&'static str> {
    match format {
        ImageFormat::Tiff => Some("public.tiff"),
        _ => AS_IS
            .iter()
            .find(|(_, candidate)| *candidate == format)
            .map(|(uti, _)| *uti),
    }
}

/// Whether the pasteboard holds anything a paste can use. Asking reads only
/// its types, so iOS shows no paste prompt.
pub(super) fn has_content() -> bool {
    let pasteboard = UIPasteboard::generalPasteboard();
    let listed =
        unsafe { pasteboard.hasStrings() || pasteboard.hasURLs() || pasteboard.hasImages() };
    listed
        || item_types(&pasteboard)
            .iter()
            .any(|types| pasted_image(types).is_some())
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

/// The pasteboard's text (or link) with its metadata, and its images.
/// Reading another app's copy outside the edit menu's Paste and ⌘V shows
/// iOS's paste prompt.
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
    let images = read_images(&pasteboard);
    if images.is_empty() {
        // An image under a type neither list names: UIKit's own reading.
        if unsafe { pasteboard.hasImages() }
            && let Some(image) = unsafe { pasteboard.image() }.and_then(|image| converted(&image))
        {
            entries.push(ClipboardEntry::Image(image));
        }
    } else {
        entries.extend(images.into_iter().map(ClipboardEntry::Image));
    }
    (!entries.is_empty()).then_some(ClipboardItem { entries })
}

/// Every item's image, in pasteboard order.
fn read_images(pasteboard: &UIPasteboard) -> Vec<Image> {
    item_types(pasteboard)
        .iter()
        .enumerate()
        .filter_map(|(index, types)| {
            let (uti, format) = match pasted_image(types)? {
                PastedImage::AsIs(uti, format) => (uti, Some(format)),
                PastedImage::Convert(uti) => (uti, None),
            };
            let item = NSIndexSet::indexSetWithIndex(index);
            let data = pasteboard
                .dataForPasteboardType_inItemSet(&NSString::from_str(uti), Some(&item))?
                .firstObject()?;
            match format {
                Some(format) => Some(Image::from_bytes(format, data.to_vec())),
                None => converted(&*UIImage::imageWithData(&data)?),
            }
        })
        .collect()
}

/// Each item's types. Asking does not read the items.
fn item_types(pasteboard: &UIPasteboard) -> Vec<Vec<String>> {
    pasteboard
        .pasteboardTypesForItemSet(None)
        .map(|items| {
            items
                .iter()
                .map(|types| types.iter().map(|uti| uti.to_string()).collect())
                .collect()
        })
        .unwrap_or_default()
}

/// An image GPUI cannot draw - a photo's HEIC - drawn upright at its pixel
/// size in sRGB and encoded as [`converted_format`] says. Drawing applies
/// its orientation, so the bytes need no EXIF orientation to show the right
/// way up anywhere.
fn converted(image: &UIImage) -> Option<Image> {
    let has_alpha = unsafe { image.CGImage() }.is_some_and(|cg| {
        !matches!(
            CGImage::alpha_info(Some(&cg)),
            CGImageAlphaInfo::None
                | CGImageAlphaInfo::NoneSkipLast
                | CGImageAlphaInfo::NoneSkipFirst
        )
    });
    let format = converted_format(has_alpha);
    let (points, scale) = unsafe { (image.size(), image.scale()) };
    let size = CGSize::new(
        (points.width * scale).round(),
        (points.height * scale).round(),
    );
    if size.width < 1. || size.height < 1. {
        return None;
    }
    let renderer_format = UIGraphicsImageRendererFormat::new();
    renderer_format.setScale(1.);
    renderer_format.setOpaque(!has_alpha);
    renderer_format.setPreferredRange(UIGraphicsImageRendererFormatRange::Standard);
    let renderer = UIGraphicsImageRenderer::initWithSize_format(
        UIGraphicsImageRenderer::alloc(),
        size,
        &renderer_format,
    );
    let image = image.retain();
    let draw = RcBlock::new(move |_: NonNull<UIGraphicsImageRendererContext>| {
        image.drawInRect(CGRect::new(CGPoint::ZERO, size));
    });
    let actions = RcBlock::as_ptr(&draw);
    let data = unsafe {
        match format {
            ImageFormat::Png => renderer.PNGDataWithActions(actions),
            _ => renderer.JPEGDataWithCompressionQuality_actions(JPEG_QUALITY, actions),
        }
    };
    Some(Image::from_bytes(format, data.to_vec()))
}

/// Whether the pasteboard has `uti`, without reading it.
fn contains(pasteboard: &UIPasteboard, uti: &str) -> bool {
    pasteboard.containsPasteboardTypes(&NSArray::from_retained_slice(&[NSString::from_str(uti)]))
}
