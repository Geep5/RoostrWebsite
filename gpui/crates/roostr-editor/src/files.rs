//! File objects (`src/lib/files.ts`). A file's bytes live on Blossom,
//! encrypted (see `roostr_core::blossom`): the File object names the blob
//! (`blob_sha256`, `blob_url`) and holds its key (`blob_key`). Files added
//! before Blossom name only `file_hash`: their bytes come from this
//! computer's harness, peer-to-peer. With a daemon, files are added through
//! the harness (`POST /files`), which also uploads them to Blossom; this
//! device's own replica encrypts and uploads them itself.

use std::path::Path;
#[cfg(not(target_family = "wasm"))]
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(not(target_family = "wasm"))]
use anyhow::Context as _;
use anyhow::{Result, anyhow};
use gpui_kit::{App, ClipboardItem, Image, ImageFormat, Task};
#[cfg(not(target_family = "wasm"))]
use gpui_kit::{AppContext as _, PathPromptOptions};
use roostr_core::Store;
use roostr_core::blossom;
use roostr_core::harness::{self, Harness};
use roostr_core::io;
use roostr_core::types::{Fields, Value, field_str};
use serde::Deserialize;

const NOT_RUNNING_TO_VIEW: &str = "Start Roostr on this computer to see this file.";

/// A file the person picked, dropped or pasted.
#[derive(Clone, Debug)]
pub struct PickedFile {
    pub name: String,
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// What the harness answers for an upload.
#[derive(Clone, Debug, Deserialize)]
pub struct Uploaded {
    pub id: String,
    #[serde(default)]
    pub hash: String,
}

/// Why `name` could not be added, as the person should read it.
pub fn add_failure_text(err: &anyhow::Error, name: &str) -> String {
    roostr_core::create::add_failure_text(err, name)
}

/// Why a file's bytes could not be shown: Roostr out of reach here, else the harness's reason.
pub fn view_failure_text(err: &anyhow::Error) -> String {
    if harness::is_unreachable(err) {
        return NOT_RUNNING_TO_VIEW.to_string();
    }
    format!("{err:#}")
}

/// The browser's `File.type` for a file name (by extension).
pub fn mime_for(name: &str) -> String {
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "ico" => "image/vnd.microsoft.icon",
        "heic" => "image/heic",
        "avif" => "image/avif",
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "md" | "markdown" => "text/markdown",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "text/javascript",
        "ts" => "video/mp2t",
        "json" => "application/json",
        "xml" => "application/xml",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "tar" => "application/x-tar",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "m4a" => "audio/mp4",
        "flac" => "audio/flac",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "rtf" => "application/rtf",
        _ => "",
    }
    .to_string()
}

/// Read a file from disk as a picked file.
#[cfg(not(target_family = "wasm"))]
pub fn read_path(path: &Path) -> Result<PickedFile> {
    let bytes =
        std::fs::read(path).with_context(|| format!("Could not read {}", path.display()))?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_string();
    Ok(PickedFile {
        mime: mime_for(&name),
        name,
        bytes,
    })
}

/// Whether this platform can open a file picker ([`pick_file`]); the
/// browser cannot ([`io::NO_FILE_PICKER`]).
pub const CAN_PICK_FILES: bool = !io::WEB;

/// Open the system file picker (`pickFile`). Resolves `None` when cancelled,
/// and at once where there is no picker ([`CAN_PICK_FILES`]).
#[cfg(target_family = "wasm")]
pub fn pick_file(_cx: &App) -> Task<Option<PickedFile>> {
    Task::ready(None)
}

/// Open the system file picker (`pickFile`). Resolves `None` when cancelled,
/// and at once where there is no picker ([`CAN_PICK_FILES`]).
#[cfg(not(target_family = "wasm"))]
pub fn pick_file(cx: &App) -> Task<Option<PickedFile>> {
    let rx = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: None,
    });
    cx.background_spawn(async move {
        let path: PathBuf = rx.await.ok()?.ok()??.into_iter().next()?;
        match read_path(&path) {
            Ok(file) => Some(file),
            Err(error) => {
                log::warn!("{error:#}");
                None
            }
        }
    })
}

