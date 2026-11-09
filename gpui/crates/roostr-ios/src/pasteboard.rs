//! The system pasteboard (`UIPasteboard.general`) as Rune clipboard content: plain text, HTML
//! (rich pastes between Roostr's editors, and from Safari or Notes) and PNG images.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_foundation::{NSArray, NSDictionary, NSString};
use objc2_ui_kit::UIPasteboard;
use rune::{ClipboardContent, ClipboardImage};

const TEXT: &str = "public.utf8-plain-text";
const HTML: &str = "public.html";
const PNG: &str = "public.png";

/// What the pasteboard holds, read for a paste the person asked for (reading it otherwise makes
/// iOS ask them to allow it). `None` when it holds nothing Rune pastes.
pub fn read() -> Option<ClipboardContent> {
    let board = UIPasteboard::generalPasteboard();
    // SAFETY: the person asked for this paste (see above).
    let text = unsafe { board.string() }.map(|text| text.to_string());
    let html = board
        .dataForPasteboardType(&NSString::from_str(HTML))
        .map(|data| String::from_utf8_lossy(&data.to_vec()).into_owned());
    let png = board
        .dataForPasteboardType(&NSString::from_str(PNG))
        .map(|data| data.to_vec())
        .or_else(|| {
            // SAFETY: as above.
            let image = unsafe { board.image() }?;
            Some(image.png_representation()?.to_vec())
        });
    let image = png.and_then(ClipboardImage::from_png);
    if text.is_none() && html.is_none() && image.is_none() {
        return None;
    }
    let mut content = ClipboardContent::new(text.unwrap_or_default());
    if let Some(html) = html {
        content = content.with_html(html);
    }
    if let Some(image) = image {
        content = content.with_image(image);
    }
    Some(content)
}

/// Puts `content` on the pasteboard (its text, and its HTML when it has some); `None` clears it.
pub fn write(content: Option<&ClipboardContent>) {
    let board = UIPasteboard::generalPasteboard();
    let mut items: Vec<Retained<NSDictionary<NSString, AnyObject>>> = Vec::new();
    if let Some(content) = content {
        let mut keys = vec![NSString::from_str(TEXT)];
        let mut values: Vec<Retained<AnyObject>> = vec![NSString::from_str(content.text()).into()];
        if let Some(html) = content.html() {
            keys.push(NSString::from_str(HTML));
            values.push(NSString::from_str(html).into());
        }
        let keys: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
        let values: Vec<&AnyObject> = values.iter().map(|v| &**v).collect();
        items.push(NSDictionary::from_slices(&keys, &values));
    }
    // SAFETY: each item maps UTI strings to NSStrings, property-list values.
    unsafe { board.setItems(&NSArray::from_retained_slice(&items)) };
}
