//! The browser's peers: its own `RTCPeerConnection`, which gathers host,
//! mDNS and STUN candidates itself.

use std::any::Any;
use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{
    MessageEvent, RtcConfiguration, RtcDataChannel, RtcDataChannelEvent, RtcIceCandidateInit,
    RtcIceServer, RtcPeerConnection, RtcPeerConnectionIceEvent, RtcPeerConnectionState, RtcSdpType,
    RtcSessionDescriptionInit,
};

use crate::{CHANNEL_LABEL, Peer, PeerEvent, PeerEvents, PeerFactory, STUN_SERVER};

/// Makes `RTCPeerConnection` peers.
pub struct WebPeers;

impl PeerFactory for WebPeers {
    fn answer(&self, offer: &str, events: PeerEvents) -> Box<dyn Peer> {
        match WebPeer::answer(offer, events.clone()) {
            Ok(peer) => Box::new(peer),
            Err(error) => {
                log::debug!("[direct] cannot answer: {error:?}");
                events.send(PeerEvent::Closed);
                Box::new(WebPeer {
                    pc: None,
                    remote: Rc::default(),
                    keep: Rc::default(),
                })
            }
        }
    }
}

/// Candidates that came before the remote description was set (the
/// browser refuses them until then).
#[derive(Default)]
struct Remote {
    set: bool,
    queued: Vec<(String, String)>,
}

/// One `RTCPeerConnection`; dropping it closes the line.
pub struct WebPeer {
    pc: Option<RtcPeerConnection>,
    remote: Rc<RefCell<Remote>>,
    /// The event handlers, alive as long as the peer.
    keep: Rc<RefCell<Keep>>,
}

/// What the handlers need alive, and the channels to unhook on close.
#[derive(Default)]
struct Keep {
    closures: Vec<Box<dyn Any>>,
    channels: Vec<RtcDataChannel>,
}

impl WebPeer {
    fn answer(offer: &str, events: PeerEvents) -> Result<WebPeer, JsValue> {
        let server = RtcIceServer::new();
        server.set_urls(&JsValue::from_str(STUN_SERVER));
        let config = RtcConfiguration::new();
        config.set_ice_servers(&js_sys::Array::of1(&server));
        let pc = RtcPeerConnection::new_with_configuration(&config)?;
        let keep: Rc<RefCell<Keep>> = Rc::default();

        let ice_events = events.clone();
        let on_ice = Closure::<dyn FnMut(RtcPeerConnectionIceEvent)>::new(
            move |event: RtcPeerConnectionIceEvent| {
                let Some(candidate) = event.candidate() else {
                    return;
                };
                let line = candidate.candidate();
                if !line.is_empty() {
                    ice_events.send(PeerEvent::Ice {
                        candidate: line,
                        mid: candidate.sdp_mid().unwrap_or_default(),
                    });
                }
            },
        );
        pc.set_onicecandidate(Some(on_ice.as_ref().unchecked_ref()));
        keep.borrow_mut().closures.push(Box::new(on_ice));

        let state_events = events.clone();
        let state_pc = pc.clone();
        let on_state = Closure::<dyn FnMut(JsValue)>::new(move |_: JsValue| {
            if matches!(
                state_pc.connection_state(),
                RtcPeerConnectionState::Failed | RtcPeerConnectionState::Closed
            ) {
                state_events.send(PeerEvent::Closed);
            }
        });
        pc.set_onconnectionstatechange(Some(on_state.as_ref().unchecked_ref()));
        keep.borrow_mut().closures.push(Box::new(on_state));

        let channel_events = events.clone();
        let channel_keep = keep.clone();
        let on_channel =
            Closure::<dyn FnMut(RtcDataChannelEvent)>::new(move |event: RtcDataChannelEvent| {
                let channel = event.channel();
                if channel.label() == CHANNEL_LABEL {
                    listen(&channel, &channel_events, &channel_keep);
                }
            });
        pc.set_ondatachannel(Some(on_channel.as_ref().unchecked_ref()));
        keep.borrow_mut().closures.push(Box::new(on_channel));

        let remote: Rc<RefCell<Remote>> = Rc::default();
        let description = RtcSessionDescriptionInit::new(RtcSdpType::Offer);
        description.set_sdp(offer);
        let (answer_pc, answer_remote) = (pc.clone(), remote.clone());
        spawn_local(async move {
            let answered: Result<String, JsValue> = async {
                JsFuture::from(answer_pc.set_remote_description(&description)).await?;
                let answer = JsFuture::from(answer_pc.create_answer()).await?;
                let sdp = js_sys::Reflect::get(&answer, &JsValue::from_str("sdp"))?
                    .as_string()
                    .unwrap_or_default();
                let local = RtcSessionDescriptionInit::new(RtcSdpType::Answer);
                local.set_sdp(&sdp);
                JsFuture::from(answer_pc.set_local_description(&local)).await?;
                Ok(sdp)
            }
            .await;
            match answered {
                Ok(sdp) => {
                    events.send(PeerEvent::Answer(sdp));
                    let queued = {
                        let mut remote = answer_remote.borrow_mut();
                        remote.set = true;
                        std::mem::take(&mut remote.queued)
                    };
                    for (candidate, mid) in queued {
                        add_ice(&answer_pc, &candidate, &mid);
                    }
                }
                Err(error) => {
                    log::debug!("[direct] answer: {error:?}");
                    events.send(PeerEvent::Closed);
                }
            }
        });
        Ok(WebPeer {
            pc: Some(pc),
            remote,
            keep,
        })
    }
}