/// A pasted clipboard image as a file: screenshots arrive with no real name.
pub fn pasted_image(image: &Image) -> PickedFile {
    let stamp = chrono::Local::now().format("%-m/%-d/%Y, %-I:%M:%S %p");
    PickedFile {
        name: format!("Pasted image {stamp}.{}", image.format.extension()),
        mime: image.format.mime_type().to_string(),
        bytes: image.bytes.clone(),
    }
}

/// Store the bytes on this computer's harness, which creates the File object in `space_id`.
pub async fn upload_to_harness(
    harness: &Harness,
    file: &PickedFile,
    space_id: &str,
) -> Result<Uploaded> {
    if file.bytes.is_empty() {
        return Err(anyhow!("The file is empty."));
    }
    let enc = |s: &str| {
        percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
    };
    let path = format!(
        "/files?name={}&mime={}&space={}",
        enc(&file.name),
        enc(&file.mime),
        enc(space_id)
    );
    let json = harness
        .post_bytes(&path, &file.bytes, "application/octet-stream")
        .await?;
    Ok(serde_json::from_value(json)?)
}

/// Add `file` to `space_id` (files.ts `uploadFile`): through this
/// computer's harness when the app runs on a daemon, else - this device's
/// own replica - encrypted straight to Blossom (`uploadToBlossom`).
pub fn upload_file(file: PickedFile, space_id: String, cx: &mut App) -> Task<Result<Uploaded>> {
    if let Some(harness) = harness::harness(cx) {
        return io::spawn(cx, async move {
            upload_to_harness(&harness, &file, &space_id).await
        });
    }
    let Some(account) = roostr_core::account::account(cx) else {
        return Task::ready(Err(anyhow::Error::new(harness::Unreachable(
            "no harness".into(),
        ))));
    };
    let relays = relays(cx);
    let name = file.name.clone();
    let mime = file.mime.clone();
    let uploaded = io::spawn(cx, async move {
        upload_to_blossom(&*account, &file, &relays).await
    });
    cx.spawn(async move |cx| {
        let blob = uploaded.await?;
        let mut fields = Fields::new();
        fields.insert("file_hash".into(), Value::string(&blob.hash));
        fields.insert("file_size".into(), Value::int(blob.size as i64));
        fields.insert("file_mime".into(), Value::string(&mime));
        fields.insert("blob_sha256".into(), Value::string(&blob.blob_sha256));
        fields.insert("blob_key".into(), Value::string(&blob.blob_key));
        fields.insert("blob_url".into(), Value::string(&blob.blob_url));
        if !space_id.is_empty() {
            fields.insert("channel".into(), Value::string(&space_id));
        }
        let title = if name.is_empty() {
            "Untitled file"
        } else {
            name.as_str()
        };
        let id = cx
            .update(|cx| roostr_core::api::note::create(title, Some("file"), Some(fields), cx))
            .await?;
        Ok(Uploaded {
            id,
            hash: blob.hash,
        })
    })
}

/// A file's bytes, encrypted and on Blossom.
struct StoredBlob {
    hash: String,
    size: usize,
    blob_sha256: String,
    blob_key: String,
    blob_url: String,
}

