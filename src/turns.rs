#[derive(Default)]
pub(crate) struct TurnTracker {
    current: Option<PendingTurn>,
    last_ordinal: Option<i64>,
    last_boundary_was_terminal: bool,
}

pub(crate) struct TurnSignal<'a> {
    pub(crate) kind: &'a str,
    pub(crate) turn_id: Option<&'a str>,
    pub(crate) requested_model: Option<&'a str>,
    pub(crate) served_model: Option<&'a str>,
}

pub(crate) struct FinalizedTurn {
    pub(crate) ordinal: i64,
    pub(crate) turn_id: Option<String>,
    pub(crate) start_sequence: Option<i64>,
    pub(crate) start_timestamp: Option<String>,
    pub(crate) start_kind: String,
    pub(crate) end_sequence: Option<i64>,
    pub(crate) end_timestamp: Option<String>,
    pub(crate) outcome: &'static str,
    pub(crate) resumed: bool,
    pub(crate) requested_model: Option<String>,
    pub(crate) requested_model_sequence: Option<i64>,
    pub(crate) observed_model: Option<String>,
    pub(crate) observed_model_sequence: Option<i64>,
}

struct PendingTurn {
    ordinal: i64,
    turn_id: Option<String>,
    start_sequence: Option<i64>,
    start_timestamp: Option<String>,
    start_kind: String,
    resumed: bool,
    requested_model: Option<String>,
    requested_model_sequence: Option<i64>,
    observed_model: Option<String>,
    observed_model_sequence: Option<i64>,
}

pub(crate) struct TurnUpdate {
    pub(crate) ordinal: Option<i64>,
    pub(crate) finalized: Option<FinalizedTurn>,
    pub(crate) opened: bool,
    pub(crate) is_resumption: bool,
    pub(crate) is_model_change: bool,
}

impl TurnTracker {
    pub(crate) fn observe(
        &mut self,
        sequence: i64,
        timestamp: Option<&str>,
        signal: TurnSignal<'_>,
    ) -> TurnUpdate {
        let opening = matches!(signal.kind, "started" | "requested_model" | "user_activity");
        let terminal_outcome = match signal.kind {
            "completed" => Some("complete"),
            "interrupted" => Some("interrupted"),
            _ => None,
        };
        let new_explicit_boundary = signal.kind == "started"
            && self.current.as_ref().is_some_and(|current| {
                signal.turn_id.is_none()
                    || current.turn_id.as_deref().is_none()
                    || current.turn_id.as_deref() != signal.turn_id
            });
        let distinct_turn_id = opening
            && matches!(
                (&self.current, signal.turn_id),
                (Some(current), Some(turn_id))
                    if current.turn_id.as_deref().is_some_and(|current| current != turn_id)
            );
        let mut finalized = (new_explicit_boundary || distinct_turn_id)
            .then(|| self.current.take().map(PendingTurn::finish_unclosed))
            .flatten();
        let should_open = opening && self.current.is_none();
        let mut is_resumption = false;
        if should_open {
            let ordinal = self.last_ordinal.unwrap_or(0) + 1;
            is_resumption = self.last_boundary_was_terminal;
            self.current = Some(PendingTurn {
                ordinal,
                turn_id: signal.turn_id.map(str::to_owned),
                start_sequence: Some(sequence),
                start_timestamp: timestamp.map(str::to_owned),
                start_kind: signal.kind.to_owned(),
                resumed: is_resumption,
                requested_model: None,
                requested_model_sequence: None,
                observed_model: None,
                observed_model_sequence: None,
            });
            self.last_ordinal = Some(ordinal);
            self.last_boundary_was_terminal = false;
        }

        if terminal_outcome.is_some() && self.current.is_none() {
            let ordinal = self.last_ordinal.unwrap_or(0) + 1;
            self.current = Some(PendingTurn {
                ordinal,
                turn_id: signal.turn_id.map(str::to_owned),
                start_sequence: None,
                start_timestamp: None,
                start_kind: "unknown".into(),
                resumed: false,
                requested_model: None,
                requested_model_sequence: None,
                observed_model: None,
                observed_model_sequence: None,
            });
            self.last_ordinal = Some(ordinal);
        }

        if let Some(current) = &mut self.current
            && current.turn_id.is_none()
            && signal.turn_id.is_some()
        {
            current.turn_id = signal.turn_id.map(str::to_owned);
        }
        let is_model_change = signal.requested_model.is_some()
            && signal.kind == "requested_model"
            && self.current.as_ref().is_some_and(|current| {
                current
                    .requested_model
                    .as_deref()
                    .is_some_and(|previous| Some(previous) != signal.requested_model)
            });
        if signal.kind == "requested_model"
            && let Some(requested_model) = signal.requested_model
            && let Some(current) = &mut self.current
        {
            current.requested_model = Some(requested_model.to_owned());
            current.requested_model_sequence = Some(sequence);
        }
        if signal.kind == "served_model_fallback"
            && let Some(served_model) = signal.served_model
            && let Some(current) = &mut self.current
        {
            current.observed_model = Some(served_model.to_owned());
            current.observed_model_sequence = Some(sequence);
        }
        let ordinal = self.current.as_ref().map(|turn| turn.ordinal);

        if let Some(outcome) = terminal_outcome
            && let Some(turn) = self.current.take()
        {
            finalized = Some(turn.finish_terminal(sequence, timestamp, outcome));
            self.last_boundary_was_terminal = true;
        }

        TurnUpdate {
            ordinal,
            finalized,
            opened: should_open,
            is_resumption,
            is_model_change,
        }
    }

