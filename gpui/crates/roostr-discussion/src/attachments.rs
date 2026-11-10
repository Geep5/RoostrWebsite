//! Files in a discussion. An image pasted (or a file dropped) into the composer is added to
//! the object's space as a File object, as the editor adds one, and goes with the next chat
//! line (`chat_post`'s `files`). A message's Files load once per instance for its bubble:
//! an image's bytes as its thumbnail, any other File by its name.

use std::path::PathBuf;

use anyhow::anyhow;
use roostr_core::State;
use roostr_core::create::{AddFile, FileBytes, add_failure_text, pasted_name};
use roostr_core::relations::{current_space_id, object_space_id};
use roostr_core::types::field_str;
use roostr_ui::Images;
use rune::ClipboardContent;

use crate::discussion::{Disc, with};

/// The [`Images`] key of a pending upload's own bytes.
const PREVIEW: &str = "roostr-chat-upload:";

/// A file on its way into the next message.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Attachment {
    /// This upload's own id (its element keys; its result finds it by this).
    pub local: String,
    pub name: String,
    pub mime: String,
    /// The [`Images`] key of its bytes, for an image.
    pub preview: Option<String>,
    /// The File object it became; "" while it uploads.
    pub file_id: String,
}

impl Attachment {
    pub(crate) fn uploading(&self) -> bool {
        self.file_id.is_empty()
    }
}

/// A File a message shows, as far as it has loaded.
#[derive(Clone, Debug, Default, PartialEq, Hash)]
pub(crate) struct ChatFile {
    pub name: String,
    pub mime: String,
    /// The [`Images`] key of its bytes, once they are in (an image).
    pub image: Option<String>,
    pub loading: bool,
    /// Why it could not be loaded.
    pub error: String,
}

impl ChatFile {
    pub(crate) fn is_image(&self) -> bool {
        self.mime.starts_with("image/")
    }
}

impl Disc {
    /// Files go with chat lines only: an exchange's envelopes carry none.
    pub(crate) fn takes_files(&self) -> bool {
        !self.read_only && !self.is_exchange
    }

    /// An attachment is still uploading: the message waits for it.
    pub(crate) fn uploading(&self) -> bool {
        self.attachments.iter().any(Attachment::uploading)
    }

    /// A paste into the composer: an image becomes an attachment. False for anything else,
    /// which the field pastes as text.
    pub(crate) fn on_paste(&mut self, content: &ClipboardContent, s: &mut State) -> bool {
        let Some(image) = content.image() else {
            return false;
        };
        if !self.takes_files() {
            return false;
        }
        // Rune hands every pasted image over as PNG.
        self.attach(
            pasted_name("png"),
            "image/png".into(),
            image.png().to_vec(),
            s,
        );
        true
    }

    /// Files dropped on the composer from another app: each becomes an attachment.
    pub(crate) fn drop_files(&mut self, paths: Vec<PathBuf>, s: &mut State) {
        if !self.takes_files() || paths.is_empty() {
            return;
        }
        #[cfg(target_family = "wasm")]
        let _ = (paths, s);
        #[cfg(not(target_family = "wasm"))]
        {
            let read = s.rt.background(async move {
                paths
                    .into_iter()
                    .map(|path| {
                        let name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        let mime = match roostr_core::create::mime_of(&path) {
                            "" => "application/octet-stream",
                            known => known,
                        };
                        (name, mime.to_string(), std::fs::read(&path))
                    })
                    .collect::<Vec<_>>()
            });
            let key = self.key.clone();
            s.rt.spawn(async move |cx| {
                let files = read.await;
                cx.update(|s| {
                    with(s, &key, |d, s| {
                        for (name, mime, bytes) in files {
                            match bytes {
                                Ok(bytes) => d.attach(name, mime, bytes, s),
                                Err(error) => d.send_error = add_failure_text(&error.into(), &name),
                            }
                        }
                    });
                });
            })
            .detach();
        }
    }

