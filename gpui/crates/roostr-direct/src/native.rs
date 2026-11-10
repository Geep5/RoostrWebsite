//! The desktop's (and the iPhone's) peers: str0m, sans-IO, driven on a
//! thread of its own over one UDP socket.
//!
//! ICE: one host candidate (the address of the default route, in the
//! answer) and, once the STUN server tells us how the world sees that
//! socket, one server-reflexive candidate trickled after it. No TURN: two
//! devices behind symmetric NATs stay on the relay path.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

use async_io::{Async, Timer};
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures::channel::oneshot;
use futures::{FutureExt, StreamExt, pin_mut, select};
use str0m::change::{SdpAnswer, SdpOffer, SdpPendingOffer};
use str0m::channel::ChannelId;
use str0m::net::{Protocol, Receive};
use str0m::{Candidate, Event, IceConnectionState, Input, Output, Rtc};

use crate::{CHANNEL_LABEL, Peer, PeerEvent, PeerEvents, PeerFactory, STUN_SERVER};

/// A line not connected this long after its offer is given up.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// STUN binding requests: sent this often, this many times.
const STUN_RETRY: Duration = Duration::from_millis(700);
const STUN_ATTEMPTS: u32 = 4;
const MAGIC: u32 = 0x2112_A442;

/// Makes str0m peers.
#[derive(Clone, Debug)]
pub struct NativePeers {
    /// `host:port` of the STUN server; `None`: host candidates only.
    stun: Option<String>,
    /// The host candidate's address; `None`: the default route's.
    host: Option<IpAddr>,
}

impl Default for NativePeers {
    fn default() -> Self {
        Self::new()
    }
}

impl NativePeers {
    /// The app's: STUN at [`STUN_SERVER`], the default route's address.
    pub fn new() -> NativePeers {
        NativePeers {
            stun: Some(STUN_SERVER.trim_start_matches("stun:").to_string()),
            host: None,
        }
    }

    /// On 127.0.0.1 only, no STUN: two peers in one process.
    pub fn loopback() -> NativePeers {
        NativePeers {
            stun: None,
            host: Some(IpAddr::V4(Ipv4Addr::LOCALHOST)),
        }
    }

    /// The offering end (what the harness does): a peer with the ordered
    /// [`CHANNEL_LABEL`] channel and its offer. Feed it the answer with
    /// [`NativePeer::accept_answer`].
    pub fn offer(&self, events: PeerEvents) -> Result<(NativePeer, String), String> {
        let (mut rtc, socket, host) = self.bind()?;
        let mut api = rtc.sdp_api();
        api.add_channel(CHANNEL_LABEL.to_string());
        let (offer, pending) = api.apply().ok_or("no offer to make")?;
        let sdp = offer.to_sdp_string();
        let peer = self.spawn(rtc, socket, host, mid_of(&sdp), Some(pending), events);
        Ok((peer, sdp))
    }

    /// Answer `offer`: the answer goes out as [`PeerEvent::Answer`].
    fn answer_offer(&self, offer: &str, events: PeerEvents) -> Result<NativePeer, String> {
        let offer = SdpOffer::from_sdp_string(offer).map_err(|e| format!("offer: {e}"))?;
        let (mut rtc, socket, host) = self.bind()?;
        let answer = rtc
            .sdp_api()
            .accept_offer(offer)
            .map_err(|e| format!("accept offer: {e}"))?;
        let sdp = answer.to_sdp_string();
        events.send(PeerEvent::Answer(sdp.clone()));
        Ok(self.spawn(rtc, socket, host, mid_of(&sdp), None, events))
    }