    pub(crate) fn active_ordinal(&self) -> Option<i64> {
        self.current.as_ref().map(|turn| turn.ordinal)
    }

    pub(crate) fn finish(&mut self) -> Option<FinalizedTurn> {
        self.current.take().map(PendingTurn::finish_unclosed)
    }
}

impl PendingTurn {
    fn finish_unclosed(self) -> FinalizedTurn {
        self.finish(None, None, "unclosed")
    }

    fn finish_terminal(
        self,
        sequence: i64,
        timestamp: Option<&str>,
        outcome: &'static str,
    ) -> FinalizedTurn {
        self.finish(Some(sequence), timestamp.map(str::to_owned), outcome)
    }

    fn finish(
        self,
        end_sequence: Option<i64>,
        end_timestamp: Option<String>,
        outcome: &'static str,
    ) -> FinalizedTurn {
        FinalizedTurn {
            ordinal: self.ordinal,
            turn_id: self.turn_id,
            start_sequence: self.start_sequence,
            start_timestamp: self.start_timestamp,
            start_kind: self.start_kind,
            end_sequence,
            end_timestamp,
            outcome,
            resumed: self.resumed,
            requested_model: self.requested_model,
            requested_model_sequence: self.requested_model_sequence,
            observed_model: self.observed_model,
            observed_model_sequence: self.observed_model_sequence,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signal<'a>(kind: &'a str, turn_id: Option<&'a str>) -> TurnSignal<'a> {
        TurnSignal {
            kind,
            turn_id,
            requested_model: None,
            served_model: None,
        }
    }

    #[test]
    fn terminal_then_activity_opens_a_resumed_turn() {
        let mut tracker = TurnTracker::default();
        assert_eq!(
            tracker
                .observe(1, Some("later"), signal("started", Some("one")))
                .ordinal,
            Some(1)
        );
        let closed = tracker.observe(2, Some("earlier"), signal("completed", Some("one")));
        assert_eq!(closed.finalized.unwrap().outcome, "complete");
        assert_eq!(tracker.active_ordinal(), None);
        let resumed = tracker.observe(3, None, signal("user_activity", None));
        assert_eq!(resumed.ordinal, Some(2));
        assert!(resumed.is_resumption);
        assert!(tracker.finish().unwrap().resumed);
    }

    #[test]
    fn distinct_context_ids_finalize_an_unclosed_previous_turn() {
        let mut tracker = TurnTracker::default();
        tracker.observe(1, None, signal("requested_model", Some("one")));
        let next = tracker.observe(2, None, signal("requested_model", Some("two")));
        assert_eq!(next.ordinal, Some(2));
        assert_eq!(next.finalized.unwrap().outcome, "unclosed");
        assert!(!next.is_resumption);
    }

    #[test]
    fn a_new_explicit_boundary_preserves_the_prior_turn_as_unclosed() {
        let mut tracker = TurnTracker::default();
        tracker.observe(1, Some("first"), signal("started", None));
        let next = tracker.observe(2, Some("second"), signal("started", None));
        let prior = next.finalized.unwrap();
        assert_eq!(prior.start_sequence, Some(1));
        assert_eq!(prior.end_sequence, None);
        assert_eq!(prior.outcome, "unclosed");
        assert_eq!(next.ordinal, Some(2));
        assert!(!next.is_resumption);
    }

    #[test]
    fn terminal_without_opening_is_retained() {
        let mut tracker = TurnTracker::default();
        let terminal = tracker.observe(1, None, signal("interrupted", None));
        let turn = terminal.finalized.unwrap();
        assert_eq!(turn.ordinal, 1);
        assert_eq!(turn.start_sequence, None);
        assert_eq!(turn.outcome, "interrupted");
    }

    #[test]
    fn requested_model_changes_are_distinct_from_served_fallbacks() {
        let mut tracker = TurnTracker::default();
        let first = tracker.observe(
            1,
            None,
            TurnSignal {
                kind: "requested_model",
                turn_id: Some("turn"),
                requested_model: Some("requested-a"),
                served_model: None,
            },
        );
        assert!(!first.is_model_change);
        let changed = tracker.observe(
            2,
            None,
            TurnSignal {
                kind: "requested_model",
                turn_id: Some("turn"),
                requested_model: Some("requested-b"),
                served_model: None,
            },
        );
        assert!(changed.is_model_change);
        let fallback = tracker.observe(
            3,
            None,
            TurnSignal {
                kind: "served_model_fallback",
                turn_id: Some("turn"),
                requested_model: Some("requested-b"),
                served_model: Some("served-c"),
            },
        );
        assert!(!fallback.is_model_change);
        let turn = tracker.finish().unwrap();
        assert_eq!(turn.requested_model.as_deref(), Some("requested-b"));
        assert_eq!(turn.observed_model.as_deref(), Some("served-c"));
    }
}
