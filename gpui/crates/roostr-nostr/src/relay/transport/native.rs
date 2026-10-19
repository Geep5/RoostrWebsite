//! Native sockets: each connection runs on its own thread under
//! `async_io::block_on`, so no executor is assumed. Frames to send arrive
//! over a channel; dropping the `Conn` (the sender) closes the socket.

use std::net::IpAddr;
use std::pin::Pin;
use std::sync::{Arc, LazyLock};
use std::task::Poll;

use async_net::TcpStream;
use async_tungstenite::WebSocketStream;
use async_tungstenite::tungstenite::Message;
use futures::channel::mpsc;
use futures::future::poll_fn;
use futures::{AsyncRead, AsyncWrite, StreamExt};
use futures_rustls::TlsConnector;
use futures_rustls::rustls::pki_types::ServerName;
use futures_rustls::rustls::{self, ClientConfig, RootCertStore};

use crate::relay::{Inner, RelayError, RelayOptions};
use crate::shared::{Ptr, WeakPtr, downgrade};
use crate::time;

pub(crate) struct Conn {
    out: mpsc::UnboundedSender<String>,
}

impl Conn {
    pub(crate) fn dial(inner: &Ptr<Inner>, generation: u64) -> Result<Conn, RelayError> {
        let (out, rx) = mpsc::unbounded();
        let weak = downgrade(inner);
        let url = inner.url.clone();
        let options = inner.options.clone();
        std::thread::Builder::new()
            .name("roostr-relay".into())
            .spawn(move || async_io::block_on(run(weak, generation, url, options, rx)))
            .map_err(|e| RelayError::Transport(format!("no socket thread: {e}")))?;
        Ok(Conn { out })
    }

    /// Queue a frame; the thread writes it once the socket is open.
    pub(crate) fn send(&mut self, text: String) -> Result<(), RelayError> {
        self.out
            .unbounded_send(text)
            .map_err(|_| RelayError::Disconnected)
    }

    /// Dropping the sender ends the socket thread, which closes the socket.
    pub(crate) fn close(self) {}
}

async fn run(
    weak: WeakPtr<Inner>,
    generation: u64,
    url: String,
    options: RelayOptions,
    mut rx: mpsc::UnboundedReceiver<String>,
) {
    if let Err(error) = session(&weak, generation, &url, &options, &mut rx).await
        && let Some(inner) = weak.upgrade()
    {
        inner.fail(generation, error);
    }
}

enum Step {
    Inbound(Option<Result<Message, async_tungstenite::tungstenite::Error>>),
    Outbound(Option<String>),
    Tick,
}

fn transport(error: impl std::fmt::Display) -> RelayError {
    RelayError::Transport(error.to_string())
}

async fn session(
    weak: &WeakPtr<Inner>,
    generation: u64,
    url: &str,
    options: &RelayOptions,
    rx: &mut mpsc::UnboundedReceiver<String>,
) -> Result<(), RelayError> {
    let socket = time::timeout(options.connect_timeout, connect(url))
        .await
        .ok_or(RelayError::Timeout)??;
    let (mut sink, mut stream) = socket.split();
    let mut timer = async_io::Timer::after(options.keepalive);
    let mut awaiting_pong = false;
    loop {
        let step = poll_fn(|cx| {
            if let Poll::Ready(message) = stream.poll_next_unpin(cx) {
                return Poll::Ready(Step::Inbound(message));
            }
            if let Poll::Ready(text) = rx.poll_next_unpin(cx) {
                return Poll::Ready(Step::Outbound(text));
            }
            if Pin::new(&mut timer).poll(cx).is_ready() {
                return Poll::Ready(Step::Tick);
            }
            Poll::Pending
        })
        .await;
        match step {
            Step::Inbound(Some(Ok(message))) => {
                let text = match &message {
                    Message::Text(text) => Some(text.as_str()),
                    Message::Binary(bytes) => std::str::from_utf8(bytes).ok(),
                    Message::Pong(_) => {
                        if awaiting_pong {
                            awaiting_pong = false;
                            timer.set_after(options.keepalive);
                        }
                        None
                    }
                    Message::Close(frame) => {
                        let reason = frame
                            .as_ref()
                            .map(|f| f.reason.to_string())
                            .unwrap_or_default();
                        return Err(RelayError::Transport(
                            format!("closed by the relay {reason}")
                                .trim_end()
                                .to_string(),
                        ));
                    }
                    Message::Ping(_) | Message::Frame(_) => None,
                };
                if let Some(text) = text {
                    let Some(inner) = weak.upgrade() else {
                        return Ok(());
                    };
                    inner.handle(generation, text);
                }
            }
            Step::Inbound(Some(Err(error))) => return Err(transport(error)),
            Step::Inbound(None) => return Err(RelayError::Transport("connection closed".into())),
            Step::Outbound(Some(text)) => {
                sink.send(Message::text(text)).await.map_err(transport)?
            }
            Step::Outbound(None) => {
                let _ = sink.close(None).await;
                return Ok(());
            }
            Step::Tick if awaiting_pong => return Err(RelayError::Unresponsive),
            Step::Tick => {
                sink.send(Message::Ping(Default::default()))
                    .await
                    .map_err(transport)?;
                awaiting_pong = true;
                timer.set_after(options.pong_timeout);
            }
        }
    }
}

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

async fn connect(url: &str) -> Result<WebSocketStream<Box<dyn Io>>, RelayError> {
    let parsed = url::Url::parse(url).map_err(|e| RelayError::Transport(format!("{url}: {e}")))?;
    let secure = match parsed.scheme() {
        "wss" => true,
        "ws" => false,
        other => {
            return Err(RelayError::Transport(format!(
                "{url}: not a websocket scheme ({other})"
            )));
        }
    };
    let port = parsed
        .port_or_known_default()
        .unwrap_or(if secure { 443 } else { 80 });
    let host = parsed
        .host()
        .ok_or_else(|| RelayError::Transport(format!("{url}: no host")))?;
    let (tcp, name) = match host {
        url::Host::Domain(domain) => (
            TcpStream::connect((domain, port)).await,
            ServerName::try_from(domain.to_string()).map_err(transport)?,
        ),
        url::Host::Ipv4(ip) => (
            TcpStream::connect((ip, port)).await,
            ServerName::IpAddress(IpAddr::V4(ip).into()),
        ),
        url::Host::Ipv6(ip) => (
            TcpStream::connect((ip, port)).await,
            ServerName::IpAddress(IpAddr::V6(ip).into()),
        ),
    };
    let tcp = tcp.map_err(transport)?;
    let _ = tcp.set_nodelay(true);
    let io: Box<dyn Io> = if secure {
        Box::new(tls().connect(name, tcp).await.map_err(transport)?)
    } else {
        Box::new(tcp)
    };
    let (socket, _response) = async_tungstenite::client_async(url, io)
        .await
        .map_err(transport)?;
    Ok(socket)
}

/// One TLS client configuration per process: ring, the Mozilla roots.
static TLS_CONFIG: LazyLock<Arc<ClientConfig>> = LazyLock::new(|| {
    let roots = RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .expect("ring supports the default TLS versions")
            .with_root_certificates(roots)
            .with_no_client_auth();
    Arc::new(config)
});

fn tls() -> TlsConnector {
    TlsConnector::from(TLS_CONFIG.clone())
}
