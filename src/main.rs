//! A fast, compact Spotify client.

mod api;
mod auth;
mod cache;
mod config;
mod images;
mod models;
mod mpris;
mod player;
mod session;
mod services;
mod ui;

use std::time::Duration;

use anyhow::Context as _;
use zgui::prelude::*;
use zgui::view;

use crate::ui::RootProps;

/// Opens the window.
///
/// Run `canora check` to test the login and the Web API without opening a window.
fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("canora=info,warn")),
        )
        .init();

    // Route background work to tokio. Hold the guard for the life of the program.
    let tokio = zgui::tokio::install()?;

    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("check") => return tokio.handle().block_on(check()),
        // `canora raw /search?q=x&type=track` reports what the Web API answers.
        Some("raw") => {
            let first = args.next().unwrap_or_else(|| "/me".to_owned());
            // `canora raw [METHOD] <path>`; the method defaults to GET.
            let (method, path) = match first.as_str() {
                "GET" | "PUT" | "POST" | "DELETE" => {
                    (first, args.next().unwrap_or_else(|| "/me".to_owned()))
                }
                _ => ("GET".to_owned(), first),
            };
            return tokio.handle().block_on(raw(&method, &path));
        }
        // `canora play` plays the top of the saved tracks and reports what the engine does.
        Some("play") => return tokio.handle().block_on(play(tokio.handle().clone())),
        // `canora rootlist` reports the playlist tree as the session sees it.
        Some("rootlist") => return tokio.handle().block_on(rootlist()),
        // `canora list <playlist-id>` reports one playlist as the session describes it.
        Some("list") => {
            let id = args
                .next()
                .unwrap_or_else(|| "37i9dQZF1DXcBWIGoYBM5M".to_owned());
            return tokio.handle().block_on(listing(&id));
        }
        // `canora cover <url>` fetches one picture, with no window in the way.
        Some("cover") => {
            let url = args.next().unwrap_or_else(|| {
                "https://i.scdn.co/image/ab67616d00001e0208181b9f840a06e7a071cf72".to_owned()
            });
            return tokio.handle().block_on(cover(&url));
        }
        // `canora sp <endpoint>` reports what one session endpoint answers.
        Some("sp") => {
            let path = args.next().unwrap_or_else(|| "/playlist/v2/rootlist".to_owned());
            return tokio.handle().block_on(spclient(&path));
        }
        // `canora station <track-id>` reports what the session's radio endpoints answer.
        Some("station") => {
            let id = args
                .next()
                .unwrap_or_else(|| "0VjIjW4GlUZAMYd2vXMi3b".to_owned());
            return tokio.handle().block_on(station(&id));
        }
        _ => {}
    }

    // The window opens first and signs in behind its own welcome screen. A run with a login on
    // disk goes straight through; a first run has something to look at while the browser is out.
    let runtime = tokio.handle().clone();

    app()
        .with_application_id("dev.canora.Canora")
        .with_title("canora")
        .with_size(1100.0, 720.0)
        .with_min_size(720.0, 480.0)
        // The window draws its own frame; the strip across the top is the handle.
        .with_decorations(false)
        .with_stylesheet(crate::ui::style::SHEET)
        .run(move || {
            view! { Root(runtime = runtime.clone()) }
        })?;

    Ok(())
}

/// Reports what the Web API answers for one path.
///
/// This shows the reply before this application reads it, which is what a wrong query parameter
/// or an unexpected shape needs.
async fn raw(method: &str, path: &str) -> anyhow::Result<()> {
    let cache = crate::auth::open_cache().context("cannot open the cache")?;
    let login = crate::auth::connect(cache, |_| {})
        .await
        .context("cannot connect")?;
    let api = crate::api::ApiClient::new(&login).await;

    match api.raw(method, path).await {
        Ok(text) => tracing::info!("{text}"),
        Err(error) => tracing::warn!(%error, %path, "the Web API refused"),
    }
    Ok(())
}