    /// Add `bytes` to the object's space as a File object (as the editor adds one); it shows
    /// in the composer meanwhile and goes with the next message once it is in.
    pub(crate) fn attach(&mut self, name: String, mime: String, bytes: Vec<u8>, s: &mut State) {
        self.send_error.clear();
        let Some(AddFile(add)) = s.add_file.clone() else {
            self.send_error = add_failure_text(&anyhow!("this app can't add files"), &name);
            return;
        };
        let local = uuid::Uuid::new_v4().to_string();
        let preview = match s.get::<Images>() {
            Some(images) if mime.starts_with("image/") => {
                let uri = format!("{PREVIEW}{local}");
                images.insert(&uri, bytes.clone());
                Some(uri)
            }
            _ => None,
        };
        self.attachments.push(Attachment {
            local: local.clone(),
            name: name.clone(),
            mime: mime.clone(),
            preview,
            file_id: String::new(),
        });
        let space = match &self.object {
            Some(object) => object_space_id(&s.store, &object.fields),
            None => current_space_id(s),
        };
        let upload = add(name.clone(), mime, bytes, space, s);
        let key = self.key.clone();
        s.rt.spawn(async move |cx| {
            let result = upload.await;
            cx.update(|s| {
                with(s, &key, |d, _| d.uploaded(&local, &name, result));
            });
        })
        .detach();
    }

    /// An upload ended: the attachment names its File, or leaves with the reason shown.
    fn uploaded(&mut self, local: &str, name: &str, result: anyhow::Result<String>) {
        // Removed while it uploaded.
        let Some(ix) = self.attachments.iter().position(|a| a.local == local) else {
            return;
        };
        let result = result.and_then(|id| {
            if id.is_empty() {
                Err(anyhow!("no File was made"))
            } else {
                Ok(id)
            }
        });
        match result {
            Ok(id) => {
                let a = &mut self.attachments[ix];
                a.file_id = id.clone();
                // The sent message's bubble shows the bytes already here.
                self.chat_files.insert(
                    id,
                    ChatFile {
                        name: a.name.clone(),
                        mime: a.mime.clone(),
                        image: a.preview.clone(),
                        loading: false,
                        error: String::new(),
                    },
                );
            }
            Err(error) => {
                self.attachments.remove(ix);
                self.send_error = add_failure_text(&error, name);
            }
        }
    }

    /// The × on an attachment: it no longer goes with the message.
    pub(crate) fn remove_attachment(&mut self, local: &str) {
        self.attachments.retain(|a| a.local != local);
    }

    /// Start loading the shown messages' Files not asked for yet.
    pub(crate) fn load_files(&mut self, s: &mut State) {
        let wanted: Vec<String> = self
            .messages
            .iter()
            .flat_map(|m| &m.files)
            .filter(|id| !self.chat_files.contains_key(*id))
            .cloned()
            .collect();
        for id in wanted {
            if self.chat_files.contains_key(&id) {
                continue;
            }
            self.chat_files.insert(
                id.clone(),
                ChatFile {
                    loading: true,
                    ..ChatFile::default()
                },
            );
            self.load_file(id, s);
        }
    }

    /// File `id`'s name and type, then (an image) its bytes, as an editor image block loads
    /// them.
    fn load_file(&self, id: String, s: &mut State) {
        let fetch = s.store.fetch_object(&id);
        let key = self.key.clone();
        s.rt.spawn(async move |cx| {
            let fields = match fetch.await {
                Ok(object) => object.fields,
                Err(error) => {
                    cx.update(|s| {
                        with(s, &key, |d, _| {
                            d.chat_files.insert(
                                id,
                                ChatFile {
                                    error: error.to_string(),
                                    ..ChatFile::default()
                                },
                            );
                        });
                    });
                    return;
                }
            };
            let mut file = ChatFile {
                name: field_str(&fields, "name").to_string(),
                mime: field_str(&fields, "file_mime").to_string(),
                ..ChatFile::default()
            };
            if file.is_image() {
                let bytes = cx.update(|s| match s.file_bytes.clone() {
                    Some(FileBytes(load)) => load(fields, s),
                    None => roostr_rt::Task::ready(Err(anyhow!("this app can't load files"))),
                });
                match bytes.await {
                    Ok((bytes, _)) => {
                        let uri = format!("roostr-file:{id}");
                        cx.update(|s| {
                            if let Some(images) = s.get::<Images>() {
                                images.insert(&uri, bytes);
                                file.image = Some(uri);
                            }
                        });
                    }
                    Err(error) => file.error = error.to_string(),
                }
            }
            cx.update(|s| {
                with(s, &key, |d, _| {
                    d.chat_files.insert(id, file);
                });
            });
        })
        .detach();
    }
}
