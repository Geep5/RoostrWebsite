//! The sync dot's words (`src/lib/sync-status.ts`).

use crate::types::{SyncPhase, SyncStatus};

/// `n.toLocaleString("en-US")`: thousands separated by commas.
pub fn count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The history pass in flight: what it received and, when known, the expected
/// count and a percentage (capped at 99 until live).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyncProgress {
    pub received: u64,
    pub expected: Option<u64>,
    pub pct: Option<u64>,
}

pub fn sync_progress(s: &SyncStatus) -> Option<SyncProgress> {
    let syncing =
        s.phase == SyncPhase::Backfill || (s.phase == SyncPhase::Live && s.received.is_some());
    if !syncing {
        return None;
    }
    let received = s.received.unwrap_or(0);
    let Some(expected) = s.expected.filter(|e| *e > 0) else {
        return Some(SyncProgress {
            received,
            expected: None,
            pct: None,
        });
    };
    let cap = if s.phase == SyncPhase::Live { 100 } else { 99 };
    let pct = (received * 100 / expected).min(cap);
    Some(SyncProgress {
        received,
        expected: Some(expected),
        pct: Some(pct),
    })
}

/// "Syncing · 1,234 of 5,000 (24%)" or "Syncing · 1,234 received".
pub fn progress_text(p: &SyncProgress) -> String {
    match (p.expected, p.pct) {
        (Some(expected), Some(pct)) => format!(
            "Syncing · {} of {} ({pct}%)",
            count(p.received),
            count(expected)
        ),
        _ => format!("Syncing · {} received", count(p.received)),
    }
}

/// The sync dot's tooltip. Unsent changes come with what sync says right now.
pub fn sync_tip(s: &SyncStatus) -> String {
    let pending = match s.pending {
        Some(n) if n > 0 => format!("{} change(s) not yet published", count(n)),
        _ => String::new(),
    };
    let progress = sync_progress(s);
    let state = match s.phase {
        SyncPhase::Error => format!("Error: {}", s.detail.as_deref().unwrap_or("sync stopped")),
        SyncPhase::Live => {
            let head = match &s.detail {
                Some(d) => d.clone(),
                None if pending.is_empty() => format!("Synced · {} changes", count(s.imported)),
                None => String::new(),
            };
            let tail = progress.as_ref().map(progress_text).unwrap_or_default();
            [head, tail]
                .into_iter()
                .filter(|x| !x.is_empty())
                .collect::<Vec<_>>()
                .join(" · ")
        }
        SyncPhase::Backfill => match &s.detail {
            Some(d) if d != "catching up" => d.clone(),
            _ => progress.as_ref().map(progress_text).unwrap_or_default(),
        },
        SyncPhase::Idle => "Not syncing".into(),
    };
    [pending, state]
        .into_iter()
        .filter(|x| !x.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tips() {
        assert_eq!(count(1234567), "1,234,567");
        let live = SyncStatus {
            phase: SyncPhase::Live,
            imported: 1200,
            ..Default::default()
        };
        assert_eq!(sync_tip(&live), "Synced · 1,200 changes");
        let back = SyncStatus {
            phase: SyncPhase::Backfill,
            received: Some(1234),
            expected: Some(5000),
            ..Default::default()
        };
        assert_eq!(sync_tip(&back), "Syncing · 1,234 of 5,000 (24%)");
        let stuck = SyncStatus {
            phase: SyncPhase::Live,
            pending: Some(3),
            ..Default::default()
        };
        assert_eq!(sync_tip(&stuck), "3 change(s) not yet published");
    }

    fn status(phase: SyncPhase) -> SyncStatus {
        SyncStatus {
            phase,
            ..Default::default()
        }
    }

    #[test]
    fn a_known_total_shows_received_of_expected_capped_at_99_until_live() {
        let fresh = SyncStatus {
            received: Some(1234),
            expected: Some(5000),
            ..status(SyncPhase::Backfill)
        };
        assert_eq!(sync_tip(&fresh), "Syncing · 1,234 of 5,000 (24%)");
        let caught_up = SyncStatus {
            received: Some(5000),
            expected: Some(5000),
            detail: Some("catching up".into()),
            bootstrapped: true,
            ..status(SyncPhase::Backfill)
        };
        assert_eq!(sync_tip(&caught_up), "Syncing · 5,000 of 5,000 (99%)");
        let live = SyncStatus {
            received: Some(5000),
            expected: Some(5000),
            ..status(SyncPhase::Live)
        };
        assert_eq!(sync_progress(&live).and_then(|p| p.pct), Some(100));
    }

    #[test]
    fn an_unknown_total_counts_what_arrived() {
        let some = SyncStatus {
            received: Some(12345),
            ..status(SyncPhase::Backfill)
        };
        assert_eq!(sync_tip(&some), "Syncing · 12,345 received");
        assert_eq!(
            sync_tip(&status(SyncPhase::Backfill)),
            "Syncing · 0 received"
        );
    }

    #[test]
    fn errors_reconnects_and_pending_keep_their_wording() {
        let error = SyncStatus {
            detail: Some("boom".into()),
            bootstrapped: true,
            ..status(SyncPhase::Error)
        };
        assert_eq!(sync_tip(&error), "Error: boom");
        let reconnecting = SyncStatus {
            received: Some(3),
            expected: Some(9),
            detail: Some("relay unresponsive - reconnecting".into()),
            bootstrapped: true,
            ..status(SyncPhase::Backfill)
        };
        assert_eq!(sync_tip(&reconnecting), "relay unresponsive - reconnecting");
        let pending = SyncStatus {
            received: Some(3),
            pending: Some(1200),
            bootstrapped: true,
            ..status(SyncPhase::Backfill)
        };
        assert_eq!(
            sync_tip(&pending),
            "1,200 change(s) not yet published · Syncing · 3 received"
        );
    }

    #[test]
    fn live_verification_shows_its_progress_and_a_finished_pass_shows_none() {
        let verifying = SyncStatus {
            imported: 9,
            detail: Some("loading spaces · 9 changes so far".into()),
            received: Some(9),
            expected: Some(40),
            ..status(SyncPhase::Live)
        };
        assert_eq!(
            sync_tip(&verifying),
            "loading spaces · 9 changes so far · Syncing · 9 of 40 (22%)"
        );
        let done = SyncStatus {
            imported: 2500,
            bootstrapped: true,
            ..status(SyncPhase::Live)
        };
        assert_eq!(sync_tip(&done), "Synced · 2,500 changes");
        assert_eq!(sync_progress(&done), None);
    }
}