/// Encrypt `file` under a fresh key and upload it to the relays' Blossom
/// server, authorized by this identity (`uploadToBlossom`, `uploadBlob`).
async fn upload_to_blossom(
    account: &dyn roostr_core::account::Account,
    file: &PickedFile,
    relays: &[String],
) -> Result<StoredBlob> {
    if file.bytes.is_empty() {
        return Err(anyhow!("The file is empty."));
    }
    let mut key = [0u8; 32];
    key[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    key[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    let blob = blossom::encrypt_blob(&file.bytes, &key)?;
    let hash = blossom::sha256_hex(&file.bytes);
    let blob_sha256 = blossom::sha256_hex(&blob);
    let authorization = account.blossom_upload_authorization(&blob_sha256, &file.name)?;
    let server = blossom::blossom_server(relays).await;
    let blob_url = blossom::upload_blob(&server, &blob, &blob_sha256, &authorization).await?;
    Ok(StoredBlob {
        hash,
        size: file.bytes.len(),
        blob_sha256,
        blob_key: key.iter().map(|byte| format!("{byte:02x}")).collect(),
        blob_url,
    })
}

/// The bytes of a File object (its fields): from Blossom when it names a
/// blob, else from this computer's harness. Returns (bytes, mime).
pub async fn fetch_file_blob(
    fields: &Fields,
    relays: &[String],
    harness: Option<&Harness>,
) -> Result<(Vec<u8>, String)> {
    let mime = match field_str(fields, "file_mime") {
        "" => "application/octet-stream".to_string(),
        m => m.to_string(),
    };
    let blob_hash = field_str(fields, "blob_sha256").to_lowercase();
    if !blob_hash.is_empty() {
        let key = hex::decode(field_str(fields, "blob_key"))
            .map_err(|_| anyhow!("a blob key is 32 bytes"))?;
        let named = field_str(fields, "blob_url").to_string();
        let fallback = format!("{}/{blob_hash}", blossom::blossom_server(relays).await);
        let blob = match blossom::download_blob(
            if named.is_empty() { &fallback } else { &named },
            &blob_hash,
        )
        .await
        {
            Ok(b) => b,
            // The descriptor's server may have moved; the configured one serves the same hash.
            Err(err) if !named.is_empty() && named != fallback => {
                log::info!("blob url failed ({err:#}); trying {fallback}");
                blossom::download_blob(&fallback, &blob_hash).await?
            }
            Err(err) => return Err(err),
        };
        return Ok((blossom::decrypt_blob(&blob, &key)?, mime));
    }
    let hash = field_str(fields, "file_hash").to_lowercase();
    let harness =
        harness.ok_or_else(|| anyhow::Error::new(harness::Unreachable("no harness".into())))?;
    let enc = percent_encoding::utf8_percent_encode(&hash, percent_encoding::NON_ALPHANUMERIC)
        .to_string();
    Ok((harness.get_bytes(&format!("/files/{enc}")).await?, mime))
}

/// The bytes a File block shows: its File object's, else (the File not
/// synced here yet) the harness's by the block's hash. Resolves (bytes, mime).
pub fn file_block_blob(
    meta: &std::collections::BTreeMap<String, String>,
    cx: &App,
) -> Task<Result<(Vec<u8>, String)>> {
    let file_id = meta.get("fileId").cloned().unwrap_or_default();
    let fallback_hash = meta.get("hash").cloned().unwrap_or_default();
    let fetch =
        (!file_id.is_empty()).then(|| Store::global(cx).read(cx).fetch_object(&file_id, cx));
    let relays = relays(cx);
    let harness = harness::harness(cx);
    io::spawn(cx, async move {
        let fields = match fetch {
            Some(task) => task.await.ok().map(|o| o.fields),
            None => None,
        };
        let fields = fields.unwrap_or_else(|| {
            let mut f = Fields::new();
            f.insert(
                "file_hash".into(),
                roostr_core::types::Value::string(fallback_hash),
            );
            f
        });
        fetch_file_blob(&fields, &relays, harness.as_deref()).await
    })
}

/// A File object's bytes as an image (a page's cover, a generated try):
/// its fields, then its bytes the usual way.
pub fn file_image(file_id: &str, cx: &App) -> Task<Result<Arc<Image>>> {
    let fetch = Store::global(cx).read(cx).fetch_object(file_id, cx);
    let relays = relays(cx);
    let harness = harness::harness(cx);
    io::spawn(cx, async move {
        let fields = fetch.await?.fields;
        let (bytes, mime) = fetch_file_blob(&fields, &relays, harness.as_deref()).await?;
        let format =
            ImageFormat::from_mime_type(&mime).ok_or_else(|| anyhow!("{mime} is not an image"))?;
        Ok(Arc::new(Image::from_bytes(format, bytes)))
    })
}

/// The identity's relays (Blossom server discovery).
pub fn relays(cx: &App) -> Vec<String> {
    Store::global(cx)
        .read(cx)
        .settings
        .as_ref()
        .map(|s| s.relays.clone())
        .unwrap_or_default()
}

/// Save bytes under `name`: the browser's download.
#[cfg(target_family = "wasm")]
pub fn save_bytes(bytes: Vec<u8>, name: String, _cx: &App) -> Task<Result<()>> {
    let mime = mime_for(&name);
    let mime = if mime.is_empty() {
        "application/octet-stream"
    } else {
        &mime
    };
    Task::ready(roostr_core::web::download(&bytes, &name, mime))
}

/// Save bytes under `name` where the person chooses (the browser's download).
#[cfg(not(target_family = "wasm"))]
pub fn save_bytes(bytes: Vec<u8>, name: String, cx: &App) -> Task<Result<()>> {
    let dir = dirs::download_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    let rx = cx.prompt_for_new_path(&dir, Some(&name));
    cx.background_spawn(async move {
        let Some(path) = rx.await.map_err(|_| anyhow!("cancelled"))?? else {
            return Ok(());
        };
        std::fs::write(&path, bytes).with_context(|| format!("Could not save {}", path.display()))
    })
}

/// Save a File block's bytes under its name (`downloadFileBlock`).
pub fn download_file_block(
    meta: &std::collections::BTreeMap<String, String>,
    name: String,
    cx: &mut App,
) -> Task<Result<()>> {
    let blob = file_block_blob(meta, cx);
    cx.spawn(async move |cx| {
        let (bytes, _) = blob.await?;
        let save = cx.update(|cx| save_bytes(bytes, name, cx));
        save.await
    })
}

/// Put an image block on the clipboard (`copyImageBlock`).
pub fn copy_image_block(
    meta: &std::collections::BTreeMap<String, String>,
    cx: &mut App,
) -> Task<Result<()>> {
    let blob = file_block_blob(meta, cx);
    cx.spawn(async move |cx| {
        let (bytes, mime) = blob.await?;
        let format = ImageFormat::from_mime_type(&mime)
            .ok_or_else(|| anyhow!("could not convert the image to PNG"))?;
        cx.update(|cx| {
            cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(format, bytes)))
        });
        Ok(())
    })
}

