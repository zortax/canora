//! How a request reaches the Web API and what comes back.
//!
//! Every call goes through [`Http::request`]. It adds the token, sends the request, and reads the
//! reply. A refused token is replaced once and the request goes again. A rate limit waits for the
//! period Spotify asks for and goes again, up to a small number of tries.

use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::api::ApiError;
use crate::api::token::TokenManager;

/// Where the Web API lives.
pub const BASE: &str = "https://api.spotify.com/v1";

/// How many times a rate-limited request goes again.
const RATE_LIMIT_TRIES: u8 = 2;

/// The longest a rate limit is obeyed before it is reported instead.
///
/// A long wait leaves the interface looking broken. Tell the caller and let it decide.
const MAX_BACKOFF: Duration = Duration::from_secs(10);

/// What a request carries, beyond the address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// Read.
    Get,
    /// Create.
    Post,
    /// Replace.
    Put,
    /// Remove.
    Delete,
}

impl From<Method> for reqwest::Method {
    fn from(method: Method) -> Self {
        match method {
            Method::Get => Self::GET,
            Method::Post => Self::POST,
            Method::Put => Self::PUT,
            Method::Delete => Self::DELETE,
        }
    }
}

/// The transport every endpoint goes through.
pub struct Http {
    client: reqwest::Client,
    tokens: TokenManager,
}

impl Http {
    /// Sends requests for this session.
    pub fn new(client: reqwest::Client, tokens: TokenManager) -> Self {
        Self { client, tokens }
    }

    /// The tokens this transport signs its requests with.
    pub fn tokens(&self) -> &TokenManager {
        &self.tokens
    }

    /// Sends one request and reads the reply as `T`.
    ///
    /// `path` is relative to the API root. `body` goes out as JSON when it is present.
    pub async fn request<T, B>(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&B>,
    ) -> Result<T, ApiError>
    where
        T: DeserializeOwned,
        B: Serialize + ?Sized,
    {
        let text = self.send(method, path, query, body).await?;
        // An endpoint that answers with no content still has to produce a value. `null` is what
        // the unit type reads from.
        let text = if text.trim().is_empty() {
            "null".to_owned()
        } else {
            text
        };
        Ok(serde_json::from_str(&text)?)
    }

    /// Sends one request and returns the reply as text.
    pub async fn text(&self, method: Method, path: &str) -> Result<String, ApiError> {
        self.send::<serde_json::Value>(method, path, &[], None).await
    }

    /// Sends one request and drops the reply.
    pub async fn call<B>(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&B>,
    ) -> Result<(), ApiError>
    where
        B: Serialize + ?Sized,
    {
        self.send(method, path, query, body).await.map(drop)
    }

    /// Sends one request and returns the body as text.
    async fn send<B>(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&B>,
    ) -> Result<String, ApiError>
    where
        B: Serialize + ?Sized,
    {
        let url = if path.starts_with("http") {
            path.to_owned()
        } else {
            format!("{BASE}{path}")
        };

        let mut refreshed = false;
        let mut rate_limited = 0;

        loop {
            let bearer = self.tokens.bearer().await?;
            let mut request = self
                .client
                .request(method.into(), &url)
                .bearer_auth(&bearer)
                .query(query);
            if let Some(body) = body {
                request = request.json(body);
            }

            // Every request the interface makes, in order. `RUST_LOG=canora::api::http=debug`
            // is how a view that reads too much is caught.
            tracing::debug!(?method, %path, "request");
            let response = request.send().await?;
            let status = response.status();

            if status.is_success() {
                return Ok(response.text().await?);
            }

            match status.as_u16() {
                // The token was refused. Take a new one and go again, once.
                401 if !refreshed => {
                    refreshed = true;
                    self.tokens.forget().await;
                }
                429 if rate_limited < RATE_LIMIT_TRIES => {
                    let wait = retry_after(&response);
                    if wait > MAX_BACKOFF {
                        return Err(ApiError::RateLimited {
                            retry_after: Some(wait),
                        });
                    }
                    rate_limited += 1;
                    tracing::warn!(?wait, %url, "rate limited");
                    tokio::time::sleep(wait).await;
                }
                429 => {
                    return Err(ApiError::RateLimited {
                        retry_after: Some(retry_after(&response)),
                    });
                }
                403 => {
                    let reason = response
                        .text()
                        .await
                        .ok()
                        .and_then(|text| spotify_message(&text))
                        .unwrap_or_default();
                    return Err(ApiError::Forbidden { reason });
                }
                404 => return Err(ApiError::NotFound),
                code => {
                    let message = response
                        .text()
                        .await
                        .ok()
                        .and_then(|text| spotify_message(&text))
                        .unwrap_or_else(|| status.to_string());
                    return Err(ApiError::Status {
                        status: code,
                        message,
                    });
                }
            }
        }
    }
}

/// How long Spotify asks a rate-limited caller to wait.
fn retry_after(response: &reqwest::Response) -> Duration {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map_or(Duration::from_secs(1), Duration::from_secs)
}

/// The message out of an error body, when there is one.
fn spotify_message(text: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    value
        .get("error")?
        .get("message")?
        .as_str()
        .map(ToOwned::to_owned)
}
