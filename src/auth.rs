//! How the application gets a Spotify session and a Web API token.
//!
//! There are two logins behind one browser visit.
//!
//! * The **session** streams audio. librespot writes reusable credentials to the cache while it
//!   connects, so every run after the first connects from disk.
//! * The **Web API** reads and edits the library. It needs a token of its own.
//!
//! Which client identifier is used decides where the Web API token comes from.
//!
//! * With an identifier of this person's own, the browser login mints the Web API token directly
//!   and a refresh token keeps it alive. This is the supported path.
//! * With no identifier, the login falls back to Spotify's own desktop identifier. Audio plays,
//!   but the Web API answers `429` to almost every request: that identifier is shared by every
//!   librespot program in the world and its quota is spent. Set an identifier to read a library.

use std::path::PathBuf;

use librespot_core::cache::Cache;
use librespot_core::config::SessionConfig;
use librespot_core::session::Session;
use librespot_core::{Error as SpotifyError, authentication::Credentials};
use librespot_oauth::{OAuthClient, OAuthClientBuilder, OAuthError, OAuthToken};
use serde::{Deserialize, Serialize};

/// Where the browser sends the code back to.
///
/// Spotify checks this address against the client identifier on every request. Register this exact
/// text in the dashboard, or the login fails.
pub const REDIRECT_URI: &str = "http://127.0.0.1:5588/login";

/// What the browser shows when it comes back.
///
/// One page answers both endings. The listener replies with the same bytes whatever the redirect
/// carried, so the page reads its own address: a `code` means the login worked, and an `error`
/// means it did not.
const DONE_PAGE: &str = include_str!("../assets/login.html");

/// How much audio the cache holds before it drops the oldest files.
const AUDIO_CACHE_BYTES: u64 = 1024 * 1024 * 1024;

/// The client identifier compiled in by `build.rs`, from `.env`.
///
/// Empty when the build found none. The identifier is public: the login uses PKCE and carries no
/// secret, so nothing is given away by compiling it in.
const COMPILED_CLIENT_ID: &str = env!("CANORA_CLIENT_ID");

/// The environment variable that overrides the compiled identifier.
pub const CLIENT_ID_VAR: &str = "SPOTIFY_CLIENT_ID";

/// What the application asks permission for.
const SCOPES: &[&str] = &[
    "streaming",
    "user-read-email",
    "user-read-private",
    "playlist-read-private",
    "playlist-read-collaborative",
    "playlist-modify-public",
    "playlist-modify-private",
    "user-library-read",
    "user-library-modify",
    "user-top-read",
    "user-follow-read",
];

