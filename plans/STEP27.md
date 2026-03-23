# STEP 27 — Retry Logic with Backoff

## Objective
Implement robust retry logic across all providers with exponential backoff, rate limit detection, and configurable retry policies.

## Prerequisites
- STEP 05 complete (common.rs with basic retry)

## Detailed Instructions

### 27.1 Enhanced retry logic (`src/provider/common.rs`)

Expand the existing `with_retry` to be more sophisticated:

```rust
/// Retry configuration.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts (not counting initial attempt).
    pub max_retries: u32,
    /// Initial backoff delay.
    pub initial_backoff: Duration,
    /// Maximum backoff delay.
    pub max_backoff: Duration,
    /// Backoff multiplier (typically 2.0 for exponential).
    pub multiplier: f64,
    /// Add random jitter to prevent thundering herd.
    pub jitter: bool,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(60),
            multiplier: 2.0,
            jitter: true,
        }
    }
}

/// Enhanced retry with exponential backoff and jitter.
pub async fn with_retry<F, Fut, T>(
    config: &RetryConfig,
    f: F,
) -> anyhow::Result<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<T>>,
{
    let mut last_error = None;
    for attempt in 0..=config.max_retries {
        match f().await {
            Ok(val) => return Ok(val),
            Err(err) => {
                let is_retriable = classify_error(&err);
                if !is_retriable || attempt == config.max_retries {
                    return Err(err);
                }

                let backoff = calculate_backoff(config, attempt, &err);
                tracing::warn!(
                    attempt = attempt + 1,
                    max = config.max_retries,
                    backoff_ms = backoff.as_millis(),
                    error = %err,
                    "Retrying after error"
                );
                tokio::time::sleep(backoff).await;
                last_error = Some(err);
            }
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Retry exhausted with no error")))
}

/// Classify whether an error is retriable.
fn classify_error(err: &anyhow::Error) -> bool {
    if let Some(pe) = err.downcast_ref::<ProviderError>() {
        return pe.is_retriable();
    }
    // Check for reqwest errors
    if let Some(re) = err.downcast_ref::<reqwest::Error>() {
        return re.is_timeout() || re.is_connect() || re.status().map(|s| s.is_server_error()).unwrap_or(false);
    }
    false
}

/// Calculate backoff delay for a given attempt.
fn calculate_backoff(config: &RetryConfig, attempt: u32, err: &anyhow::Error) -> Duration {
    // Check if error specifies a retry-after delay
    if let Some(pe) = err.downcast_ref::<ProviderError>() {
        if let ProviderError::RateLimit { retry_after: Some(delay) } = pe {
            return *delay;
        }
    }

    // Exponential backoff
    let base = config.initial_backoff.as_millis() as f64
        * config.multiplier.powi(attempt as i32);
    let capped = base.min(config.max_backoff.as_millis() as f64);

    // Add jitter (±25%)
    let delay = if config.jitter {
        let jitter_range = capped * 0.25;
        let jitter = (rand_simple() * 2.0 - 1.0) * jitter_range; // ±25%
        (capped + jitter).max(0.0)
    } else {
        capped
    };

    Duration::from_millis(delay as u64)
}

/// Simple pseudo-random number (0.0-1.0) without external dependency.
fn rand_simple() -> f64 {
    use std::time::SystemTime;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    (nanos as f64) / (u32::MAX as f64)
}
```

### 27.2 Rate limit header extraction

Parse `Retry-After` and `x-ratelimit-reset-*` headers from API responses:

```rust
/// Extract retry-after delay from HTTP response headers.
pub fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    // Check Retry-After header (seconds or HTTP date)
    if let Some(val) = headers.get("retry-after") {
        if let Ok(s) = val.to_str() {
            if let Ok(secs) = s.parse::<u64>() {
                return Some(Duration::from_secs(secs));
            }
        }
    }
    // Check x-ratelimit-reset-requests header
    if let Some(val) = headers.get("x-ratelimit-reset-requests") {
        if let Ok(s) = val.to_str() {
            if let Ok(ms) = s.parse::<u64>() {
                return Some(Duration::from_millis(ms));
            }
        }
    }
    None
}
```

### 27.3 Apply retry to all providers

