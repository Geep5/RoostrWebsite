//! File objects (`src/lib/files.ts`). A file's bytes live on Blossom,
//! encrypted (see `roostr_core::blossom`): the File object names the blob
//! (`blob_sha256`, `blob_url`) and holds its key (`blob_key`). Files added
//! before Blossom name only `file_hash`: their bytes come from this
//! computer's harness, peer-to-peer. With a daemon, files are added through
//! the harness (`POST /files`), which also uploads them to Blossom; this
//! device's own replica encrypts and uploads them itself.

use std::collections::BTreeMap;
use std::path::Path;
#[cfg(not(target_family = "wasm"))]
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(not(target_family = "wasm"))]
use anyhow::Context as _;
use anyhow::{Result, anyhow};
use roostr_core::State;
use roostr_core::blossom;
use roostr_core::harness::{self, Harness};
use roostr_core::io;
use roostr_core::types::{Fields, Value, field_str};
use roostr_rt::Task;
use rune::Tx;
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
pub fn pick_file(_s: &mut State) -> Task<Option<PickedFile>> {
    Task::ready(None)
}

/// Open the system file picker (`pickFile`, the app's [`State::pick_file`]
/// hook) and read the chosen file. Resolves `None` when cancelled, when the
/// file can't be read, and at once when the app installed no picker.
#[cfg(not(target_family = "wasm"))]
pub fn pick_file(s: &mut State) -> Task<Option<PickedFile>> {
    let Some(roostr_core::create::PickFile(pick)) = s.pick_file.clone() else {
        return Task::ready(None);
    };
    let chosen = pick(s);
    s.rt.spawn(async move |cx| {
        let path: PathBuf = chosen.await?;
        cx.background(async move {
            match read_path(&path) {
                Ok(file) => Some(file),
                Err(error) => {
                    log::warn!("{error:#}");
                    None
                }
            }
        })
        .await
    })
}

/// The name a pasted image gets: screenshots arrive with no real name.
fn pasted_name(extension: &str) -> String {
    let stamp = chrono::Local::now().format("%-m/%-d/%Y, %-I:%M:%S %p");
    format!("Pasted image {stamp}.{extension}")
}

/// A pasted clipboard image as a file (Rune hands every pasted image over as PNG).
pub fn pasted_image(image: &rune::ClipboardImage) -> PickedFile {
    pasted_png(image.png().to_vec())
}