/// What stopped a login.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// The browser flow failed.
    #[error("browser login failed: {0}")]
    OAuth(#[from] OAuthError),
    /// The session refused the credentials, or the network is down.
    #[error("cannot connect to Spotify: {0}")]
    Session(#[from] SpotifyError),
    /// The cache directory is unusable.
    #[error("cache directory: {0}")]
    Cache(#[from] crate::config::ConfigError),
    /// The thread that ran the browser flow ended early.
    #[error("login task ended: {0}")]
    Join(#[from] tokio::task::JoinError),
    /// The session was dropped and cannot be built again in place.
    ///
    /// librespot connects a session once. Everything below the interface holds that one session —
    /// the player's engine, the client that reads the playlists Spotify makes — so a session that
    /// died is a restart rather than a retry.
    #[error("the connection to Spotify was lost; start canora again")]
    Stale,
}

/// How far a login has got.
///
/// The interface shows one line per phase, so a person knows why the window is waiting.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AuthPhase {
    /// Looking for credentials on disk.
    #[default]
    CheckingCache,
    /// Waiting for the person to finish in the browser.
    WaitingForBrowser,
    /// Talking to Spotify.
    Connecting,
    /// Connected.
    Ready,
    /// Stopped, with the reason.
    #[allow(dead_code, reason = "the console reports it; the window opens only after a login")]
    Failed(String),
}

/// Which client identifier the login uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientId {
    /// An identifier this person registered. The Web API works.
    Own(String),
    /// Spotify's desktop identifier. Audio plays and the Web API is rate limited.
    Shared,
}

impl ClientId {
    /// Which identifier this build uses.
    ///
    /// Take the environment first, so a different account can be tested without a rebuild. Fall
    /// back to the one compiled in, and to the shared one when the build found none.
    #[must_use]
    pub fn resolve() -> Self {
        let from_env = std::env::var(CLIENT_ID_VAR).ok();
        let candidate = from_env.as_deref().unwrap_or(COMPILED_CLIENT_ID).trim();
        if candidate.is_empty() {
            Self::Shared
        } else {
            Self::Own(candidate.to_owned())
        }
    }

    /// The identifier as text.
    #[must_use]
    pub fn as_str(&self) -> String {
        match self {
            Self::Own(id) => id.clone(),
            Self::Shared => SessionConfig::default().client_id,
        }
    }

    /// Whether this identifier can reach the Web API without a shared quota.
    #[must_use]
    pub fn reaches_web_api(&self) -> bool {
        matches!(self, Self::Own(_))
    }
}

/// The refresh token kept between runs.
///
/// Only a login with this person's own identifier produces one worth keeping. The shared
/// identifier takes its Web API token from the session instead.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredRefresh {
    /// Which identifier minted it. A different identifier makes it useless.
    client_id: String,
    /// The token that mints new access tokens.
    refresh_token: String,
}

/// A login built from what is on disk, before anything reaches the network.
///
/// `Session::new` costs nothing: it builds the object and connects when it is told to. Everything
/// the application needs can therefore be built first and connected afterwards, which is what lets
/// the window open on the cached library rather than on a waiting screen.
///
/// The Web API needs no session at all when a refresh token is on disk. That token mints access
/// tokens over plain HTTP, so the library reads as soon as the window opens. The session is for
/// audio.
pub struct Standby {
    /// The session, built and not connected.
    pub session: Session,
    /// Which identifier this build uses.
    pub client_id: ClientId,
    /// The refresh token from a previous run, when there is one.
    pub refresh_token: Option<String>,
    /// Where librespot keeps credentials, volume and audio.
    pub credentials: Cache,
    /// Whether the session has connected.
    ///
    /// A session connects once and once only: librespot holds the connection in a cell it can set
    /// a single time, and a second `connect` fails with *Session is not connected*, which says the
    /// opposite of what happened. So the second attempt has to be the one that does not happen.
    linked: std::sync::atomic::AtomicBool,
}

impl Standby {
    /// This, as the rest of the application reads a login.
    #[must_use]
    pub fn as_login(&self) -> Login {
        Login {
            session: self.session.clone(),
            client_id: self.client_id.clone(),
            web_token: None,
            refresh_token: self.refresh_token.clone(),
        }
    }
}

/// Builds the session from what is on disk. Touches no network.
#[must_use]
pub fn standby(credentials: Cache) -> Standby {
    let client_id = ClientId::resolve();
    tracing::info!(
        web_api = client_id.reaches_web_api(),
        "using {} client identifier",
        if client_id.reaches_web_api() {
            "your own"
        } else {
            "the shared"
        }
    );
    let refresh_token = load_refresh(&client_id);
    Standby {
        session: Session::new(SessionConfig::default(), Some(credentials.clone())),
        client_id,
        refresh_token,
        credentials,
        linked: std::sync::atomic::AtomicBool::new(false),
    }
}

