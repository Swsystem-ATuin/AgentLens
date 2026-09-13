use crate::sources::TokenUsage;

#[derive(Default)]
struct Counter {
    last: Option<i64>,
    total: Option<i64>,
    overflowed: bool,
}

impl Counter {
    fn observe(&mut self, value: Option<i64>) -> (Option<i64>, bool, bool) {
        let Some(value) = value else {
            return (None, false, false);
        };
        if value < 0 {
            return (None, false, true);
        }
        let (delta, reset) = match self.last {
            Some(last) if value >= last => (value - last, false),
            Some(_) => (value, true),
            None => (value, false),
        };
        self.last = Some(value);
        if !self.overflowed {
            self.total = self.total.unwrap_or(0).checked_add(delta);
            self.overflowed = self.total.is_none();
        }
        (Some(delta), reset, self.overflowed)
    }
}

#[derive(Default)]
pub(crate) struct UsageAccounting {
    input: Counter,
    output: Counter,
    cached_input: Counter,
    reasoning_output: Counter,
    total: Counter,
    previous: Option<TokenUsage>,
    pub(crate) snapshot_count: i64,
    pub(crate) per_turn_observation_count: i64,
    pub(crate) reset_count: i64,
    pub(crate) duplicate_count: i64,
    pub(crate) missing_count: i64,
    pub(crate) inconsistent_count: i64,
}

pub(crate) struct ProcessedUsage {
    pub(crate) delta: TokenUsage,
    pub(crate) cumulative_present: bool,
    pub(crate) per_turn_present: bool,
    pub(crate) duplicate: bool,
    pub(crate) reset: bool,
    pub(crate) missing: bool,
    pub(crate) inconsistent: bool,
}

pub(crate) struct UsageSummary {
    pub(crate) usage: TokenUsage,
    pub(crate) snapshot_count: i64,
    pub(crate) per_turn_observation_count: i64,
    pub(crate) reset_count: i64,
    pub(crate) duplicate_count: i64,
    pub(crate) missing_count: i64,
    pub(crate) inconsistent_count: i64,
}

impl UsageAccounting {
    pub(crate) fn observe(
        &mut self,
        cumulative: Option<&TokenUsage>,
        per_turn: Option<&TokenUsage>,
    ) -> ProcessedUsage {
        self.snapshot_count += 1;
        self.per_turn_observation_count += i64::from(per_turn.is_some());
        let duplicate = cumulative.is_some() && cumulative == self.previous.as_ref();
        let missing = cumulative.is_none_or(TokenUsage::has_missing_core);
        let mut inconsistent = cumulative.is_some_and(TokenUsage::is_inconsistent)
            || per_turn.is_some_and(TokenUsage::is_inconsistent);
        let (delta, reset) = if let Some(usage) = cumulative {
            let (input, input_reset, input_invalid) = self.input.observe(usage.input_tokens);
            let (output, output_reset, output_invalid) = self.output.observe(usage.output_tokens);
            let (cached_input, cached_reset, cached_invalid) =
                self.cached_input.observe(usage.cached_input_tokens);
            let (reasoning_output, reasoning_reset, reasoning_invalid) =
                self.reasoning_output.observe(usage.reasoning_output_tokens);
            let (total, total_reset, total_invalid) = self.total.observe(usage.total_tokens);
            inconsistent |= input_invalid
                || output_invalid
                || cached_invalid
                || reasoning_invalid
                || total_invalid;
            (
                TokenUsage {
                    input_tokens: input,
                    output_tokens: output,
                    cached_input_tokens: cached_input,
                    reasoning_output_tokens: reasoning_output,
                    total_tokens: total,
                },
                input_reset || output_reset || cached_reset || reasoning_reset || total_reset,
            )
        } else {
            (TokenUsage::default(), false)
        };
        self.previous = cumulative.cloned();
        self.duplicate_count += i64::from(duplicate);
        self.reset_count += i64::from(reset);
        self.missing_count += i64::from(missing);
        self.inconsistent_count += i64::from(inconsistent);
        ProcessedUsage {
            delta,
            cumulative_present: cumulative.is_some(),
            per_turn_present: per_turn.is_some(),
            duplicate,
            reset,
            missing,
            inconsistent,
        }
    }

