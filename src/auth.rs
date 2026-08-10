//! How the application gets a Spotify session and a Web API token.
//!
//! There are two logins, and each one needs its own browser visit.
//!
//! * The **session** streams audio and serves what the Web API refuses. librespot writes reusable
//!   credentials to the cache while it connects, so every run after the first connects from disk.
//! * The **Web API** reads and edits the library. It needs a token of its own.
//!
//! The two cannot share one visit, because each half insists on a different client identifier.
//!
//! * The session's reusable credential is only worth anything if **Spotify's own** desktop
//!   identifier minted it. Every request the session makes ends at `login5`, which upgrades that
//!   credential into an access token, and `login5` refuses a credential that a registered
//!   application minted: `INVALID_CREDENTIALS` when it is presented under Spotify's identifier,
//!   and `BAD_REQUEST` under the application's own, which `login5` does not accept at all. The
//!   access point is not so strict, so a session built the wrong way authenticates and then fails
//!   every request it makes — see [`SESSION_CLIENT_ID`].
//! * The Web API wants the opposite. Spotify's identifier is shared by every librespot program in
//!   the world and its quota is spent, so it answers `429` to almost every request. Only an
//!   identifier this person registered reads a library.
//!
//! So the first run opens the browser twice: once for the library, once for playback. Both are
//! kept afterwards — a refresh token for the Web API, reusable credentials for the session — and
//! no later run opens a browser at all.
//!
//! With no registered identifier there is nothing to ask for the library with, so that build opens
//! the browser once and takes its Web API token from the session, rate limit and all.

use std::path::PathBuf;

use librespot_core::cache::Cache;
use librespot_core::config::SessionConfig;
use librespot_core::session::Session;
use librespot_core::{Error as SpotifyError, authentication::Credentials};
use librespot_oauth::{OAuthClient, OAuthClientBuilder, OAuthError, OAuthToken};
use serde::{Deserialize, Serialize};

use crate::session::SessionCell;

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

/// The identifier that mints the session's reusable credentials.
///
/// This is Spotify's own desktop identifier, which is what [`ClientId::Shared`] carries. It has to
/// be this one: `login5` upgrades the session's stored credential on every request the session
/// makes, and it only accepts a credential one of Spotify's own identifiers minted. A registered
/// application's identifier is refused outright.
///
/// librespot already sends this identifier for the session's own requests — `SessionConfig` is
/// left at its default, so `login5` and the client token both carry it. This names the one place
/// the application has a say in: which identifier the browser mints the credential under.
const SESSION_CLIENT_ID: ClientId = ClientId::Shared;

/// What the session login asks permission for.
///
/// Streaming is the whole of it. Everything else the session serves it serves on the strength of
/// the account, not a scope.
const SESSION_SCOPES: &[&str] = &["streaming"];

/// What the application asks permission for.
pub const SCOPES: &[&str] = &[
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
    /// The session died, and the disk holds nothing to build another one with.
    ///
    /// librespot connects a session one time, so the application replaces a session that died —
    /// see [`relink`]. The replacement is a login with the credentials that librespot wrote. A
    /// login that wrote none of them leaves the browser as the only way back.
    #[error("the connection to Spotify was lost; sign in again")]
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
    /// Waiting for the person to finish the library login in the browser.
    WaitingForBrowser,
    /// Waiting for the person to finish the playback login in the browser.
    ///
    /// The second visit of a first run. It is a separate phase because the screen has to say why
    /// the browser opened again, which a repeat of the first line would not.
    WaitingForPlayback,
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
    ///
    /// A cell holds it. The application replaces a session that dies, and all the code that reads
    /// the session must see the replacement.
    pub session: SessionCell,
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
    /// Held while the application builds a replacement session.
    ///
    /// The watchdog and the button in the header both ask for one. Two replacements make two
    /// connections to Spotify, and the connection that nothing points at stays open.
    replacing: tokio::sync::Mutex<()>,
    /// What the session's own tasks run on.
    ///
    /// The application can replace a session from any thread, and librespot builds one only inside
    /// a runtime. This handle gives the runtime to the caller.
    runtime: tokio::runtime::Handle,
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

    /// Whether the session connected one time and has since died.
    ///
    /// A session that never connected is still good. The first login owns that one.
    #[must_use]
    pub fn is_dead(&self) -> bool {
        self.linked.load(std::sync::atomic::Ordering::Relaxed) && self.session.is_invalid()
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
        session: SessionCell::new(Session::new(
            SessionConfig::default(),
            Some(credentials.clone()),
        )),
        client_id,
        refresh_token,
        credentials,
        linked: std::sync::atomic::AtomicBool::new(false),
        replacing: tokio::sync::Mutex::new(()),
        runtime: tokio::runtime::Handle::current(),
    }
}

