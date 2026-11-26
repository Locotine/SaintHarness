use saint_harness_schema::AiError;
use std::time::Duration;

pub const GENERAL_BACKOFF_SECS: [u64; 10] = [2, 4, 8, 10, 10, 10, 10, 10, 10, 10];
pub const MAX_TIMEOUT_RETRIES: usize = 3;
pub const NETWORK_MAX_TOTAL_SECS: u64 = 3600;
pub const NETWORK_CAP_SECS: u64 = 30;
pub const RETRY_AFTER_CAP_MS: u64 = 15 * 60 * 1000;

/// Three independent retry budgets (`general`, `network`, `timeout`) honoring `x-should-retry`
/// and `Retry-After` (capped at 15 minutes).
#[derive(Debug, Default, Clone)]
pub struct RetryBudgets {
    pub general_attempts: usize,
    pub network_attempts: usize,
    pub network_elapsed_secs: u64,
    pub timeout_attempts: usize,
}

impl RetryBudgets {
    pub fn next_delay(&mut self, err: &AiError, jitter_factor: f64) -> Option<Duration> {
        let clamped_jitter = jitter_factor.clamp(0.8, 1.2);
        match err {
            AiError::Authentication { .. }
            | AiError::QuotaExceeded { .. }
            | AiError::ContentPolicy { .. }
            | AiError::InvalidRequest { .. }
            | AiError::PayloadTooLarge { .. }
            | AiError::ContextOverflow { .. } => None,

            AiError::Timeout { .. } => {
                if self.timeout_attempts >= MAX_TIMEOUT_RETRIES {
                    return None;
                }
                let base_secs = 2u64 << self.timeout_attempts;
                self.timeout_attempts += 1;
                Some(Duration::from_millis(
                    ((base_secs * 1000) as f64 * clamped_jitter).round() as u64,
                ))
            }

            AiError::Transport {
                network_down: true, ..
            } => {
                if self.network_elapsed_secs >= NETWORK_MAX_TOTAL_SECS {
                    return None;
                }
                let exp = 2u64.saturating_mul(1u64 << self.network_attempts.min(4));
                let secs = exp.min(NETWORK_CAP_SECS);
                self.network_attempts += 1;
                self.network_elapsed_secs = self.network_elapsed_secs.saturating_add(secs);
                Some(Duration::from_millis(
                    ((secs * 1000) as f64 * clamped_jitter).round() as u64,
                ))
            }

            AiError::RateLimit {
                retry_after_ms,
                should_retry_header,
                ..
            }
            | AiError::ProviderInternal {
                retry_after_ms,
                should_retry_header,
                ..
            } => {
                if *should_retry_header == Some(false) {
                    return None;
                }
                if self.general_attempts >= GENERAL_BACKOFF_SECS.len() {
                    return None;
                }
                let base_ms = GENERAL_BACKOFF_SECS[self.general_attempts] * 1000;
                self.general_attempts += 1;
                let ms = retry_after_ms
                    .map(|ra| ra.min(RETRY_AFTER_CAP_MS))
                    .unwrap_or_else(|| ((base_ms as f64) * clamped_jitter).round() as u64);
                Some(Duration::from_millis(ms))
            }

            AiError::IncompleteStream { .. }
            | AiError::Transport {
                network_down: false,
                ..
            }
            | AiError::UnknownProvider { .. } => {
                if self.general_attempts >= GENERAL_BACKOFF_SECS.len() {
                    return None;
                }
                let base_ms = GENERAL_BACKOFF_SECS[self.general_attempts] * 1000;
                self.general_attempts += 1;
                Some(Duration::from_millis(
                    ((base_ms as f64) * clamped_jitter).round() as u64,
                ))
            }
        }
    }
}