/// Connects the session in `standby`.
///
/// Uses the credentials on disk when there are any. Opens a browser when there are none, or when
/// the ones on disk are refused. Returns the Web API token a browser login mints.
pub async fn link<F>(standby: &Standby, mut phase: F) -> Result<Option<OAuthToken>, AuthError>
where
    F: FnMut(AuthPhase),
{
    use std::sync::atomic::Ordering;

    let client_id = &standby.client_id;
    phase(AuthPhase::CheckingCache);

    // A second attempt on a session that already connected is the Web API's business alone: the
    // audio half is up, and connecting it again is what librespot refuses.
    if standby.linked.load(Ordering::Relaxed) {
        if standby.session.is_invalid() {
            return Err(AuthError::Stale);
        }
        phase(AuthPhase::Ready);
        return Ok(None);
    }

    // A session reconnects from disk. The Web API needs a browser only when its refresh token is
    // absent, so both halves have to be in hand before the browser can be skipped.
    let can_skip_browser = standby.credentials.credentials().is_some()
        && (!client_id.reaches_web_api() || standby.refresh_token.is_some());

    if can_skip_browser
        && let Some(stored) = standby.credentials.credentials()
    {
        phase(AuthPhase::Connecting);
        match standby.session.connect(stored, true).await {
            Ok(()) => {
                standby.linked.store(true, Ordering::Relaxed);
                phase(AuthPhase::Ready);
                return Ok(None);
            }
            Err(error) => tracing::warn!(%error, "stored credentials refused, opening a browser"),
        }
    }

    phase(AuthPhase::WaitingForBrowser);
    let token = login_in_browser(client_id.clone()).await?;
    if client_id.reaches_web_api() && !token.refresh_token.is_empty() {
        save_refresh(client_id, &token.refresh_token);
    }

    phase(AuthPhase::Connecting);
    // Store the credentials while connecting. Every later run reads them back.
    standby
        .session
        .connect(Credentials::with_access_token(&token.access_token), true)
        .await?;
    standby.linked.store(true, Ordering::Relaxed);
    phase(AuthPhase::Ready);
    Ok(Some(token))
}

/// What a completed login hands to the rest of the application.
pub struct Login {
    /// The session that streams audio.
    pub session: Session,
    /// Which identifier was used.
    pub client_id: ClientId,
    /// The Web API token from the browser login, when there is one.
    pub web_token: Option<OAuthToken>,
    /// The refresh token from a previous run, when there is one.
    pub refresh_token: Option<String>,
}

/// The cache librespot keeps credentials, volume and audio in.
pub fn open_cache() -> Result<Cache, AuthError> {
    let root = crate::config::cache_dir()?;
    let credentials: PathBuf = root.join("credentials");
    let audio: PathBuf = root.join("audio");
    Ok(Cache::new(
        Some(credentials.clone()),
        Some(credentials),
        Some(audio),
        Some(AUDIO_CACHE_BYTES),
    )?)
}

/// Where the refresh token is kept.
fn refresh_path() -> Result<PathBuf, AuthError> {
    Ok(crate::config::config_dir()?.join("oauth.json"))
}

/// The refresh token on disk, if it belongs to `client_id`.
fn load_refresh(client_id: &ClientId) -> Option<String> {
    let path = refresh_path().ok()?;
    let text = std::fs::read_to_string(path).ok()?;
    let stored: StoredRefresh = serde_json::from_str(&text).ok()?;
    (stored.client_id == client_id.as_str()).then_some(stored.refresh_token)
}

/// Writes the refresh token to disk.
pub fn save_refresh(client_id: &ClientId, refresh_token: &str) {
    let stored = StoredRefresh {
        client_id: client_id.as_str(),
        refresh_token: refresh_token.to_owned(),
    };
    let result = refresh_path().and_then(|path| {
        let text = serde_json::to_string(&stored).unwrap_or_default();
        std::fs::write(path, text).map_err(|error| AuthError::Cache(error.into()))
    });
    if let Err(error) = result {
        tracing::warn!(%error, "cannot keep the refresh token");
    }
}