/// Builds a session in the place of one that died, and connects it.
///
/// librespot cannot connect a session again. The connection lives in a cell that librespot sets
/// one time, and the invalid mark stays for the life of the session. The recovery is therefore a
/// new session. It connects with the credentials on the disk, so it needs no browser: librespot
/// wrote reusable credentials there at the first connection.
///
/// Returns nothing when the session is already well. A second caller finds that state after the
/// first caller does the work.
pub async fn relink(standby: &Standby) -> Result<Option<Session>, AuthError> {
    let _building = standby.replacing.lock().await;
    if !standby.session.is_invalid() {
        return Ok(None);
    }

    // A replacement is only worth building from credentials `login5` will still upgrade. Ones
    // minted the other way connect and then refuse every request, which would turn a dead session
    // into a live one that does nothing.
    if !session_credentials_usable(&standby.credentials) {
        return Err(AuthError::Stale);
    }
    let Some(stored) = standby.credentials.credentials() else {
        // Only a login that wrote no credentials arrives here, and that login needs a browser.
        return Err(AuthError::Stale);
    };

    // librespot builds a session inside a runtime, and it spawns tasks while it connects. Do
    // both on the runtime, whatever thread the caller runs on.
    let credentials = standby.credentials.clone();
    let session = standby
        .runtime
        .spawn(async move {
            let session = Session::new(SessionConfig::default(), Some(credentials));
            session.connect(stored, true).await.map(|()| session)
        })
        .await??;

    standby.session.set(session.clone());
    tracing::info!(username = %session.username(), "the streaming session was replaced");
    Ok(Some(session))
}

/// Connects the session in `standby`.
///
/// Uses the credentials on disk when there are any. Opens a browser for each half that has nothing
/// on disk, and for the session when the credentials on disk are refused. Returns the Web API
/// token, when this run minted one; a run that read its refresh token off the disk mints none.
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

    // The library half. Its refresh token outlives every run, so the browser opens for it only
    // once. A build with no registered identifier has nothing to ask for and takes its token from
    // the session instead.
    let mut web_token = None;
    if client_id.reaches_web_api() && standby.refresh_token.is_none() {
        phase(AuthPhase::WaitingForBrowser);
        let token = login_in_browser(client_id.clone(), SCOPES).await?;
        if !token.refresh_token.is_empty() {
            save_refresh(client_id, &token.refresh_token);
        }
        web_token = Some(token);
    }

    // The playback half. The credentials on disk carry it whenever Spotify's own identifier minted
    // them; anything else has to be minted again, however good it looks.
    if session_credentials_usable(&standby.credentials)
        && let Some(stored) = standby.credentials.credentials()
    {
        phase(AuthPhase::Connecting);
        match standby.session.get().connect(stored, true).await {
            Ok(()) => {
                standby.linked.store(true, Ordering::Relaxed);
                phase(AuthPhase::Ready);
                return Ok(web_token);
            }
            Err(error) => tracing::warn!(%error, "stored credentials refused, opening a browser"),
        }
    }

    phase(AuthPhase::WaitingForPlayback);
    // Streaming is all the session needs. A build with no registered identifier reads the library
    // through this token as well, so that one has to ask for the library scopes too.
    let session_scopes = if client_id.reaches_web_api() {
        SESSION_SCOPES
    } else {
        SCOPES
    };
    let session_token = login_in_browser(SESSION_CLIENT_ID, session_scopes).await?;

    phase(AuthPhase::Connecting);
    // Store the credentials while connecting. Every later run reads them back.
    standby
        .session
        .get()
        .connect(
            Credentials::with_access_token(&session_token.access_token),
            true,
        )
        .await?;
    note_minted_by(&SESSION_CLIENT_ID);
    standby.linked.store(true, Ordering::Relaxed);
    phase(AuthPhase::Ready);

    // A build with no registered identifier reads the library through the session, and the token
    // that connected it is the one to start from.
    Ok(web_token.or((!client_id.reaches_web_api()).then_some(session_token)))
}

/// What a completed login hands to the rest of the application.
pub struct Login {
    /// The session that streams audio.
    pub session: SessionCell,
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

/// Where the identifier that minted the session's credentials is noted.
///
/// This sits beside the credentials it describes, inside the directory [`forget`] removes, so the
/// note cannot outlive what it refers to.
fn minted_by_path() -> Result<PathBuf, AuthError> {
    Ok(crate::config::cache_dir()?.join("credentials").join("minted-by"))
}

/// Whether the credentials on disk are ones the session can still use.
///
/// A credential minted under any other identifier authenticates at the access point and is then
/// refused by `login5` on the first request, which reads as a dead account rather than a wrong
/// login. Reading the note is how that is caught before the browser is skipped.
fn session_credentials_usable(cache: &Cache) -> bool {
    if cache.credentials().is_none() {
        return false;
    }
    let noted = minted_by_path()
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok());
    minted_the_session_way(noted.as_deref())
}