    /// A socket, a fresh `Rtc` and the host candidate.
    fn bind(&self) -> Result<(Rtc, UdpSocket, SocketAddr), String> {
        let ip = match self.host {
            Some(ip) => ip,
            None => default_route_ip().ok_or("no network address")?,
        };
        // Bound to the candidate's own address: what arrives is for it.
        let socket = UdpSocket::bind(SocketAddr::new(ip, 0)).map_err(|e| format!("bind: {e}"))?;
        let host = socket.local_addr().map_err(|e| format!("bind: {e}"))?;
        let mut rtc = Rtc::new(Instant::now());
        let candidate = Candidate::host(host, "udp").map_err(|e| format!("candidate: {e}"))?;
        rtc.add_local_candidate(candidate);
        Ok((rtc, socket, host))
    }

    fn spawn(
        &self,
        rtc: Rtc,
        socket: UdpSocket,
        host: SocketAddr,
        mid: String,
        pending: Option<SdpPendingOffer>,
        events: PeerEvents,
    ) -> NativePeer {
        let (tx, rx) = unbounded();
        let stun = self.stun.clone();
        let line = Line {
            rtc,
            host,
            mid,
            pending,
            events,
            channel: None,
            connected: false,
            started: Instant::now(),
        };
        let spawned = std::thread::Builder::new()
            .name("roostr-direct".into())
            .spawn(move || async_io::block_on(line.run(socket, stun, rx)));
        if let Err(error) = spawned {
            log::warn!("[direct] cannot start a peer: {error}");
        }
        NativePeer { tx }
    }
}

impl PeerFactory for NativePeers {
    fn answer(&self, offer: &str, events: PeerEvents) -> Box<dyn Peer> {
        match self.answer_offer(offer, events.clone()) {
            Ok(peer) => Box::new(peer),
            Err(error) => {
                log::debug!("[direct] cannot answer: {error}");
                events.send(PeerEvent::Closed);
                let (tx, _) = unbounded();
                Box::new(NativePeer { tx })
            }
        }
    }
}

enum Command {
    RemoteIce(String),
    Answer(String),
    Send(String),
}

/// One str0m peer; dropping it closes the line.
pub struct NativePeer {
    tx: UnboundedSender<Command>,
}

impl NativePeer {
    /// The offering end: the remote answer.
    pub fn accept_answer(&self, sdp: &str) {
        let _ = self.tx.unbounded_send(Command::Answer(sdp.to_string()));
    }

    /// Send one text message on the channel (once open).
    pub fn send(&self, text: &str) {
        let _ = self.tx.unbounded_send(Command::Send(text.to_string()));
    }
}

impl Peer for NativePeer {
    fn remote_ice(&self, candidate: &str, _mid: &str) {
        let _ = self
            .tx
            .unbounded_send(Command::RemoteIce(candidate.to_string()));
    }
}

/// What woke the loop.
enum Wake {
    Packet(usize, SocketAddr),
    Command(Option<Command>),
    Timer,
    /// The STUN server's address (`None`: not found).
    Resolved(Option<SocketAddr>),
}

/// The peer's state on its thread.
struct Line {
    rtc: Rtc,
    host: SocketAddr,
    /// The data section's `mid`, for trickled candidates.
    mid: String,
    pending: Option<SdpPendingOffer>,
    events: PeerEvents,
    channel: Option<ChannelId>,
    connected: bool,
    started: Instant,
}

