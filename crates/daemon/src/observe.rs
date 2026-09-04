//! Bounded request-event ring and observation helpers.

use std::collections::VecDeque;

use crate::protocol::{
    DaemonError, DaemonStatus, EVENT_RING_CAPACITY, EventCursor, OBSERVE_PAGE_SIZE, Observation,
    RequestEvent,
};
use retrieval::StageTimings;

/// Fields for a terminal worker operation event, excluding elapsed/instance.
pub struct TerminalEventDraft {
    pub connection_id: u64,
    pub request_id: u64,
    pub operation: &'static str,
    pub outcome: &'static str,
    pub error_code: Option<String>,
    pub result_count: Option<u64>,
    pub stage_millis: Option<StageTimings>,
}

/// In-memory ring of terminal request metadata. Short lock only.
#[derive(Debug, Default)]
pub struct EventRing {
    events: VecDeque<RequestEvent>,
    next_sequence: u64,
}

impl EventRing {
    pub fn push(&mut self, mut event: RequestEvent) -> u64 {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        event.cursor.sequence = sequence;
        self.events.push_back(event);
        while self.events.len() > EVENT_RING_CAPACITY {
            self.events.pop_front();
        }
        sequence
    }

    pub fn next_sequence(&self) -> u64 {
        self.next_sequence
    }

    /// Return events after `after`, signalling gaps when the cursor is stale.
    pub fn page(
        &self,
        instance_id: &str,
        after: Option<&EventCursor>,
    ) -> (Vec<RequestEvent>, EventCursor, bool, bool) {
        let mut gap = false;
        let start_seq = match after {
            None => 0,
            Some(cursor) if cursor.instance_id != instance_id => {
                gap = true;
                0
            }
            Some(cursor) => {
                let oldest = self.events.front().map(|e| e.cursor.sequence);
                if let Some(oldest) = oldest {
                    if cursor.sequence.saturating_add(1) < oldest {
                        gap = true;
                    }
                } else if cursor.sequence > 0 && self.next_sequence > 0 {
                    // Empty ring but client had a prior cursor from this instance.
                    if cursor.sequence + 1 < self.next_sequence {
                        gap = true;
                    }
                }
                cursor.sequence.saturating_add(1)
            }
        };

        let mut events: Vec<RequestEvent> = self
            .events
            .iter()
            .filter(|e| e.cursor.sequence >= start_seq)
            .cloned()
            .collect();

        let more = events.len() > OBSERVE_PAGE_SIZE;
        if more {
            events.truncate(OBSERVE_PAGE_SIZE);
        }

        let next_sequence = events.last().map(|e| e.cursor.sequence).unwrap_or_else(|| {
            start_seq
                .saturating_sub(1)
                .min(self.next_sequence.saturating_sub(1))
        });

        // When no events returned, next_cursor stays at the last consumed sequence
        // (or 0 if none). Never replay the last consumed sequence.
        let next_cursor = EventCursor {
            instance_id: instance_id.to_owned(),
            sequence: if events.is_empty() {
                after.map(|c| c.sequence).unwrap_or(0)
            } else {
                next_sequence
            },
        };

        (events, next_cursor, gap, more)
    }
}

pub fn safe_error_code(err: &DaemonError) -> String {
    match err {
        DaemonError::ProtocolVersion { .. } => "protocol_version",
        DaemonError::Starting => "starting",
        DaemonError::IndexInProgress => "index_in_progress",
        DaemonError::SymbolNotFound { .. } => "symbol_not_found",
        DaemonError::SymbolAmbiguous { .. } => "symbol_ambiguous",
        DaemonError::StoreStale { .. } => "store_stale",
        DaemonError::GpuUnavailable { .. } => "gpu_unavailable",
        DaemonError::RequestTooLarge { .. } => "request_too_large",
        DaemonError::Malformed { .. } => "malformed",
        DaemonError::Internal { .. } => "internal",
        DaemonError::ObserverForbidden { .. } => "observer_forbidden",
    }
    .to_owned()
}

pub fn build_observation(
    status: DaemonStatus,
    ring: &EventRing,
    after: Option<&EventCursor>,
) -> Observation {
    let instance_id = status.instance_id.clone();
    let (events, next_cursor, gap, more) = ring.page(&instance_id, after);
    Observation {
        status,
        events,
        next_cursor,
        gap,
        more,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Lifecycle, ResourceSnapshot};

    fn event(instance: &str, seq_hint: u64, connection_id: u64, request_id: u64) -> RequestEvent {
        RequestEvent {
            cursor: EventCursor {
                instance_id: instance.into(),
                sequence: seq_hint,
            },
            connection_id,
            request_id,
            completed_at_unix_ms: 1,
            operation: "Search".into(),
            elapsed_micros: 10,
            outcome: "ok".into(),
            error_code: None,
            result_count: Some(1),
            stage_millis: None,
        }
    }

    fn status(instance: &str) -> DaemonStatus {
        DaemonStatus {
            lifecycle: Lifecycle::Ready,
            instance_id: instance.into(),
            observed_at_unix_ms: 1,
            model_id: None,
            chunks_live: None,
            chunks_dead: None,
            indexed_commit: None,
            idle_seconds: 0,
            uptime_seconds: 0,
            current_progress: None,
            last_index: None,
            resources: ResourceSnapshot::unavailable(1),
        }
    }

    #[test]
    fn ring_assigns_monotonic_sequences() {
        let mut ring = EventRing::default();
        let s0 = ring.push(event("a", 99, 1, 2));
        let s1 = ring.push(event("a", 99, 1, 3));
        assert_eq!(s0, 0);
        assert_eq!(s1, 1);
    }

    #[test]
    fn page_never_replays_last_consumed_sequence() {
        let mut ring = EventRing::default();
        for i in 0..5 {
            ring.push(event("a", 0, 1, i));
        }
        let (page, next, gap, more) = ring.page(
            "a",
            Some(&EventCursor {
                instance_id: "a".into(),
                sequence: 1,
            }),
        );
        assert!(!gap);
        assert!(!more);
        assert_eq!(page.first().map(|e| e.cursor.sequence), Some(2));
        assert_eq!(next.sequence, 4);
    }

    #[test]
    fn mismatched_instance_signals_gap() {
        let mut ring = EventRing::default();
        ring.push(event("new", 0, 1, 1));
        let (_events, _next, gap, _) = ring.page(
            "new",
            Some(&EventCursor {
                instance_id: "old".into(),
                sequence: 0,
            }),
        );
        assert!(gap);
    }

    #[test]
    fn observation_omits_sensitive_payload_fields() {
        let mut ring = EventRing::default();
        let mut ev = event("a", 0, 1, 2);
        ev.operation = "Search".into();
        ring.push(ev);
        let obs = build_observation(status("a"), &ring, None);
        let encoded = format!("{obs:?}");
        assert!(!encoded.contains("SECRET_QUERY"));
        assert!(!encoded.contains("secret_path.rs"));
        assert_eq!(obs.events[0].operation, "Search");
        assert_eq!(obs.events[0].request_id, 2);
    }
}
