//! Access tokens for the Web API.
//!
//! There are two ways to hold one, and which one is in use follows from the client identifier the
//! login used.
//!
//! * [`TokenSource::App`] holds a refresh token minted for this person's own identifier and
//!   exchanges it for access tokens. The quota belongs to that identifier alone.
//! * [`TokenSource::Session`] takes a token from the streaming session through login5. It needs no
//!   registration, and Spotify rate limits it hard, because every librespot program shares the one
//!   identifier behind it.

use std::time::{Duration, Instant, SystemTime};

use librespot_oauth::OAuthToken;
use tokio::sync::Mutex;

use crate::api::ApiError;
use crate::auth::ClientId;
use crate::session::SessionCell;

/// How long before expiry a token is replaced.
///
/// A token that expires while a request is in flight costs a round trip and a retry. Take the new
/// one a minute early instead.
const EARLY: Duration = Duration::from_secs(60);

/// Where access tokens come from.
pub enum TokenSource {
    /// This person's own registered application.
    App {
        /// Which application.
        client_id: ClientId,
        /// The token that mints access tokens.
        refresh_token: String,
    },
    /// The streaming session.
    Session(SessionCell),
}

/// Holds the Web API token and replaces it when it runs out.
pub struct TokenManager {
    source: TokenSource,
    /// The token in hand, and when it stops working. One request fetches at a time.
    held: Mutex<Option<Held>>,
}

/// A token and its expiry.
#[derive(Debug, Clone)]
struct Held {
    value: String,
    expires_at: SystemTime,
}

impl Held {
    /// Whether this token is still worth sending.
    fn is_fresh(&self) -> bool {
        SystemTime::now() + EARLY < self.expires_at
    }
}

impl TokenManager {
    /// Takes tokens from `source`.
    pub fn new(source: TokenSource) -> Self {
        Self {
            source,
            held: Mutex::new(None),
        }
    }

    /// Starts with the token the browser login just minted.
    ///
    /// This saves one exchange on the run that logs in.
    pub async fn prime(&self, token: &OAuthToken) {
        let remaining = token.expires_at.saturating_duration_since(Instant::now());
        *self.held.lock().await = Some(Held {
            value: token.access_token.clone(),
            expires_at: SystemTime::now() + remaining,
        });
    }

    /// The value for the `Authorization` header.
    ///
    /// Return the token in hand while it is fresh. One caller fetches and the rest wait for it.
    pub async fn bearer(&self) -> Result<String, ApiError> {
        let mut held = self.held.lock().await;
        if let Some(token) = held.as_ref()
            && token.is_fresh()
        {
            return Ok(token.value.clone());
        }

        let fresh = match &self.source {
            TokenSource::App {
                client_id,
                refresh_token,
            } => {
                let client_id = client_id.clone();
                let refresh_token = refresh_token.clone();
                // The exchange blocks a thread. Keep it off the runtime's workers.
                let token = tokio::task::spawn_blocking(move || {
                    crate::auth::oauth_client(&client_id)?.refresh_token(&refresh_token)
                })
                .await
                .map_err(|error| ApiError::Auth(error.to_string()))?
                .map_err(|error| ApiError::Auth(error.to_string()))?;

                let remaining = token.expires_at.saturating_duration_since(Instant::now());
                Held {
                    value: token.access_token,
                    expires_at: SystemTime::now() + remaining,
                }
            }
            TokenSource::Session(session) => {
                let token = session.get().login5().auth_token().await?;
                Held {
                    value: token.access_token,
                    expires_at: token.timestamp + token.expires_in,
                }
            }
        };

        let value = fresh.value.clone();
        *held = Some(fresh);
        Ok(value)
    }

    /// Drops the token in hand.
    ///
    /// Call this when Spotify refuses a token the clock says is still good. The next request takes
    /// a new one.
    pub async fn forget(&self) {
        *self.held.lock().await = None;
    }
}