impl Line {
    async fn run(
        mut self,
        socket: UdpSocket,
        stun: Option<String>,
        mut commands: UnboundedReceiver<Command>,
    ) {
        let socket = match Async::new(socket) {
            Ok(socket) => socket,
            Err(error) => {
                log::debug!("[direct] socket: {error}");
                self.events.send(PeerEvent::Closed);
                return;
            }
        };
        // Resolved on a thread of its own: a slow DNS answer must not hold
        // the connectivity checks up.
        let mut resolving = stun.map(resolve);
        let mut stun: Option<Stun> = None;
        let mut buf = vec![0u8; 2048];
        loop {
            // Drain to the next timeout (str0m: every mutation, then this).
            let timeout = match self.drain(&socket) {
                Some(timeout) => timeout,
                None => {
                    self.events.send(PeerEvent::Closed);
                    return;
                }
            };
            let now = Instant::now();
            if !self.connected && now >= self.started + CONNECT_TIMEOUT {
                log::debug!("[direct] no connection in {CONNECT_TIMEOUT:?}");
                self.events.send(PeerEvent::Closed);
                return;
            }
            let mut wake_at = timeout.min(self.started + CONNECT_TIMEOUT);
            if let Some(next) = stun.as_ref().and_then(Stun::next) {
                wake_at = wake_at.min(next);
            }
            let wake = {
                let recv = socket.recv_from(&mut buf).fuse();
                let command = commands.next().fuse();
                let timer = FutureExt::fuse(Timer::at(wake_at));
                let resolved = async {
                    match resolving.as_mut() {
                        Some(rx) => rx.await.ok().flatten(),
                        None => futures::future::pending().await,
                    }
                }
                .fuse();
                pin_mut!(recv, command, timer, resolved);
                select! {
                    received = recv => match received {
                        Ok((n, source)) => Wake::Packet(n, source),
                        // Windows reports an earlier send's ICMP error here.
                        Err(_) => continue,
                    },
                    command = command => Wake::Command(command),
                    _ = timer => Wake::Timer,
                    server = resolved => Wake::Resolved(server),
                }
            };
            match wake {
                Wake::Resolved(server) => {
                    resolving = None;
                    stun = server.map(Stun::at);
                }
                Wake::Packet(n, source) => {
                    if let Some(s) = stun.as_mut()
                        && let Some(mapped) = s.response(source, &buf[..n])
                    {
                        self.add_reflexive(mapped);
                        continue;
                    }
                    let Ok(contents) = buf[..n].try_into() else {
                        continue;
                    };
                    let input = Input::Receive(
                        Instant::now(),
                        Receive {
                            proto: Protocol::Udp,
                            source,
                            destination: self.host,
                            contents,
                        },
                    );
                    if self.rtc.handle_input(input).is_err() {
                        self.events.send(PeerEvent::Closed);
                        return;
                    }
                }
                Wake::Command(None) => {
                    // The peer was dropped: say goodbye on the wire, then stop.
                    self.rtc.disconnect();
                    let _ = self.drain(&socket);
                    return;
                }
                Wake::Command(Some(command)) => self.command(command),
                Wake::Timer => {
                    if let Some(s) = stun.as_mut() {
                        s.retry(socket.get_ref());
                    }
                    if self
                        .rtc
                        .handle_input(Input::Timeout(Instant::now()))
                        .is_err()
                    {
                        self.events.send(PeerEvent::Closed);
                        return;
                    }
                }
            }
        }
    }

    /// Send what str0m has to send, report its events; the next deadline, or
    /// `None` once the line is over.
    fn drain(&mut self, socket: &Async<UdpSocket>) -> Option<Instant> {
        loop {
            match self.rtc.poll_output() {
                Ok(Output::Timeout(at)) => {
                    return self.rtc.is_alive().then_some(at);
                }
                Ok(Output::Transmit(t)) => {
                    let _ = socket.get_ref().send_to(&t.contents, t.destination);
                }
                Ok(Output::Event(event)) => match event {
                    Event::Connected => self.connected = true,
                    Event::ChannelOpen(id, _) => {
                        self.channel = Some(id);
                        self.events.send(PeerEvent::Open);
                    }
                    Event::ChannelData(data) => match String::from_utf8(data.data) {
                        Ok(text) => self.events.send(PeerEvent::Message(text)),
                        Err(_) => log::debug!("[direct] a message that is not text"),
                    },
                    Event::ChannelClose(_)
                    | Event::IceConnectionStateChange(IceConnectionState::Disconnected) => {
                        return None;
                    }
                    _ => {}
                },
                Err(error) => {
                    log::debug!("[direct] {error}");
                    return None;
                }
            }
        }
    }