/// Whether a note names the identifier the session's credentials have to come from.
///
/// No note means no: a build that kept none predates the split, so its credentials were minted
/// under the registered identifier and `login5` will refuse them.
fn minted_the_session_way(noted: Option<&str>) -> bool {
    noted.is_some_and(|noted| noted.trim() == SESSION_CLIENT_ID.as_str())
}

/// Notes which identifier minted the credentials librespot has just written.
fn note_minted_by(client_id: &ClientId) {
    let result = minted_by_path().and_then(|path| {
        std::fs::write(path, client_id.as_str()).map_err(|error| AuthError::Cache(error.into()))
    });
    if let Err(error) = result {
        tracing::warn!(%error, "cannot note which identifier minted the credentials");
    }
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
/// Removes the credentials librespot kept, the note of what minted them, and the refresh token
/// beside them. The next start finds nothing and asks the browser again — twice, once per half.
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
/// Both halves have to be there. A session reconnects from the credentials librespot kept — and
/// only from ones Spotify's own identifier minted — while the Web API needs a refresh token beside
/// them, so one without the other still means the browser.
#[must_use]
pub fn is_signed_in() -> bool {
    let client_id = ClientId::resolve();
    let Ok(cache) = open_cache() else {
        return false;
    };
    session_credentials_usable(&cache)
        && (!client_id.reaches_web_api() || load_refresh(&client_id).is_some())
}

/// Builds the browser-login client for `client_id`.
///
/// Both halves come back to the same address. They run one after the other, so the listener the
/// first one puts on that port is gone before the second one asks for it.
pub fn oauth_client(client_id: &ClientId, scopes: &[&str]) -> Result<OAuthClient, OAuthError> {
    OAuthClientBuilder::new(&client_id.as_str(), REDIRECT_URI, scopes.to_vec())
        .open_in_browser()
        .with_custom_message(DONE_PAGE)
        .build()
}

/// Runs the browser flow.
///
/// The flow blocks a thread: it opens a browser and waits on a loopback socket for the redirect.
/// Run it away from the runtime's worker threads.
async fn login_in_browser(
    client_id: ClientId,
    scopes: &'static [&'static str],
) -> Result<OAuthToken, AuthError> {
    Ok(tokio::task::spawn_blocking(move || {
        oauth_client(&client_id, scopes)?.get_access_token()
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
    async fn a_session_that_died_is_replaced_rather_than_connected_again() {
        let directory = std::env::temp_dir().join("canora-auth-test-stale");
        let standby = standby_in(&directory);
        standby
            .linked
            .store(true, std::sync::atomic::Ordering::Relaxed);
        standby.session.get().shutdown();

        assert!(standby.is_dead(), "a connected session that shut down is dead");

        let failure = link(&standby, |_| {}).await;
        assert!(
            matches!(failure, Err(AuthError::Stale)),
            "a dropped session cannot be connected again in place: {failure:?}"
        );
    }

    #[tokio::test]
    async fn a_session_that_never_connected_is_not_dead() {
        // The watchdog reads this. A window on the welcome screen holds a session that did no
        // work yet, and a replacement for that session races the login.
        let directory = std::env::temp_dir().join("canora-auth-test-fresh");
        let standby = standby_in(&directory);

        assert!(!standby.is_dead());
        standby.session.get().shutdown();
        assert!(!standby.is_dead(), "nothing was connected, so nothing died");
    }

    #[tokio::test]
    async fn a_replacement_needs_credentials_on_disk() {
        let directory = std::env::temp_dir().join("canora-auth-test-relink");
        let _ = std::fs::remove_dir_all(&directory);
        let standby = standby_in(&directory);
        standby.session.get().shutdown();

        assert!(
            matches!(relink(&standby).await, Err(AuthError::Stale)),
            "there is nothing to connect a new session with"
        );
    }

    #[test]
    fn credentials_from_before_the_split_are_not_reused() {
        // The failure this catches is silent: a credential the registered identifier minted
        // authenticates at the access point and is then refused by `login5` on every request, so
        // the application looks signed in and serves nothing.
        assert!(
            !minted_the_session_way(None),
            "a cache with no note predates the split"
        );
        assert!(
            !minted_the_session_way(Some(COMPILED_CLIENT_ID)),
            "the registered identifier cannot mint the session's credentials"
        );
        assert!(
            minted_the_session_way(Some(&SESSION_CLIENT_ID.as_str())),
            "Spotify's own identifier is the one that works"
        );
        assert!(
            minted_the_session_way(Some(&format!("{}\n", SESSION_CLIENT_ID.as_str()))),
            "a trailing newline is still the same identifier"
        );
    }

    #[tokio::test]
    async fn a_healthy_session_is_left_alone() {
        let directory = std::env::temp_dir().join("canora-auth-test-healthy");
        let standby = standby_in(&directory);

        let replaced = relink(&standby)
            .await
            .expect("a session that is not invalid needs no replacement");
        assert!(replaced.is_none(), "no second connection was made");
    }
}