/// PNG bytes from the clipboard as a file.
pub fn pasted_png(bytes: Vec<u8>) -> PickedFile {
    PickedFile {
        name: pasted_name("png"),
        mime: "image/png".to_string(),
        bytes,
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
pub fn upload_file(file: PickedFile, space_id: String, s: &mut State) -> Task<Result<Uploaded>> {
    if let Some(harness) = s.harness.clone() {
        return s
            .rt
            .background(async move { upload_to_harness(&harness, &file, &space_id).await });
    }
    let Some(account) = roostr_core::account::account(s) else {
        return Task::ready(Err(anyhow::Error::new(harness::Unreachable(
            "no harness".into(),
        ))));
    };
    let relays = relays(s);
    let name = file.name.clone();
    let mime = file.mime.clone();
    let uploaded =
        s.rt.background(async move { upload_to_blossom(&*account, &file, &relays).await });
    s.rt.spawn(async move |cx| {
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
            .update(|s| roostr_core::api::note::create(title, Some("file"), Some(fields), s))
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
    meta: &BTreeMap<String, String>,
    s: &State,
) -> Task<Result<(Vec<u8>, String)>> {
    let file_id = meta.get("fileId").cloned().unwrap_or_default();
    let fallback_hash = meta.get("hash").cloned().unwrap_or_default();
    let fetch = (!file_id.is_empty()).then(|| s.store.fetch_object(&file_id));
    let relays = relays(s);
    let harness = s.harness.clone();
    s.rt.spawn(async move |cx| {
        let fields = match fetch {
            Some(task) => task.await.ok().map(|o| o.fields),
            None => None,
        };
        let fields = fields.unwrap_or_else(|| {
            let mut f = Fields::new();
            f.insert("file_hash".into(), Value::string(fallback_hash));
            f
        });
        cx.background(async move { fetch_file_blob(&fields, &relays, harness.as_deref()).await })
            .await
    })
}

/// A File object's bytes for an image (a page's cover, a generated try):
/// its fields, then its bytes the usual way. Resolves (bytes, mime); an
/// error when the File is not an image.
pub fn file_image(file_id: &str, s: &State) -> Task<Result<(Vec<u8>, String)>> {
    let fetch = s.store.fetch_object(file_id);
    let relays = relays(s);
    let harness = s.harness.clone();
    s.rt.spawn(async move |cx| {
        let fields = fetch.await?.fields;
        let (bytes, mime) = cx
            .background(async move { fetch_file_blob(&fields, &relays, harness.as_deref()).await })
            .await?;
        if !mime.to_ascii_lowercase().starts_with("image/") {
            return Err(anyhow!("{mime} is not an image"));
        }
        Ok((bytes, mime))
    })
}

/// The identity's relays (Blossom server discovery).
pub fn relays(s: &State) -> Vec<String> {
    s.store
        .settings
        .as_ref()
        .map(|settings| settings.relays.clone())
        .unwrap_or_default()
}

/// `name`'s MIME type, `application/octet-stream` when unknown.
#[cfg(target_family = "wasm")]
fn download_mime(name: &str) -> String {
    let mime = mime_for(name);
    if mime.is_empty() {
        "application/octet-stream".to_string()
    } else {
        mime
    }
}

/// Save bytes under `name`: the browser's download.
#[cfg(target_family = "wasm")]
pub fn save_bytes(bytes: Vec<u8>, name: String, _tx: &mut Tx) {
    if let Err(error) = roostr_core::web::download(&bytes, &name, &download_mime(&name)) {
        log::error!("{error:#}");
    }
}

/// Save bytes under `name` where the person chooses: the system's Save
/// dialog, opened in the Downloads folder with the name filled in.
#[cfg(not(target_family = "wasm"))]
pub fn save_bytes(bytes: Vec<u8>, name: String, tx: &mut Tx) {
    tx.save_file(
        save_options(name),
        move |s: &mut State, result, _| match result {
            Ok(Some(path)) => {
                s.rt.background(async move { write_file(&path, &bytes) })
                    .detach_and_log_err(&s.rt)
            }
            Ok(None) => {}
            Err(error) => log::error!("save dialog: {error}"),
        },
    );
}

/// The Save dialog for a file named `name`, in the Downloads folder (else home).
#[cfg(not(target_family = "wasm"))]
fn save_options(name: String) -> rune::FileDialogOptions {
    let dir = dirs::download_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    rune::FileDialogOptions {
        title: "Save".into(),
        directory: Some(dir),
        file_name: Some(name),
        ..Default::default()
    }
}

#[cfg(not(target_family = "wasm"))]
fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    std::fs::write(path, bytes).with_context(|| format!("Could not save {}", path.display()))
}

/// Save a File block's bytes under its name (`downloadFileBlock`). The
/// desktop asks where first (the Save dialog needs this handler's `tx`),
/// then fetches and writes; the browser fetches, then downloads. A failed
/// fetch or write reaches `on_error`; a cancelled dialog does nothing.
pub fn download_file_block(
    meta: &BTreeMap<String, String>,
    name: String,
    s: &mut State,
    tx: &mut Tx,
    on_error: impl FnOnce(&mut State, anyhow::Error) + 'static,
) {
    #[cfg(not(target_family = "wasm"))]
    {
        let _ = &s;
        let meta = meta.clone();
        tx.save_file(save_options(name), move |s: &mut State, result, _| {
            let path = match result {
                Ok(Some(path)) => path,
                Ok(None) => return,
                Err(error) => return on_error(s, anyhow!("{error}")),
            };
            let blob = file_block_blob(&meta, s);
            s.rt.spawn(async move |cx| {
                let saved = async {
                    let (bytes, _) = blob.await?;
                    cx.background(async move { write_file(&path, &bytes) })
                        .await
                }
                .await;
                if let Err(error) = saved {
                    cx.update(|s| on_error(s, error));
                }
            })
            .detach();
        });
    }
    #[cfg(target_family = "wasm")]
    {
        let _ = tx;
        let blob = file_block_blob(meta, s);
        s.rt.spawn(async move |cx| {
            let saved = blob.await.and_then(|(bytes, _)| {
                roostr_core::web::download(&bytes, &name, &download_mime(&name))
            });
            if let Err(error) = saved {
                cx.update(|s| on_error(s, error));
            }
        })
        .detach();
    }
}

/// Put an image's bytes on the clipboard as PNG (`copyImageBlock`): other
/// formats are decoded and encoded again.
pub fn copy_image(bytes: &[u8], tx: &mut Tx) -> Result<()> {
    let png = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        bytes.to_vec()
    } else {
        let image = image::load_from_memory(bytes)
            .map_err(|_| anyhow!("could not convert the image to PNG"))?;
        let mut out = std::io::Cursor::new(Vec::new());
        image
            .write_to(&mut out, image::ImageFormat::Png)
            .map_err(|_| anyhow!("could not convert the image to PNG"))?;
        out.into_inner()
    };
    let image = rune::ClipboardImage::from_png(png)
        .ok_or_else(|| anyhow!("could not convert the image to PNG"))?;
    tx.copy_rich(rune::ClipboardContent::default().with_image(image));
    Ok(())
}

/// Show the bytes in a new browser tab (the browser's viewer stands in for
/// the system's).
#[cfg(target_family = "wasm")]
pub fn open_externally(bytes: Arc<Vec<u8>>, name: &str, _s: &State) -> Task<Result<()>> {
    Task::ready(roostr_core::web::open_bytes(&bytes, &download_mime(name)))
}

/// Write bytes to a temp file named like the file and open it in its default app.
#[cfg(not(target_family = "wasm"))]
pub fn open_externally(bytes: Arc<Vec<u8>>, name: &str, s: &State) -> Task<Result<()>> {
    let dir = std::env::temp_dir().join("roostr-files");
    let path = dir.join(sanitize(name));
    s.rt.background(async move {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&path, bytes.as_slice())?;
        open_with_system(&path)
    })
}

/// Open `path` in the system's default app for it (Rune's `open_url` takes
/// only web links, so this asks the platform's opener as GPUI did).
#[cfg(not(target_family = "wasm"))]
fn open_with_system(path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = std::process::Command::new("xdg-open");
    let status = command
        .arg(path)
        .status()
        .with_context(|| format!("Could not open {}", path.display()))?;
    if !status.success() {
        return Err(anyhow!("Could not open {}", path.display()));
    }
    Ok(())
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

    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn temp_names_stay_in_the_folder() {
        assert_eq!(sanitize("a/b:c\\d"), "a_b_c_d");
        assert_eq!(sanitize("  "), "file");
    }

    #[test]
    fn pasted_images_are_named_png() {
        let file = pasted_png(vec![1, 2, 3]);
        assert!(file.name.starts_with("Pasted image "));
        assert!(file.name.ends_with(".png"));
        assert_eq!(file.mime, "image/png");
    }
}
