//! Browser sockets: `web_sys::WebSocket`, its events fed straight into the
//! relay state. The browser answers pings itself, so there is no keepalive.

use gloo_timers::callback::Timeout;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{BinaryType, CloseEvent, Event, MessageEvent, WebSocket};

use crate::relay::{Inner, RelayError};
use crate::shared::{Ptr, downgrade};

/// Handlers the socket calls into; kept alive as long as the socket.
struct Handlers {
    _open: Closure<dyn FnMut()>,
    _message: Closure<dyn FnMut(MessageEvent)>,
    _close: Closure<dyn FnMut(CloseEvent)>,
    _error: Closure<dyn FnMut(Event)>,
    _connect_deadline: Timeout,
}

pub(crate) struct Conn {
    socket: WebSocket,
    open: bool,
    /// Frames sent while the socket is still connecting.
    pending: Vec<String>,
    handlers: Option<Handlers>,
}

fn js_message(value: &wasm_bindgen::JsValue) -> String {
    value
        .dyn_ref::<js_sys::Error>()
        .map(|e| String::from(e.message()))
        .or_else(|| value.as_string())
        .unwrap_or_else(|| "unknown error".into())
}

impl Conn {
    pub(crate) fn dial(inner: &Ptr<Inner>, generation: u64) -> Result<Conn, RelayError> {
        let socket =
            WebSocket::new(&inner.url).map_err(|e| RelayError::Transport(js_message(&e)))?;
        socket.set_binary_type(BinaryType::Arraybuffer);
        let weak = downgrade(inner);

        let on_open = {
            let weak = weak.clone();
            Closure::<dyn FnMut()>::new(move || {
                if let Some(inner) = weak.upgrade() {
                    opened(&inner, generation);
                }
            })
        };
        let on_message = {
            let weak = weak.clone();
            Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
                let Some(inner) = weak.upgrade() else { return };
                let data = event.data();
                if let Some(text) = data.as_string() {
                    inner.handle(generation, &text);
                } else if let Some(buffer) = data.dyn_ref::<js_sys::ArrayBuffer>() {
                    let bytes = js_sys::Uint8Array::new(buffer).to_vec();
                    if let Ok(text) = std::str::from_utf8(&bytes) {
                        inner.handle(generation, text);
                    }
                }
            })
        };
        let on_close = {
            let weak = weak.clone();
            Closure::<dyn FnMut(CloseEvent)>::new(move |event: CloseEvent| {
                if let Some(inner) = weak.upgrade() {
                    let reason = event.reason();
                    let detail = if reason.is_empty() {
                        event.code().to_string()
                    } else {
                        format!("{} {reason}", event.code())
                    };
                    inner.fail(
                        generation,
                        RelayError::Transport(format!("socket closed ({detail})")),
                    );
                }
            })
        };
        let on_error = {
            let weak = weak.clone();
            Closure::<dyn FnMut(Event)>::new(move |_: Event| {
                if let Some(inner) = weak.upgrade() {
                    inner.fail(generation, RelayError::Transport("socket error".into()));
                }
            })
        };
        let connect_deadline = {
            let weak = weak.clone();
            let ms = inner
                .options
                .connect_timeout
                .as_millis()
                .min(u32::MAX as u128) as u32;
            Timeout::new(ms, move || {
                let Some(inner) = weak.upgrade() else { return };
                let still_connecting = inner.state.with(|st| {
                    st.generation == generation && st.conn.as_ref().is_some_and(|c| !c.open)
                });
                if still_connecting {
                    inner.fail(generation, RelayError::Timeout);
                }
            })
        };
        socket.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        socket.set_onclose(Some(on_close.as_ref().unchecked_ref()));
        socket.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        Ok(Conn {
            socket,
            open: false,
            pending: Vec::new(),
            handlers: Some(Handlers {
                _open: on_open,
                _message: on_message,
                _close: on_close,
                _error: on_error,
                _connect_deadline: connect_deadline,
            }),
        })
    }

    pub(crate) fn send(&mut self, text: String) -> Result<(), RelayError> {
        if !self.open {
            self.pending.push(text);
            return Ok(());
        }
        self.socket
            .send_with_str(&text)
            .map_err(|e| RelayError::Transport(js_message(&e)))
    }

    pub(crate) fn close(self) {}
}

impl Drop for Conn {
    fn drop(&mut self) {
        self.socket.set_onopen(None);
        self.socket.set_onmessage(None);
        self.socket.set_onclose(None);
        self.socket.set_onerror(None);
        let _ = self.socket.close();
        // The socket may be dropped from inside one of these handlers (a
        // close event failing the connection): free them once it returns.
        if let Some(handlers) = self.handlers.take() {
            wasm_bindgen_futures::spawn_local(async move { drop(handlers) });
        }
    }
}

/// The socket opened: send what queued up while it connected.
fn opened(inner: &Ptr<Inner>, generation: u64) {
    let failed = inner.state.with(|st| {
        if st.generation != generation {
            return None;
        }
        let conn = st.conn.as_mut()?;
        conn.open = true;
        for text in std::mem::take(&mut conn.pending) {
            if let Err(error) = conn.socket.send_with_str(&text) {
                return Some(RelayError::Transport(js_message(&error)));
            }
        }
        None
    });
    if let Some(error) = failed {
        inner.fail(generation, error);
    }
}