/// Show the bytes in a new browser tab (the browser's viewer stands in for
/// the system's).
#[cfg(target_family = "wasm")]
pub fn open_externally(bytes: Arc<Vec<u8>>, name: &str, _cx: &App) -> Task<Result<()>> {
    let mime = mime_for(name);
    let mime = if mime.is_empty() {
        "application/octet-stream"
    } else {
        &mime
    };
    Task::ready(roostr_core::web::open_bytes(&bytes, mime))
}

/// Write bytes to a temp file named like the file and open it in its default app.
#[cfg(not(target_family = "wasm"))]
pub fn open_externally(bytes: Arc<Vec<u8>>, name: &str, cx: &App) -> Task<Result<()>> {
    let dir = std::env::temp_dir().join("roostr-files");
    let path = dir.join(sanitize(name));
    let task = cx.background_spawn({
        let path = path.clone();
        async move {
            std::fs::create_dir_all(&dir)?;
            std::fs::write(&path, bytes.as_slice())?;
            anyhow::Ok(())
        }
    });
    cx.spawn(async move |cx| {
        task.await?;
        cx.update(|cx| cx.open_with_system(&path));
        Ok(())
    })
}

#[cfg(not(target_family = "wasm"))]
fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '\0') {
                '_'
            } else {
                c
            }
        })
        .collect();
    if cleaned.trim().is_empty() {
        "file".into()
    } else {
        cleaned
    }
}

/// Bytes as the header shows them: B, KB, MB, GB (1024-based).
pub fn human_size(bytes: f64) -> String {
    if !bytes.is_finite() || bytes < 0. {
        return String::new();
    }
    if bytes < 1024. {
        return format!("{bytes} B");
    }
    let units = ["KB", "MB", "GB", "TB"];
    let mut value = bytes / 1024.;
    let mut unit = 0;
    while value >= 1024. && unit < units.len() - 1 {
        value /= 1024.;
        unit += 1;
    }
    if value < 10. {
        format!("{value:.1} {}", units[unit])
    } else {
        format!("{} {}", value.round(), units[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(human_size(0.), "0 B");
        assert_eq!(human_size(1536.), "1.5 KB");
        assert_eq!(human_size(200. * 1024.), "200 KB");
        assert_eq!(human_size(5. * 1024. * 1024.), "5.0 MB");
    }
}