/// Plays saved tracks and reports what the engine does.
///
/// This tests the audio pipeline: it loads a track, plays it, seeks, changes the volume, and
/// moves to the next track.
async fn play(runtime: tokio::runtime::Handle) -> anyhow::Result<()> {
    use crate::player::{PlayContext, PlayerCommand};

    let cache = crate::auth::open_cache().context("cannot open the cache")?;
    let login = crate::auth::connect(cache.clone(), |_| {})
        .await
        .context("cannot connect")?;
    let api = crate::api::ApiClient::new(&login).await;

    let saved = api.saved_tracks(0).await.context("cannot read saved tracks")?;
    let tracks: Vec<_> = saved.items.into_iter().take(3).collect();
    anyhow::ensure!(!tracks.is_empty(), "there are no saved tracks to play");
    for track in &tracks {
        tracing::info!(name = %track.name, by = %track.artist_line(), "queued");
    }

    let player = crate::player::spawn(login.session.clone(), cache, &runtime, |event| {
        // A position report arrives twice a second. Say it short, and the rest in full.
        match event {
            crate::player::PlaybackEvent::Moved { position } => {
                tracing::info!(seconds = position.as_secs_f64(), "position");
            }
            crate::player::PlaybackEvent::TrackStarted { track, index, .. } => {
                tracing::info!(index, name = %track.name, by = %track.artist_line(), "track");
            }
            crate::player::PlaybackEvent::QueueChanged { upcoming } => {
                tracing::info!(to_come = upcoming.len(), "queue");
            }
            other => tracing::info!(event = ?other),
        }
    })
    .context("cannot start the player")?;

    player.play_tracks(tracks, 0, PlayContext::Liked);
    tokio::time::sleep(Duration::from_secs(6)).await;

    tracing::info!("seek to 60s");
    player.seek(Duration::from_secs(60));
    tokio::time::sleep(Duration::from_secs(4)).await;

    tracing::info!("volume to 30%");
    player.set_volume(0.3);
    tokio::time::sleep(Duration::from_secs(2)).await;

    tracing::info!("next track");
    player.next();
    tokio::time::sleep(Duration::from_secs(6)).await;

    tracing::info!("pause");
    player.toggle_play();
    tokio::time::sleep(Duration::from_secs(2)).await;

    player.send(PlayerCommand::Shutdown);
    tokio::time::sleep(Duration::from_millis(300)).await;
    Ok(())
}

/// Reports the playlist tree as the session sees it.
async fn rootlist() -> anyhow::Result<()> {
    let cache = crate::auth::open_cache().context("cannot open the cache")?;
    let login = crate::auth::connect(cache, |_| {})
        .await
        .context("cannot connect")?;

    for entry in crate::api::direct::tree(&login.session.get()).await {
        match entry {
            crate::api::direct::Entry::Playlist(id) => tracing::info!(playlist = %id.0),
            crate::api::direct::Entry::Folder {
                name, playlists, ..
            } => {
                tracing::info!(folder = %name, holds = playlists.len());
                for id in playlists {
                    tracing::info!(folder = %name, playlist = %id.0);
                }
            }
        }
    }
    Ok(())
}

/// Reports one playlist as the session describes it.
async fn listing(id: &str) -> anyhow::Result<()> {
    let cache = crate::auth::open_cache().context("cannot open the cache")?;
    let login = crate::auth::connect(cache, |_| {})
        .await
        .context("cannot connect")?;

    let id = crate::models::PlaylistId(id.to_owned());
    match crate::api::direct::listing(&login.session.get(), &id).await {
        Some(listing) => {
            tracing::info!(
                name = %listing.name,
                about = %listing.description,
                tracks = listing.tracks.len(),
                "playlist"
            );
            for image in &listing.images {
                tracing::info!(cover = %image.url);
            }
            for track in listing.tracks.iter().take(5) {
                tracing::info!(track = %track.0);
            }
        }
        None => tracing::warn!("the session will not serve this playlist"),
    }
    Ok(())
}

/// Fetches one picture and reports what came back.
///
/// The window fetches pictures on its own thread. This one runs on the runtime with nothing else
/// in the way, which is what tells a stalled picture apart from a stalled interface.
async fn cover(url: &str) -> anyhow::Result<()> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("canora/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(20))
        .build()?;

    let started = std::time::Instant::now();
    match client.get(url).send().await {
        Ok(response) => {
            let status = response.status();
            let bytes = response.bytes().await.map(|it| it.len()).unwrap_or_default();
            tracing::info!(%status, bytes, took = ?started.elapsed(), %url, "picture");
        }
        Err(error) => tracing::warn!(%error, took = ?started.elapsed(), %url, "cannot fetch"),
    }
    Ok(())
}

/// Reports what one session endpoint answers.
///
/// The session speaks to the same hosts the desktop client uses, which carry endpoints the Web
/// API has no equivalent for. This asks one of them directly.
async fn spclient(path: &str) -> anyhow::Result<()> {
    let cache = crate::auth::open_cache().context("cannot open the cache")?;
    let login = crate::auth::connect(cache, |_| {})
        .await
        .context("cannot connect")?;

    match login
        .session
        .get()
        .spclient()
        .request(&http::Method::GET, path, None, None)
        .await
    {
        Ok(bytes) => {
            tracing::info!(%path, bytes = bytes.len(), "the session answered");
            tracing::info!("{:.4000}", String::from_utf8_lossy(&bytes));
        }
        Err(error) => tracing::warn!(%error, %path, "the session refused"),
    }
    Ok(())
}

