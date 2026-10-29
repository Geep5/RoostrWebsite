//! A subscription that survives its socket: SyncEngine.swift's `liveLoop`.
//! When the subscription fails it waits the backoff (2 s, doubling up to
//! 60 s) and subscribes again with filters built afresh, so a caller can
//! move `since` to its cursor; any answer from the relay resets the backoff.

use std::time::Duration;

use futures::{Stream, StreamExt, stream};

use super::{Relay, RelayError, Subscription, SubscriptionItem};
use crate::filter::Filter;
use crate::time;

/// First reconnect delay, and the delay after a relay answered.
pub const BACKOFF_FLOOR: Duration = Duration::from_secs(2);
/// Longest reconnect delay.
pub const BACKOFF_CEILING: Duration = Duration::from_secs(60);

/// What a [`live`] stream delivers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiveItem {
    Event(crate::Event),
    /// The relay sent its stored events for this (re)subscription (EOSE).
    /// After a reconnect the caller catches up on what the dead socket
    /// missed, re-fetches gift wraps and wakes its outbox.
    Up {
        reconnected: bool,
    },
    /// The subscription failed; the next attempt follows the backoff.
    Down(RelayError),
}

struct Live<F> {
    relay: Relay,
    filters: F,
    sub: Option<Subscription>,
    backoff: Duration,
    reconnecting: bool,
}

/// Subscribe to `relay` with `filters()` forever, resubscribing after
/// failures. The stream never ends; drop it to close the subscription.
pub fn live<F>(relay: Relay, filters: F) -> impl Stream<Item = LiveItem>
where
    F: FnMut() -> Vec<Filter>,
{
    let state = Live {
        relay,
        filters,
        sub: None,
        backoff: BACKOFF_FLOOR,
        reconnecting: false,
    };
    stream::unfold(state, |mut st| async move {
        if st.sub.is_none() {
            if st.reconnecting {
                time::sleep(st.backoff).await;
                st.backoff = (st.backoff * 2).min(BACKOFF_CEILING);
            }
            st.sub = Some(st.relay.subscribe((st.filters)()));
        }
        let next = st.sub.as_mut().expect("subscribed above").next().await;
        let item = match next {
            Some(Ok(SubscriptionItem::Event(event) | SubscriptionItem::Sequenced(event, _))) => {
                st.backoff = BACKOFF_FLOOR;
                LiveItem::Event(event)
            }
            Some(Ok(SubscriptionItem::Eose | SubscriptionItem::Cursor(_))) => {
                st.backoff = BACKOFF_FLOOR;
                LiveItem::Up {
                    reconnected: st.reconnecting,
                }
            }
            Some(Err(error)) => {
                st.sub = None;
                st.reconnecting = true;
                LiveItem::Down(error)
            }
            None => {
                st.sub = None;
                st.reconnecting = true;
                LiveItem::Down(RelayError::Disconnected)
            }
        };
        Some((item, st))
    })
}