/// Report the channel's opening, messages and end.
fn listen(channel: &RtcDataChannel, events: &PeerEvents, keep: &Rc<RefCell<Keep>>) {
    let open_events = events.clone();
    let on_open = Closure::<dyn FnMut(JsValue)>::new(move |_: JsValue| {
        open_events.send(PeerEvent::Open);
    });
    channel.set_onopen(Some(on_open.as_ref().unchecked_ref()));
    let message_events = events.clone();
    let on_message =
        Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
            match event.data().as_string() {
                Some(text) => message_events.send(PeerEvent::Message(text)),
                None => log::debug!("[direct] a message that is not text"),
            }
        });
    channel.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
    let close_events = events.clone();
    let on_close = Closure::<dyn FnMut(JsValue)>::new(move |_: JsValue| {
        close_events.send(PeerEvent::Closed);
    });
    channel.set_onclose(Some(on_close.as_ref().unchecked_ref()));
    let mut keep = keep.borrow_mut();
    keep.closures.push(Box::new(on_open));
    keep.closures.push(Box::new(on_message));
    keep.closures.push(Box::new(on_close));
    keep.channels.push(channel.clone());
}

fn add_ice(pc: &RtcPeerConnection, candidate: &str, mid: &str) {
    let init = RtcIceCandidateInit::new(candidate);
    if mid.is_empty() {
        init.set_sdp_m_line_index(Some(0));
    } else {
        init.set_sdp_mid(Some(mid));
    }
    let added = pc.add_ice_candidate_with_opt_rtc_ice_candidate_init(Some(&init));
    spawn_local(async move {
        if let Err(error) = JsFuture::from(added).await {
            log::debug!("[direct] remote candidate: {error:?}");
        }
    });
}

impl Peer for WebPeer {
    fn remote_ice(&self, candidate: &str, mid: &str) {
        let Some(pc) = &self.pc else { return };
        let mut remote = self.remote.borrow_mut();
        if remote.set {
            drop(remote);
            add_ice(pc, candidate, mid);
        } else {
            remote.queued.push((candidate.to_string(), mid.to_string()));
        }
    }
}

impl Drop for WebPeer {
    fn drop(&mut self) {
        if let Some(pc) = self.pc.take() {
            pc.set_onicecandidate(None);
            pc.set_onconnectionstatechange(None);
            pc.set_ondatachannel(None);
            pc.close();
        }
        // Unhook before the handlers go: a late event must not call a dropped closure.
        let mut keep = std::mem::take(&mut *self.keep.borrow_mut());
        for channel in keep.channels.drain(..) {
            channel.set_onopen(None);
            channel.set_onmessage(None);
            channel.set_onclose(None);
            channel.close();
        }
        drop(keep);
    }
}