/// Forgets the login.
///
/// Removes the credentials librespot kept and the refresh token beside them. The next start finds
/// nothing and asks the browser again.
pub fn forget() -> Result<(), AuthError> {
    let credentials = crate::config::cache_dir()?.join("credentials");
    match std::fs::remove_dir_all(&credentials) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(AuthError::Cache(error.into())),
    }
    match std::fs::remove_file(refresh_path()?) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(AuthError::Cache(error.into())),
    }
    Ok(())
}

/// Whether a login is already on disk.
///
/// Both halves have to be there. A session reconnects from the credentials librespot kept, and the
/// Web API needs a refresh token beside them, so one without the other still means the browser.
#[must_use]
pub fn is_signed_in() -> bool {
    let client_id = ClientId::resolve();
    let Ok(cache) = open_cache() else {
        return false;
    };
    cache.credentials().is_some()
        && (!client_id.reaches_web_api() || load_refresh(&client_id).is_some())
}

/// Builds the browser-login client for `client_id`.
pub fn oauth_client(client_id: &ClientId) -> Result<OAuthClient, OAuthError> {
    OAuthClientBuilder::new(&client_id.as_str(), REDIRECT_URI, SCOPES.to_vec())
        .open_in_browser()
        .with_custom_message(DONE_PAGE)
        .build()
}

/// Runs the browser flow.
///
/// The flow blocks a thread: it opens a browser and waits on a loopback socket for the redirect.
/// Run it away from the runtime's worker threads.
async fn login_in_browser(client_id: ClientId) -> Result<OAuthToken, AuthError> {
    Ok(tokio::task::spawn_blocking(move || {
        oauth_client(&client_id)?.get_access_token()
    })
    .await??)
}

/// Connects a session and collects what the Web API needs.
///
/// This is the whole login in one call, for the commands that open no window. The window builds a
/// [`Standby`] and calls [`link`] behind what the cache already holds.
pub async fn connect<F>(credentials: Cache, phase: F) -> Result<Login, AuthError>
where
    F: FnMut(AuthPhase),
{
    let standby = standby(credentials);
    let token = link(&standby, phase).await?;
    Ok(Login {
        refresh_token: token
            .as_ref()
            .map(|it| it.refresh_token.clone())
            .filter(|it| !it.is_empty())
            .or(standby.refresh_token),
        session: standby.session,
        client_id: standby.client_id,
        web_token: token,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A standby whose session was never connected, on a cache of its own.
    fn standby_in(directory: &std::path::Path) -> Standby {
        let credentials = Cache::new(Some(directory), Some(directory), None, None)
            .expect("a cache under a temporary directory opens");
        standby(credentials)
    }

    #[tokio::test]
    async fn a_second_attempt_leaves_a_connected_session_alone() {
        // librespot sets the connection into a cell it can set once, and a second `connect` fails
        // with "Session is not connected" — which reads as though the first one never happened.
        // Every retry after a Web API failure went through that path.
        let directory = std::env::temp_dir().join("canora-auth-test-linked");
        let standby = standby_in(&directory);
        standby
            .linked
            .store(true, std::sync::atomic::Ordering::Relaxed);

        let mut phases = Vec::new();
        let token = link(&standby, |phase| phases.push(phase))
            .await
            .expect("a connected session needs no second connection");

        assert!(token.is_none(), "no browser was opened");
        assert_eq!(phases.last(), Some(&AuthPhase::Ready));
    }

    #[tokio::test]
    async fn a_session_that_died_is_a_restart() {
        let directory = std::env::temp_dir().join("canora-auth-test-stale");
        let standby = standby_in(&directory);
        standby
            .linked
            .store(true, std::sync::atomic::Ordering::Relaxed);
        standby.session.shutdown();

        let failure = link(&standby, |_| {}).await;
        assert!(
            matches!(failure, Err(AuthError::Stale)),
            "a dropped session cannot be connected again in place: {failure:?}"
        );
    }
}