Wrap each provider's `create_message` with retry:
```rust
async fn create_message(&self, ...) -> anyhow::Result<ProviderStream> {
    // Don't retry the stream itself — retry the connection/initial request
    // If the stream fails mid-way, that's handled by the agent (re-calls API)
    with_retry(&RetryConfig::default(), || async {
        self.create_message_inner(system_prompt, messages, tools, config).await
    }).await
}
```

### 27.4 Display retry status in TUI

When retrying, show a brief message in the chat:
```
 ⟳ Rate limited. Retrying in 5s... (attempt 2/3)
```

## Tests

```rust
#[cfg(test)]
mod retry_tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test]
    async fn test_retry_succeeds_first_try() {
        let config = RetryConfig { max_retries: 3, ..Default::default() };
        let result = with_retry(&config, || async { Ok::<_, anyhow::Error>(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn test_retry_succeeds_after_failures() {
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();
        let config = RetryConfig {
            max_retries: 3,
            initial_backoff: Duration::from_millis(10),
            ..Default::default()
        };
        let result = with_retry(&config, || {
            let att = attempts_clone.clone();
            async move {
                let n = att.fetch_add(1, Ordering::Relaxed);
                if n < 2 {
                    Err(ProviderError::Server { status: 500, message: "fail".to_string() }.into())
                } else {
                    Ok(42)
                }
            }
        }).await;
        assert_eq!(result.unwrap(), 42);
        assert_eq!(attempts.load(Ordering::Relaxed), 3);
    }

    #[tokio::test]
    async fn test_retry_exhausted() {
        let config = RetryConfig {
            max_retries: 2,
            initial_backoff: Duration::from_millis(1),
            ..Default::default()
        };
        let result = with_retry(&config, || async {
            Err::<i32, _>(ProviderError::Server { status: 500, message: "fail".to_string() }.into())
        }).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_non_retriable_error_fails_immediately() {
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();
        let config = RetryConfig { max_retries: 3, ..Default::default() };
        let _ = with_retry(&config, || {
            let att = attempts_clone.clone();
            async move {
                att.fetch_add(1, Ordering::Relaxed);
                Err::<i32, _>(ProviderError::Auth("bad key".to_string()).into())
            }
        }).await;
        assert_eq!(attempts.load(Ordering::Relaxed), 1); // No retry
    }

    #[test]
    fn test_calculate_backoff_exponential() {
        let config = RetryConfig {
            initial_backoff: Duration::from_secs(1),
            multiplier: 2.0,
            max_backoff: Duration::from_secs(60),
            jitter: false,
            ..Default::default()
        };
        let err = anyhow::anyhow!("test");
        let b0 = calculate_backoff(&config, 0, &err);
        let b1 = calculate_backoff(&config, 1, &err);
        let b2 = calculate_backoff(&config, 2, &err);
        assert_eq!(b0, Duration::from_secs(1));
        assert_eq!(b1, Duration::from_secs(2));
        assert_eq!(b2, Duration::from_secs(4));
    }

    #[test]
    fn test_backoff_capped_at_max() {
        let config = RetryConfig {
            initial_backoff: Duration::from_secs(1),
            multiplier: 2.0,
            max_backoff: Duration::from_secs(5),
            jitter: false,
            ..Default::default()
        };
        let err = anyhow::anyhow!("test");
        let b10 = calculate_backoff(&config, 10, &err);
        assert_eq!(b10, Duration::from_secs(5));
    }

    #[test]
    fn test_parse_retry_after_seconds() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("retry-after", "5".parse().unwrap());
        assert_eq!(parse_retry_after(&headers), Some(Duration::from_secs(5)));
    }

    #[test]
    fn test_parse_retry_after_missing() {
        let headers = reqwest::header::HeaderMap::new();
        assert_eq!(parse_retry_after(&headers), None);
    }
}
```

## Acceptance Criteria
- [x] Exponential backoff with configurable parameters
- [x] Jitter to prevent thundering herd
- [x] Rate limit retry-after header respected
- [x] Non-retriable errors fail immediately (401, 400)
- [x] Retriable errors retry up to max_retries (429, 500+, connection errors)
- [x] Backoff capped at max_backoff
- [ ] Retry status shown in TUI
- [x] `cargo clippy -- -D warnings` passes
- [x] All tests pass (10+ cases)
