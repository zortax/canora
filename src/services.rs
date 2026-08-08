//! What the interface talks to.
//!
//! One value holds the Web API, the player, the radio and the picture cache. The interface takes
//! it from context and clones it into whatever needs it.

use std::sync::Arc;

use crate::api::radio::Radio;
use crate::api::{ApiClient, ApiError};
use crate::cache::Cache;
use crate::images::ImageCache;
use crate::models::UserProfile;
use crate::player::PlayerHandle;

/// What stopped the application from starting.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// The login failed.
    #[error(transparent)]
    Auth(#[from] crate::auth::AuthError),
    /// The account could not be read.
    #[error(transparent)]
    Api(#[from] ApiError),
    /// The audio pipeline could not start.
    #[error(transparent)]
    Player(#[from] crate::player::PlayerError),
    /// The library cache could not be opened.
    #[error(transparent)]
    Cache(#[from] crate::cache::CacheError),
    /// The cache directory is unusable.
    #[error(transparent)]
    Config(#[from] crate::config::ConfigError),
}

/// Everything the interface reaches the outside world through.
#[derive(Clone)]
pub struct Services {
    /// Reads and edits the account.
    pub api: Arc<ApiClient>,
    /// Builds a station around a track.
    pub radio: Arc<Radio>,
    /// Plays audio.
    pub player: PlayerHandle,
    /// Holds album art.
    pub images: Arc<ImageCache>,
    /// The library, as it was last read.
    pub cache: Arc<Cache>,
    /// The streaming session, for what the Web API does not carry.
    pub session: crate::session::SessionCell,
    /// What a login needs, for connecting behind the window.
    pub standby: Arc<crate::auth::Standby>,
}

impl Services {
    /// Connects the session and reads the account.
    ///
    /// The window is already drawn by the time this runs. A failure is reported rather than fatal:
    /// the cached library stays on the screen and the header says the account is out of reach.
    pub async fn link(
        &self,
        mut phase: impl FnMut(crate::auth::AuthPhase),
    ) -> Result<UserProfile, StartError> {
        // librespot connects a session one time, so a session that died needs a replacement. The
        // button in the header therefore does the work that the watchdog does.
        if self.standby.is_dead() {
            phase(crate::auth::AuthPhase::Connecting);
            self.reconnect().await?;
            phase(crate::auth::AuthPhase::Ready);
        } else {
            let token = crate::auth::link(&self.standby, phase).await?;
            if let Some(token) = &token {
                // A browser login mints a Web API token directly. Use it rather than spending the
                // refresh token on a second one.
                self.api.prime(token).await;
            }
        }
        let me = self.api.me().await?.clone();
        self.cache.put_profile(&me).await;
        Ok(me)
    }

    /// Puts a fresh session in the place of one that died, and hands it to the player.
    ///
    /// All the code that reads the session reads it through the cell, so the Web API and the
    /// playlists Spotify makes need no more work. librespot keeps a private copy of the session in
    /// the audio pipeline, so the player gets the new session in a command.
    pub async fn reconnect(&self) -> Result<(), StartError> {
        let Some(session) = crate::auth::relink(&self.standby).await? else {
            return Ok(());
        };
        self.player.session_replaced(session);
        Ok(())
    }

    /// How the account arranges its playlists.
    pub async fn rootlist(&self) -> Vec<crate::api::direct::Entry> {
        crate::api::direct::tree(&self.session.get()).await
    }
}

/// Builds everything the interface uses. Touches no network.
///
/// The session is built and left unconnected, and the cached library is opened. The window can
/// therefore draw a full page before anything has been asked of Spotify. Call [`Services::link`]
/// afterwards to connect.
///
/// The Web API needs no session at all when a refresh token is on disk: that token mints access
/// tokens over plain HTTP. Only audio waits for the session.
///
/// Playback events go out on `events`. The interface drains that channel on its own thread, which
/// is where every signal is written.
pub async fn start(
    runtime: tokio::runtime::Handle,
    events: tokio::sync::mpsc::UnboundedSender<crate::player::PlaybackEvent>,
) -> Result<Services, StartError> {
    let credentials = crate::auth::open_cache()?;
    let standby = Arc::new(crate::auth::standby(credentials.clone()));

    if !standby.client_id.reaches_web_api() {
        tracing::warn!(
            "no client identifier: audio plays, and Spotify refuses most library requests. \
             Put SPOTIFY_CLIENT_ID in .env and build again."
        );
    }

    let api = Arc::new(ApiClient::new(&standby.as_login()).await);
    let radio = Arc::new(Radio::new(standby.session.clone()));

    // The engine reports once, and both readers hear it: the interface draws it, and the desktop
    // shows it. The desktop's copy is set up after the player, because it takes the handle.
    let (desktop_tx, desktop_rx) = tokio::sync::mpsc::unbounded_channel();
    // The watchdog reports an outage on the same channel, so it keeps a sender of its own.
    let outages = events.clone();
    let player = crate::player::spawn(standby.session.clone(), credentials, &runtime, move |event| {
        // A closed channel means the window is gone. The engine stops with it.
        let _ = desktop_tx.send(event.clone());
        let _ = events.send(event);
    })?;
    crate::desktop::relay(&runtime, player.clone(), desktop_rx);

    // A picture that never arrives must not hold the ask open: every later reader of that address
    // waits on the same fetch, so one stalled connection is one cover that never appears again.
    let pictures = reqwest::Client::builder()
        .user_agent(concat!("canora/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .unwrap_or_default();
    let images = ImageCache::new(pictures, crate::config::cache_dir()?.join("images"));

    // A cache that cannot be opened is reported, and the interface reads Spotify directly.
    let cache = Arc::new(Cache::open().await?);
    // The client reads tracks from here before it asks Spotify for them.
    api.with_cache(cache.clone());

    let services = Services {
        api,
        radio,
        player,
        images,
        cache,
        session: standby.session.clone(),
        standby,
    };

    // A connection that dies stops the audio pipeline and reports nothing. Watch the connection.
    crate::session::supervise(&runtime, services.clone(), outages);

    Ok(services)
}
