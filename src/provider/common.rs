//! Shared HTTP client, retry logic, and error types for all providers.
//!
//! Provides `RetryConfig` for configurable exponential backoff with jitter,
//! `ProviderError` for typed error classification, `Retry-After` header
//! parsing, and a shared HTTP client factory.

use reqwest::Client;
use std::time::Duration;
use thiserror::Error;

/// Errors specific to provider operations.
#[derive(Error, Debug)]
pub enum ProviderError {
    /// Authentication/authorization failure (401/403). Not retriable.
    #[error("Authentication failed: {0}")]
    Auth(String),

    /// Rate limit hit (429). Retriable after backoff.
    #[error("Rate limited: retry after {retry_after:?}")]
    RateLimit { retry_after: Option<Duration> },

    /// Server error (5xx). Retriable.
    #[error("Server error ({status}): {message}")]
    Server { status: u16, message: String },

    /// Low-level HTTP error.
    #[error("Request failed: {0}")]
    Request(#[from] reqwest::Error),

    /// Streaming error mid-response.
    #[error("Stream error: {0}")]
    Stream(String),

    /// Malformed or unexpected response body.
    #[error("Invalid response: {0}")]
    InvalidResponse(String),
}

impl ProviderError {
    /// Whether this error is retriable (rate limits, 5xx, connection errors).
    pub const fn is_retriable(&self) -> bool {
        match self {
            Self::RateLimit { .. } => true,
            Self::Server { status, .. } => *status >= 500,
            _ => false,
        }
    }
}

/// Retry configuration with exponential backoff and optional jitter.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts (not counting initial attempt).
    pub max_retries: u32,
    /// Initial backoff delay.
    pub initial_backoff: Duration,
    /// Maximum backoff delay cap.
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

/// Retries an async operation with configurable exponential backoff.
///
/// On retriable errors (rate limit, 5xx, connection/timeout), waits and
/// retries up to `config.max_retries` times. Non-retriable errors (auth,
/// bad request) fail immediately. Rate limit `Retry-After` values are
/// respected when present.
pub async fn with_retry<F, Fut, T>(config: &RetryConfig, f: F) -> anyhow::Result<T>
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
    if let Some(re) = err.downcast_ref::<reqwest::Error>() {
        return re.is_timeout()
            || re.is_connect()
            || re.status().is_some_and(|s| s.is_server_error());
    }
    false
}

/// Calculate backoff delay for a given attempt, respecting `Retry-After`.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::cast_possible_wrap
)]
fn calculate_backoff(config: &RetryConfig, attempt: u32, err: &anyhow::Error) -> Duration {
    if let Some(ProviderError::RateLimit {
        retry_after: Some(delay),
    }) = err.downcast_ref::<ProviderError>()
    {
        return *delay;
    }

    let base = config.initial_backoff.as_millis() as f64 * config.multiplier.powi(attempt as i32);
    let capped = base.min(config.max_backoff.as_millis() as f64);

    let delay = if config.jitter {
        let jitter_range = capped * 0.25;
        let jitter = rand_simple().mul_add(2.0, -1.0) * jitter_range;
        (capped + jitter).max(0.0)
    } else {
        capped
    };

    Duration::from_millis(delay as u64)
}

/// Simple pseudo-random number (0.0–1.0) without external dependency.
fn rand_simple() -> f64 {
    use std::time::SystemTime;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    f64::from(nanos) / f64::from(u32::MAX)
}

/// Extract retry-after delay from HTTP response headers.
pub fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    if let Some(val) = headers.get("retry-after") {
        if let Ok(s) = val.to_str() {
            if let Ok(secs) = s.parse::<u64>() {
                return Some(Duration::from_secs(secs));
            }
        }
    }
    if let Some(val) = headers.get("x-ratelimit-reset-requests") {
        if let Ok(s) = val.to_str() {
            if let Ok(ms) = s.parse::<u64>() {
                return Some(Duration::from_millis(ms));
            }
        }
    }
    None
}