    fn command(&mut self, command: Command) {
        match command {
            Command::RemoteIce(candidate) => {
                let line = candidate.trim().trim_start_matches("a=");
                // mDNS (`….local`) and other unparsable candidates: the
                // connectivity checks find the peer reflexively instead.
                if let Ok(candidate) = Candidate::from_sdp_string(line) {
                    self.rtc.add_remote_candidate(candidate);
                }
            }
            Command::Answer(sdp) => {
                let (Some(pending), Ok(answer)) =
                    (self.pending.take(), SdpAnswer::from_sdp_string(&sdp))
                else {
                    return;
                };
                if let Err(error) = self.rtc.sdp_api().accept_answer(pending, answer) {
                    log::debug!("[direct] answer: {error}");
                }
            }
            Command::Send(text) => {
                if let Some(mut channel) = self.channel.and_then(|id| self.rtc.channel(id))
                    && let Err(error) = channel.write(false, text.as_bytes())
                {
                    log::debug!("[direct] send: {error}");
                }
            }
        }
    }

    fn add_reflexive(&mut self, mapped: SocketAddr) {
        if mapped == self.host {
            return; // not behind a NAT: the host candidate is the same
        }
        let Ok(candidate) = Candidate::server_reflexive(mapped, self.host, "udp") else {
            return;
        };
        let line = candidate.to_sdp_string();
        if self.rtc.add_local_candidate(candidate).is_some() {
            self.events.send(PeerEvent::Ice {
                candidate: line,
                mid: self.mid.clone(),
            });
        }
    }
}

/// The address the default route leaves from (no packet is sent).
fn default_route_ip() -> Option<IpAddr> {
    let probe = UdpSocket::bind("0.0.0.0:0").ok()?;
    probe.connect("8.8.8.8:80").ok()?;
    let ip = probe.local_addr().ok()?.ip();
    (!ip.is_unspecified()).then_some(ip)
}

/// The `mid` of the SDP's data section.
fn mid_of(sdp: &str) -> String {
    sdp.lines()
        .find_map(|line| line.trim().strip_prefix("a=mid:"))
        .unwrap_or("0")
        .to_string()
}

/// The IPv4 address of `server` (`host:port`), looked up on a thread.
fn resolve(server: String) -> oneshot::Receiver<Option<SocketAddr>> {
    let (tx, rx) = oneshot::channel();
    let spawned = std::thread::Builder::new()
        .name("roostr-direct-dns".into())
        .spawn(move || {
            let found = server
                .to_socket_addrs()
                .ok()
                .and_then(|mut addrs| addrs.find(SocketAddr::is_ipv4));
            let _ = tx.send(found);
        });
    if let Err(error) = spawned {
        log::debug!("[direct] no thread for the STUN lookup: {error}");
    }
    rx
}

/// One STUN binding exchange (RFC 5389) over the peer's own socket.
struct Stun {
    server: SocketAddr,
    transaction: [u8; 12],
    sent: u32,
    next: Option<Instant>,
}

impl Stun {
    /// A binding exchange with `server`, its first request due now.
    fn at(server: SocketAddr) -> Stun {
        let mut transaction = [0u8; 12];
        transaction[..8].copy_from_slice(&random_u64().to_le_bytes());
        transaction[8..].copy_from_slice(&random_u64().to_le_bytes()[..4]);
        Stun {
            server,
            transaction,
            sent: 0,
            next: Some(Instant::now()),
        }
    }

    /// When to (re)send, while unanswered.
    fn next(&self) -> Option<Instant> {
        self.next
    }