/// Reports what the session's radio endpoints answer for one track.
async fn station(id: &str) -> anyhow::Result<()> {
    use librespot_core::spotify_uri::SpotifyUri;

    let cache = crate::auth::open_cache().context("cannot open the cache")?;
    let login = crate::auth::connect(cache, |_| {})
        .await
        .context("cannot connect")?;
    let uri = SpotifyUri::from_uri(&format!("spotify:track:{id}"))
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    let session = login.session.get();
    let client = session.spclient();

    match client.get_radio_for_track(&uri).await {
        Ok(bytes) => tracing::info!(
            bytes = bytes.len(),
            reply = %format_args!("{:.1200}", String::from_utf8_lossy(&bytes)),
            "inspiredby-mix"
        ),
        Err(error) => tracing::warn!(%error, "inspiredby-mix refused"),
    }

    let context_uri = format!("spotify:track:{id}");
    match client
        .get_apollo_station("stations", &context_uri, Some(20), Vec::new(), true)
        .await
    {
        Ok(bytes) => tracing::info!(
            bytes = bytes.len(),
            reply = %format_args!("{:.1200}", String::from_utf8_lossy(&bytes)),
            "radio-apollo"
        ),
        Err(error) => tracing::warn!(%error, "radio-apollo refused"),
    }

    Ok(())
}

/// Connects, reads the account, and reports what came back.
///
/// This tests the parts that need a real account: the browser login, the credentials on disk, the
/// token from the session, and the Web API.
async fn check() -> anyhow::Result<()> {
    let client_id = crate::auth::ClientId::resolve();
    tracing::info!(
        own = client_id.reaches_web_api(),
        "client identifier; the shared one is refused most requests"
    );

    let cache = crate::auth::open_cache().context("cannot open the cache")?;
    tracing::info!(on_disk = cache.credentials().is_some(), "credentials");

    let login = crate::auth::connect(cache, |phase| tracing::info!(?phase))
        .await
        .context("cannot connect")?;
    tracing::info!(username = %login.session.get().username(), "connected");

    let api = crate::api::ApiClient::new(&login).await;
    let me = api.me().await.context("cannot read the account")?;
    tracing::info!(
        name = %me.display_name,
        id = %me.id,
        plan = %me.product,
        "account"
    );

    let playlists = api
        .my_playlists()
        .await
        .context("cannot read the playlists")?;
    tracing::info!(count = playlists.len(), "playlists");
    for playlist in playlists.iter().take(5) {
        tracing::info!(
            name = %playlist.name,
            tracks = playlist.total_tracks,
            editable = playlist.kind.is_editable(),
            "playlist"
        );
    }

    let saved = api.saved_tracks(0).await.context("cannot read saved tracks")?;
    tracing::info!(count = saved.total, "liked songs");
    if let Some(track) = saved.items.first() {
        tracing::info!(name = %track.name, by = %track.artist_line(), "first saved track");

        // Song radio prefers the Web API and falls back to the session.
        let radio = crate::api::radio::Radio::new(login.session.clone());
        match radio.for_track(&api, &track.id).await {
            Ok(tracks) => tracing::info!(tracks = tracks.len(), "radio"),
            Err(error) => tracing::warn!(%error, "radio is unavailable"),
        }
    }

    // The playlist a person owns is the one the interface may edit. Read its tracks.
    if let Some(mine) = playlists.iter().find(|it| it.kind.is_editable())
        && let Some(id) = &mine.id
    {
        match api.playlist_tracks(id, 0).await {
            Ok(page) => {
                tracing::info!(name = %mine.name, tracks = page.total, "own playlist");
                for track in page.items.iter().take(3) {
                    tracing::info!(name = %track.name, by = %track.artist_line(), "track");
                }
                // Like state drives the heart on every row.
                let ids: Vec<_> = page.items.iter().take(5).map(|it| it.id.clone()).collect();
                match api.saved_contains(&ids).await {
                    Ok(saved) => tracing::info!(?saved, "which are saved"),
                    Err(error) => tracing::warn!(%error, "cannot read which are saved"),
                }
            }
            Err(error) => tracing::warn!(%error, name = %mine.name, "cannot read the playlist"),
        }
    }

    let results = api.search("radiohead").await.context("cannot search")?;
    tracing::info!(
        tracks = results.tracks.len(),
        artists = results.artists.len(),
        albums = results.albums.len(),
        playlists = results.playlists.len(),
        "search"
    );

    Ok(())
}