/// Creates a configured HTTP client with sensible timeouts.
pub fn create_http_client() -> reqwest::Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(300))
        .connect_timeout(Duration::from_secs(10))
        .pool_max_idle_per_host(5)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn provider_error_retriable_rate_limit() {
        assert!(ProviderError::RateLimit { retry_after: None }.is_retriable());
        assert!(
            ProviderError::RateLimit {
                retry_after: Some(Duration::from_secs(5))
            }
            .is_retriable()
        );
    }

    #[test]
    fn provider_error_retriable_server() {
        assert!(
            ProviderError::Server {
                status: 500,
                message: "error".to_string()
            }
            .is_retriable()
        );
        assert!(
            ProviderError::Server {
                status: 503,
                message: "unavailable".to_string()
            }
            .is_retriable()
        );
    }

    #[test]
    fn provider_error_not_retriable() {
        assert!(!ProviderError::Auth("bad key".to_string()).is_retriable());
        assert!(
            !ProviderError::Server {
                status: 400,
                message: "bad request".to_string()
            }
            .is_retriable()
        );
        assert!(
            !ProviderError::Server {
                status: 404,
                message: "not found".to_string()
            }
            .is_retriable()
        );
        assert!(!ProviderError::InvalidResponse("bad".to_string()).is_retriable());
    }

    #[test]
    fn provider_error_display() {
        let err = ProviderError::Auth("invalid key".to_string());
        assert_eq!(err.to_string(), "Authentication failed: invalid key");

        let err = ProviderError::RateLimit {
            retry_after: Some(Duration::from_secs(30)),
        };
        assert!(err.to_string().contains("Rate limited"));
    }

    #[test]
    fn retry_config_default() {
        let config = RetryConfig::default();
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.initial_backoff, Duration::from_secs(1));
        assert_eq!(config.max_backoff, Duration::from_secs(60));
        assert!((config.multiplier - 2.0).abs() < f64::EPSILON);
        assert!(config.jitter);
    }

    #[tokio::test]
    async fn retry_succeeds_first_try() {
        let config = RetryConfig::default();
        let result = with_retry(&config, || async { Ok::<_, anyhow::Error>(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn retry_succeeds_after_failures() {
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();
        let config = RetryConfig {
            max_retries: 3,
            initial_backoff: Duration::from_millis(10),
            jitter: false,
            ..Default::default()
        };
        let result = with_retry(&config, || {
            let att = attempts_clone.clone();
            async move {
                let n = att.fetch_add(1, Ordering::Relaxed);
                if n < 2 {
                    Err(ProviderError::Server {
                        status: 500,
                        message: "fail".to_string(),
                    }
                    .into())
                } else {
                    Ok(42)
                }
            }
        })
        .await;
        assert_eq!(result.unwrap(), 42);
        assert_eq!(attempts.load(Ordering::Relaxed), 3);
    }

    #[tokio::test]
    async fn retry_exhausted() {
        let config = RetryConfig {
            max_retries: 2,
            initial_backoff: Duration::from_millis(1),
            jitter: false,
            ..Default::default()
        };
        let result = with_retry(&config, || async {
            Err::<i32, _>(
                ProviderError::Server {
                    status: 500,
                    message: "fail".to_string(),
                }
                .into(),
            )
        })
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn non_retriable_error_fails_immediately() {
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();
        let config = RetryConfig {
            max_retries: 3,
            initial_backoff: Duration::from_millis(10),
            ..Default::default()
        };
        let _ = with_retry(&config, || {
            let att = attempts_clone.clone();
            async move {
                att.fetch_add(1, Ordering::Relaxed);
                Err::<i32, _>(ProviderError::Auth("bad key".to_string()).into())
            }
        })
        .await;
        assert_eq!(attempts.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn calculate_backoff_exponential() {
        let config = RetryConfig {
            initial_backoff: Duration::from_secs(1),
            multiplier: 2.0,
            max_backoff: Duration::from_secs(60),
            jitter: false,
            ..Default::default()
        };
        let err = anyhow::anyhow!("test");
        assert_eq!(calculate_backoff(&config, 0, &err), Duration::from_secs(1));
        assert_eq!(calculate_backoff(&config, 1, &err), Duration::from_secs(2));
        assert_eq!(calculate_backoff(&config, 2, &err), Duration::from_secs(4));
    }

    #[test]
    fn backoff_capped_at_max() {
        let config = RetryConfig {
            initial_backoff: Duration::from_secs(1),
            multiplier: 2.0,
            max_backoff: Duration::from_secs(5),
            jitter: false,
            ..Default::default()
        };
        let err = anyhow::anyhow!("test");
        assert_eq!(calculate_backoff(&config, 10, &err), Duration::from_secs(5));
    }

    #[test]
    fn backoff_with_jitter_varies() {
        let config = RetryConfig {
            initial_backoff: Duration::from_secs(1),
            multiplier: 2.0,
            max_backoff: Duration::from_secs(60),
            jitter: true,
            ..Default::default()
        };
        let err = anyhow::anyhow!("test");
        let b = calculate_backoff(&config, 0, &err);
        assert!(b.as_millis() >= 750);
        assert!(b.as_millis() <= 1250);
    }

    #[test]
    fn backoff_respects_rate_limit_retry_after() {
        let config = RetryConfig {
            jitter: false,
            ..Default::default()
        };
        let err: anyhow::Error = ProviderError::RateLimit {
            retry_after: Some(Duration::from_secs(30)),
        }
        .into();
        assert_eq!(calculate_backoff(&config, 0, &err), Duration::from_secs(30));
    }

    #[test]
    fn parse_retry_after_seconds() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("retry-after", "5".parse().unwrap());
        assert_eq!(parse_retry_after(&headers), Some(Duration::from_secs(5)));
    }

    #[test]
    fn parse_retry_after_ratelimit_header() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-ratelimit-reset-requests", "3000".parse().unwrap());
        assert_eq!(
            parse_retry_after(&headers),
            Some(Duration::from_millis(3000))
        );
    }

    #[test]
    fn parse_retry_after_missing() {
        let headers = reqwest::header::HeaderMap::new();
        assert_eq!(parse_retry_after(&headers), None);
    }

    #[test]
    fn parse_retry_after_invalid() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("retry-after", "not-a-number".parse().unwrap());
        assert_eq!(parse_retry_after(&headers), None);
    }

    #[test]
    fn classify_provider_error_retriable() {
        let err: anyhow::Error = ProviderError::Server {
            status: 502,
            message: "bad gateway".to_string(),
        }
        .into();
        assert!(classify_error(&err));
    }

    #[test]
    fn classify_provider_error_not_retriable() {
        let err: anyhow::Error = ProviderError::Auth("bad".to_string()).into();
        assert!(!classify_error(&err));
    }

    #[test]
    fn classify_unknown_error_not_retriable() {
        let err = anyhow::anyhow!("some random error");
        assert!(!classify_error(&err));
    }

    #[test]
    fn create_http_client_succeeds() {
        let client = create_http_client();
        assert!(client.is_ok());
    }
}