    pub(crate) fn summary(&self) -> UsageSummary {
        UsageSummary {
            usage: TokenUsage {
                input_tokens: self.input.total,
                output_tokens: self.output.total,
                cached_input_tokens: self.cached_input.total,
                reasoning_output_tokens: self.reasoning_output.total,
                total_tokens: self.total.total,
            },
            snapshot_count: self.snapshot_count,
            per_turn_observation_count: self.per_turn_observation_count,
            reset_count: self.reset_count,
            duplicate_count: self.duplicate_count,
            missing_count: self.missing_count,
            inconsistent_count: self.inconsistent_count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(
        input: Option<i64>,
        output: Option<i64>,
        cached: Option<i64>,
        total: Option<i64>,
    ) -> TokenUsage {
        TokenUsage {
            input_tokens: input,
            output_tokens: output,
            cached_input_tokens: cached,
            reasoning_output_tokens: None,
            total_tokens: total,
        }
    }

    #[test]
    fn cumulative_accounting_handles_duplicates_resets_and_missing_fields() {
        let mut accounting = UsageAccounting::default();
        let first = usage(Some(10), Some(4), Some(3), Some(14));
        assert_eq!(
            accounting.observe(Some(&first), None).delta.total_tokens,
            Some(14)
        );
        let duplicate = accounting.observe(Some(&first), None);
        assert!(duplicate.duplicate);
        assert_eq!(duplicate.delta.total_tokens, Some(0));
        let partial = usage(Some(16), None, Some(5), None);
        assert!(accounting.observe(Some(&partial), None).missing);
        let resumed = usage(Some(20), Some(9), Some(6), Some(29));
        assert_eq!(
            accounting.observe(Some(&resumed), None).delta.output_tokens,
            Some(5)
        );
        let reset = usage(Some(2), Some(1), Some(1), Some(3));
        assert!(accounting.observe(Some(&reset), None).reset);
        let summary = accounting.summary();
        assert_eq!(summary.usage.input_tokens, Some(22));
        assert_eq!(summary.usage.output_tokens, Some(10));
        assert_eq!(summary.usage.cached_input_tokens, Some(7));
        assert_eq!(summary.usage.total_tokens, Some(32));
        assert_eq!(
            (
                summary.duplicate_count,
                summary.reset_count,
                summary.missing_count
            ),
            (1, 1, 1)
        );
    }

    #[test]
    fn subset_and_reported_total_inconsistencies_are_flagged_not_recomputed() {
        let mut accounting = UsageAccounting::default();
        let cumulative = TokenUsage {
            input_tokens: Some(5),
            output_tokens: Some(3),
            cached_input_tokens: Some(6),
            reasoning_output_tokens: Some(4),
            total_tokens: Some(99),
        };
        let processed = accounting.observe(Some(&cumulative), Some(&cumulative));
        assert!(processed.inconsistent);
        let summary = accounting.summary();
        assert_eq!(summary.usage.total_tokens, Some(99));
        assert_eq!(summary.usage.cached_input_tokens, Some(6));
        assert_eq!(summary.usage.reasoning_output_tokens, Some(4));
        assert_eq!(summary.inconsistent_count, 1);
        assert_eq!(summary.per_turn_observation_count, 1);
    }

    #[test]
    fn unavailable_fields_stay_null_and_per_turn_observations_are_not_additive() {
        let mut accounting = UsageAccounting::default();
        let cumulative = usage(None, None, None, Some(12));
        let per_turn = usage(Some(100), Some(50), Some(20), Some(150));
        let processed = accounting.observe(Some(&cumulative), Some(&per_turn));
        assert!(processed.missing);
        let summary = accounting.summary();
        assert_eq!(summary.usage.input_tokens, None);
        assert_eq!(summary.usage.output_tokens, None);
        assert_eq!(summary.usage.cached_input_tokens, None);
        assert_eq!(summary.usage.reasoning_output_tokens, None);
        assert_eq!(summary.usage.total_tokens, Some(12));
        assert_eq!(summary.per_turn_observation_count, 1);
    }
}
