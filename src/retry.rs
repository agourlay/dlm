use crate::{DlmError, ProgressBarManager};
use std::time::Duration;
use tokio_retry::strategy::ExponentialBackoff;

pub fn retry_strategy(max_attempts: usize) -> impl Iterator<Item = Duration> {
    // usually implemented as `interval * factor^retry`
    // but tokio-retry does `interval^retry * factor`
    ExponentialBackoff::from_millis(10) // base interval in `interval^retry`
        .max_delay(Duration::from_secs(10 * 60)) // max 10 minutes
        .factor(1)
        .take(max_attempts) // limit retries
}

pub fn retry_handler(e: &DlmError, pbm: &ProgressBarManager, link: &str) -> bool {
    let should_retry = is_network_error(e);
    if should_retry {
        let msg = format!("Scheduling retry for {} after error {}", link, e);
        pbm.log_above_progress_bars(&msg);
    }
    should_retry
}

fn is_network_error(e: &DlmError) -> bool {
    matches!(
        e,
        DlmError::ConnectionClosed
            | DlmError::ConnectionTimeout
            | DlmError::ResponseBodyError
            | DlmError::DeadLineElapsedTimeout
    )
}

#[cfg(test)]
mod retry_tests {
    use super::*;

    #[test]
    fn retry_strategy_values() {
        // no jitter for determinism
        let mut s = retry_strategy(10);
        // http://exponentialbackoffcalculator.com/
        assert_eq!(s.next(), Some(Duration::from_millis(10)));
        assert_eq!(s.next(), Some(Duration::from_millis(100)));
        assert_eq!(s.next(), Some(Duration::from_secs(1)));
        assert_eq!(s.next(), Some(Duration::from_secs(10)));
        assert_eq!(s.next(), Some(Duration::from_secs(100)));
        assert_eq!(s.next(), Some(Duration::from_secs(600)));
        assert_eq!(s.next(), Some(Duration::from_secs(600)));
        assert_eq!(s.next(), Some(Duration::from_secs(600)));
        assert_eq!(s.next(), Some(Duration::from_secs(600)));
        assert_eq!(s.next(), Some(Duration::from_secs(600)));
        assert_eq!(s.next(), None);
    }
}
