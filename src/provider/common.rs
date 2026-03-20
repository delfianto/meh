//! Shared HTTP client, retry logic, and error types for all providers.

use reqwest::Client;
use std::time::Duration;
use thiserror::Error;

/// Errors specific to provider operations.
#[derive(Error, Debug)]
pub enum ProviderError {
    #[error("Authentication failed: {0}")]
    Auth(String),

    #[error("Rate limited: retry after {retry_after:?}")]
    RateLimit { retry_after: Option<Duration> },

    #[error("Server error ({status}): {message}")]
    Server { status: u16, message: String },

    #[error("Request failed: {0}")]
    Request(#[from] reqwest::Error),

    #[error("Stream error: {0}")]
    Stream(String),

    #[error("Invalid response: {0}")]
    InvalidResponse(String),
}

impl ProviderError {
    /// Whether this error is retriable (rate limits and 5xx errors).
    pub const fn is_retriable(&self) -> bool {
        match self {
            Self::RateLimit { .. } => true,
            Self::Server { status, .. } => *status >= 500,
            _ => false,
        }
    }
}

/// Creates a configured HTTP client with sensible timeouts.
pub fn create_http_client() -> reqwest::Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(300))
        .connect_timeout(Duration::from_secs(10))
        .pool_max_idle_per_host(5)
        .build()
}

/// Retries an async operation with exponential backoff.
///
/// On retriable errors (rate limit, 5xx), waits and retries up to
/// `max_retries` times. On non-retriable errors, returns immediately.
/// For rate limits with a `retry_after` duration, uses that value;
/// otherwise uses exponential backoff: 1s, 2s, 4s, ...
pub async fn with_retry<F, Fut, T>(max_retries: u32, f: F) -> anyhow::Result<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<T>>,
{
    let mut last_err = None;
    for attempt in 0..=max_retries {
        match f().await {
            Ok(val) => return Ok(val),
            Err(e) => {
                if let Some(pe) = e.downcast_ref::<ProviderError>() {
                    if !pe.is_retriable() || attempt == max_retries {
                        return Err(e);
                    }
                    let delay = match pe {
                        ProviderError::RateLimit {
                            retry_after: Some(d),
                        } => *d,
                        _ => Duration::from_millis(1000 * 2u64.pow(attempt)),
                    };
                    tracing::warn!(attempt, ?delay, "Retriable error, backing off");
                    tokio::time::sleep(delay).await;
                } else if attempt == max_retries {
                    return Err(e);
                }
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("Retry exhausted")))
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[tokio::test]
    async fn with_retry_succeeds_first_try() {
        let result = with_retry(3, || async { Ok::<_, anyhow::Error>(42) }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn with_retry_fails_non_retriable() {
        let result = with_retry(3, || async {
            Err::<i32, _>(ProviderError::Auth("bad".to_string()).into())
        })
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn with_retry_fails_after_max() {
        let result = with_retry(1, || async {
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

    #[test]
    fn create_http_client_succeeds() {
        let client = create_http_client();
        assert!(client.is_ok());
    }
}