    fn retry(&mut self, socket: &UdpSocket) {
        let Some(next) = self.next else { return };
        if Instant::now() < next {
            return;
        }
        if self.sent >= STUN_ATTEMPTS {
            self.next = None;
            return;
        }
        let mut request = Vec::with_capacity(20);
        request.extend_from_slice(&0x0001u16.to_be_bytes());
        request.extend_from_slice(&0u16.to_be_bytes());
        request.extend_from_slice(&MAGIC.to_be_bytes());
        request.extend_from_slice(&self.transaction);
        let _ = socket.send_to(&request, self.server);
        self.sent += 1;
        self.next = Some(Instant::now() + STUN_RETRY);
    }

    /// The mapped address, when `packet` is the answer to our request.
    fn response(&mut self, source: SocketAddr, packet: &[u8]) -> Option<SocketAddr> {
        if source != self.server
            || packet.len() < 20
            || packet[..2] != 0x0101u16.to_be_bytes()
            || packet[8..20] != self.transaction
        {
            return None;
        }
        self.next = None;
        parse_mapped(packet)
    }
}

/// XOR-MAPPED-ADDRESS (or plain MAPPED-ADDRESS) of a binding response, IPv4.
fn parse_mapped(packet: &[u8]) -> Option<SocketAddr> {
    let length = u16::from_be_bytes([packet[2], packet[3]]) as usize;
    let body = packet.get(20..20 + length)?;
    let mut plain = None;
    let mut at = 0;
    while at + 4 <= body.len() {
        let kind = u16::from_be_bytes([body[at], body[at + 1]]);
        let len = u16::from_be_bytes([body[at + 2], body[at + 3]]) as usize;
        let value = body.get(at + 4..at + 4 + len)?;
        if (kind == 0x0020 || kind == 0x0001) && len >= 8 && value[1] == 0x01 {
            let mut port = u16::from_be_bytes([value[2], value[3]]);
            let mut ip = u32::from_be_bytes([value[4], value[5], value[6], value[7]]);
            if kind == 0x0020 {
                port ^= (MAGIC >> 16) as u16;
                ip ^= MAGIC;
                return Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::from(ip)), port));
            }
            plain = Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::from(ip)), port));
        }
        at += 4 + len.div_ceil(4) * 4;
    }
    plain
}

/// Process-random bits for a STUN transaction id.
fn random_u64() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_binding_response_yields_the_xor_mapped_address() {
        let transaction = [7u8; 12];
        let mapped: SocketAddr = "203.0.113.5:40000".parse().unwrap();
        let IpAddr::V4(ip) = mapped.ip() else {
            unreachable!()
        };
        let mut value = vec![0, 0x01];
        value.extend_from_slice(&(mapped.port() ^ (MAGIC >> 16) as u16).to_be_bytes());
        value.extend_from_slice(&(u32::from(ip) ^ MAGIC).to_be_bytes());
        // An unrelated attribute first (SOFTWARE, padded).
        let mut body = vec![0x80, 0x22, 0, 3, b'a', b'b', b'c', 0];
        body.extend_from_slice(&[0x00, 0x20, 0, 8]);
        body.extend_from_slice(&value);
        let mut packet = vec![0x01, 0x01];
        packet.extend_from_slice(&(body.len() as u16).to_be_bytes());
        packet.extend_from_slice(&MAGIC.to_be_bytes());
        packet.extend_from_slice(&transaction);
        packet.extend_from_slice(&body);
        let server: SocketAddr = "198.51.100.1:19302".parse().unwrap();
        let mut stun = Stun {
            server,
            transaction,
            sent: 1,
            next: Some(Instant::now()),
        };
        let stranger: SocketAddr = "198.51.100.2:19302".parse().unwrap();
        assert_eq!(
            stun.response(stranger, &packet),
            None,
            "only the server answers"
        );
        assert_eq!(stun.response(server, &packet), Some(mapped));
        assert_eq!(stun.next(), None, "answered: no more requests");
    }

    #[test]
    fn the_data_sections_mid_is_found() {
        let sdp = "v=0\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\na=mid:data\r\n";
        assert_eq!(mid_of(sdp), "data");
        assert_eq!(mid_of("v=0\r\n"), "0");
    }
}
