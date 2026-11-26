use chrono::{DateTime, Duration, Utc};
use saint_harness_schema::Usage;

#[derive(Debug, Clone)]
pub struct CacheMeter {
    ttl_seconds: u64,
    last_hit_at: Option<DateTime<Utc>>,
    last_prefix_hash: Option<String>,
    last_hit_ratio: f64,
    total_input: u64,
    total_cache_read: u64,
    total_cache_write: u64,
}

impl CacheMeter {
    pub fn new(ttl_seconds: u64) -> Self {
        Self {
            ttl_seconds,
            last_hit_at: None,
            last_prefix_hash: None,
            last_hit_ratio: 0.0,
            total_input: 0,
            total_cache_read: 0,
            total_cache_write: 0,
        }
    }

    pub fn record_step(&mut self, prefix_hash: &str, usage: &Usage, now: DateTime<Utc>) -> bool {
        let prefix_preserved = self
            .last_prefix_hash
            .as_deref()
            .is_some_and(|prev| prev == prefix_hash);
        self.last_prefix_hash = Some(prefix_hash.to_string());
        self.total_input = self.total_input.saturating_add(usage.input_tokens);
        self.total_cache_read = self
            .total_cache_read
            .saturating_add(usage.cache_read_input_tokens);
        self.total_cache_write = self
            .total_cache_write
            .saturating_add(usage.cache_write_input_tokens);
        self.last_hit_ratio = usage.cache_hit_ratio();
        if usage.cache_read_input_tokens > 0 || usage.cache_write_input_tokens > 0 {
            self.last_hit_at = Some(now);
        }
        prefix_preserved
    }

    pub fn ttl_remaining_secs(&self, now: DateTime<Utc>) -> u64 {
        let Some(last) = self.last_hit_at else {
            return 0;
        };
        let expires = last + Duration::seconds(self.ttl_seconds as i64);
        let diff = expires.signed_duration_since(now).num_seconds();
        if diff <= 0 { 0 } else { diff as u64 }
    }

    pub fn is_warm(&self, now: DateTime<Utc>) -> bool {
        self.ttl_remaining_secs(now) > 0
    }

    pub fn last_hit_ratio(&self) -> f64 {
        self.last_hit_ratio
    }

    /// Positive return value indicates retroactive pruning saves more tokens over the next
    /// `expected_future_turns` than the one-time cache rewrite penalty of invalidating `prefix_tokens`.
    pub fn break_even_saving(
        &self,
        reclaimable_tokens: u64,
        prefix_tokens_invalidated: u64,
        expected_future_turns: u64,
    ) -> i64 {
        // Cache read costs ~0.1x base input; cache write costs ~1.25x base input.
        let rewrite_penalty = ((prefix_tokens_invalidated as f64) * 1.15) as i64;
        let recurring_saving =
            ((reclaimable_tokens as f64) * 0.10 * (expected_future_turns as f64)) as i64;
        recurring_saving - rewrite_penalty
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_meter_tracks_warm_ttl_and_cold_expiry() {
        let mut meter = CacheMeter::new(300);
        let t0 = Utc::now();
        let usage = Usage {
            input_tokens: 10_000,
            non_cached_input_tokens: 1_200,
            cache_read_input_tokens: 8_800,
            cache_write_input_tokens: 0,
            output_tokens: 300,
            reasoning_tokens: 0,
            provider_reported_cost_usd: None,
        };
        assert!(!meter.record_step("h1", &usage, t0));
        assert!(meter.record_step("h1", &usage, t0 + Duration::seconds(10)));
        assert!(meter.is_warm(t0 + Duration::seconds(200)));
        assert!(!meter.is_warm(t0 + Duration::seconds(320)));
    }
}
