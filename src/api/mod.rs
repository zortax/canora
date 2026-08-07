//! The Spotify Web API, in the shape the interface reads.

pub mod http;
pub mod radio;
pub mod direct;
pub mod token;
pub mod wire;

use std::sync::OnceLock;
use std::time::Duration;

use serde_json::json;

use crate::api::http::{Http, Method};
use crate::api::token::{TokenManager, TokenSource};
use crate::models::{
    Album, AlbumId, Artist, ArtistId, Page, Playlist, PlaylistId, SearchResults, Track, TrackId,
    UserProfile,
};

/// How many items an endpoint returns at once. Fifty is the most Spotify allows.
const PAGE: u8 = 50;

/// How many identifiers go in one batch request.
const BATCH: usize = 50;

/// How many single-track requests run at once.
///
/// The batch endpoint is closed, so a radio queue is fifty small requests. Keep enough of them in
/// flight to be quick, and few enough to stay under the rate limit.
const TRACK_FETCHES: usize = 4;

/// How many items a browse endpoint returns at once.
///
/// Spotify caps the catalogue endpoints at ten for an application like this one and answers
/// `400 Invalid limit` above it. The endpoints that page through a library still allow fifty.
const BROWSE: u8 = 10;

/// How many results a search asks for, per kind.
const SEARCH_LIMIT: u8 = BROWSE;

/// What came back for one track.
enum Reply {
    /// The track, read.
    Read(Box<Track>),
    /// Spotify will not serve it. Leave it out.
    Gone,
    /// Spotify asked for a pause. Leave it out and say the list is short.
    Later,
}

/// Whether an error means the Web API keeps this from an application like this one.
///
/// Spotify's own playlists answer `404`, and some of them `403`. Both mean the same thing here:
/// ask the session instead.
fn is_hidden(error: &ApiError) -> bool {
    matches!(error, ApiError::NotFound | ApiError::Forbidden { .. })
}

/// What stopped a request.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// The request never reached Spotify.
    #[error("network: {0}")]
    Transport(#[from] reqwest::Error),
    /// Spotify refused the request.
    #[error("Spotify said {status}: {message}")]
    Status {
        /// What it answered with.
        status: u16,
        /// What it said.
        message: String,
    },
    /// Too many requests went out.
    #[error("too many requests")]
    RateLimited {
        /// How long Spotify asks for.
        retry_after: Option<Duration>,
    },
    /// This account or this application may not do that.
    #[error("not allowed{}", if .reason.is_empty() { String::new() } else { format!(": {}", .reason) })]
    Forbidden {
        /// What Spotify said, when it said anything.
        reason: String,
    },
    /// There is no such thing.
    #[error("not found")]
    NotFound,
    /// No token could be taken from the session.
    #[error("cannot get an access token: {0}")]
    Token(#[from] librespot_core::Error),
    /// The token could not be refreshed.
    #[error("cannot refresh the access token: {0}")]
    Auth(String),
    /// The reply was not what this application reads.
    #[error("cannot read the reply: {0}")]
    Decode(#[from] serde_json::Error),
}

impl ApiError {
    /// Whether this is Spotify refusing an edit, rather than a fault.
    ///
    /// A read-only playlist answers this way. The interface says so and carries on.
    #[must_use]
    pub fn is_forbidden(&self) -> bool {
        matches!(self, Self::Forbidden { .. })
    }
}

/// Reads and edits a Spotify account.
pub struct ApiClient {
    http: Http,
    me: OnceLock<UserProfile>,
    /// The streaming session, for the playlists the Web API refuses.
    session: librespot_core::session::Session,
    /// The local copy of the library, when there is one.
    ///
    /// A track read here is kept, and the next list that carries it needs no request. The cache
    /// is opened after this client, so it arrives afterwards.
    store: OnceLock<std::sync::Arc<crate::cache::Cache>>,
}

impl ApiClient {
    /// Reads and edits the account the login belongs to.
    ///
    /// Take the Web API token from this person's own application when the login used one. Fall
    /// back to the session otherwise, and expect Spotify to rate limit it.
    pub async fn new(login: &crate::auth::Login) -> Self {
        let source = match (&login.client_id, &login.refresh_token) {
            (client_id @ crate::auth::ClientId::Own(_), Some(refresh_token)) => TokenSource::App {
                client_id: client_id.clone(),
                refresh_token: refresh_token.clone(),
            },
            _ => TokenSource::Session(login.session.clone()),
        };

        let tokens = TokenManager::new(source);
        if let Some(token) = &login.web_token {
            tokens.prime(token).await;
        }

        let client = reqwest::Client::builder()
            .user_agent(concat!("canora/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default();
        Self {
            http: Http::new(client, tokens),
            me: OnceLock::new(),
            session: login.session.clone(),
            store: OnceLock::new(),
        }
    }

    /// Takes the Web API token a browser login just minted.
    pub async fn prime(&self, token: &librespot_oauth::OAuthToken) {
        self.http.tokens().prime(token).await;
    }

    /// Gives this client the local copy of the library to read tracks from.
    pub fn with_cache(&self, cache: std::sync::Arc<crate::cache::Cache>) {
        let _ = self.store.set(cache);
    }

    /// What the Web API answers for one path, as text.
    ///
    /// This is for looking at a reply by hand. Nothing in the interface uses it.
    pub async fn raw(&self, method: &str, path: &str) -> Result<String, ApiError> {
        let method = match method {
            "PUT" => Method::Put,
            "POST" => Method::Post,
            "DELETE" => Method::Delete,
            _ => Method::Get,
        };
        self.http.text(method, path).await
    }

    /// Who is signed in.
    ///
    /// The answer never changes while the application runs, so it is read once.
    pub async fn me(&self) -> Result<&UserProfile, ApiError> {
        if let Some(me) = self.me.get() {
            return Ok(me);
        }
        let user: wire::PrivateUser = self.http.request(Method::Get, "/me", &[], NO_BODY).await?;
        Ok(self.me.get_or_init(|| user.into()))
    }

    // -- playlists ---------------------------------------------------------------------------

    /// Every playlist this person keeps.
    ///
    /// Read every page. A library of a few hundred playlists is a handful of requests, and the
    /// sidebar shows all of them.
    pub async fn my_playlists(&self) -> Result<Vec<Playlist>, ApiError> {
        let me = self.me().await?.id.clone();
        let mut all = Vec::new();
        let mut offset = 0_u32;

        loop {
            let page: wire::Paged<wire::SimplePlaylist> = self
                .http
                .request(
                    Method::Get,
                    "/me/playlists",
                    &[
                        ("limit", PAGE.to_string()),
                        ("offset", offset.to_string()),
                    ],
                    NO_BODY,
                )
                .await?;
            let page = page.map_into(|item| item.into_playlist(me.as_str()));
            let more = page.has_more();
            offset = page.next_offset();
            all.extend(page.items);
            if !more {
                break;
            }
        }

        Ok(all)
    }

    /// One playlist.
    ///
    /// The playlists Spotify makes answer `404` here. The session serves those, so a refusal
    /// asks it instead.
    pub async fn playlist(&self, id: &PlaylistId) -> Result<Playlist, ApiError> {
        let me = self.me().await?.id.clone();
        let reply: Result<wire::SimplePlaylist, ApiError> = self
            .http
            .request(
                Method::Get,
                &format!("/playlists/{id}"),
                &[("fields", FIELDS_PLAYLIST.to_owned())],
                NO_BODY,
            )
            .await;

        match reply {
            Ok(reply) => reply.into_playlist(me.as_str()).ok_or(ApiError::NotFound),
            Err(error) if is_hidden(&error) => {
                self.made_playlist(id).await.ok_or(ApiError::NotFound)
            }
            Err(error) => Err(error),
        }
    }

    /// One playlist Spotify makes, as the session describes it.
    ///
    /// Returns nothing when the session will not serve it either.
    pub async fn made_playlist(&self, id: &PlaylistId) -> Option<Playlist> {
        let listing = direct::listing(&self.session, id).await?;
        Some(Playlist {
            id: Some(id.clone()),
            name: listing.name,
            kind: crate::models::PlaylistKind::Followed {
                owner_name: "Spotify".to_owned(),
            },
            description: listing.description,
            images: listing.images,
            total_tracks: u32::try_from(listing.tracks.len()).unwrap_or(u32::MAX),
            snapshot_id: None,
        })
    }

    /// One page of the tracks in a playlist.
    ///
    /// The playlists Spotify makes answer `404` here as well. The session gives their track
    /// identifiers, and each track is then read one at a time.
    pub async fn playlist_tracks(
        &self,
        id: &PlaylistId,
        offset: u32,
    ) -> Result<Page<Track>, ApiError> {
        let page: Result<wire::Paged<wire::PlaylistItem>, ApiError> = self
            .http
            .request(
                Method::Get,
                &format!("/playlists/{id}/items"),
                &[
                    ("limit", PAGE.to_string()),
                    ("offset", offset.to_string()),
                    ("additional_types", "track".to_owned()),
                ],
                NO_BODY,
            )
            .await;

        match page {
            Ok(page) => Ok(page.map_into(|entry| entry.item.and_then(wire::FullTrack::into_track))),
            Err(error) if is_hidden(&error) => self.made_playlist_tracks(id, offset).await,
            Err(error) => Err(error),
        }
    }

    /// One page of the tracks in a playlist Spotify makes.
    async fn made_playlist_tracks(
        &self,
        id: &PlaylistId,
        offset: u32,
    ) -> Result<Page<Track>, ApiError> {
        let listing = direct::listing(&self.session, id)
            .await
            .ok_or(ApiError::NotFound)?;
        let total = u32::try_from(listing.tracks.len()).unwrap_or(u32::MAX);

        let from = usize::try_from(offset).unwrap_or(usize::MAX).min(listing.tracks.len());
        let to = (from + usize::from(PAGE)).min(listing.tracks.len());
        let (items, whole) = self.tracks(&listing.tracks[from..to]).await?;

        Ok(Page {
            items,
            total,
            offset,
            whole,
            // The slice, rather than what came back from it. A track the catalogue will not
            // serve still moves the reader along.
            covered: u32::try_from(to - from).unwrap_or(u32::MAX),
        })
    }

    /// Makes a playlist and returns it.
    pub async fn create_playlist(&self, name: &str) -> Result<Playlist, ApiError> {
        let me = self.me().await?.id.clone();
        let reply: wire::SimplePlaylist = self
            .http
            .request(
                Method::Post,
                &format!("/users/{me}/playlists"),
                &[],
                Some(&json!({ "name": name, "public": false })),
            )
            .await?;
        reply.into_playlist(me.as_str()).ok_or(ApiError::NotFound)
    }

    /// Renames a playlist.
    pub async fn rename_playlist(&self, id: &PlaylistId, name: &str) -> Result<(), ApiError> {
        self.http
            .call(
                Method::Put,
                &format!("/playlists/{id}"),
                &[],
                Some(&json!({ "name": name })),
            )
            .await
    }

    /// Stops following a playlist.
    ///
    /// Spotify keeps no delete for a playlist. Unfollowing is what removes it from a library, and
    /// it is what the interface offers as "Delete".
    pub async fn unfollow_playlist(&self, id: &PlaylistId) -> Result<(), ApiError> {
        self.http
            .call(
                Method::Delete,
                &format!("/playlists/{id}/followers"),
                &[],
                NO_BODY,
            )
            .await
    }

    /// Adds tracks to a playlist and returns the new version mark.
    pub async fn add_tracks(
        &self,
        id: &PlaylistId,
        tracks: &[TrackId],
        position: Option<u32>,
    ) -> Result<String, ApiError> {
        let mut body = json!({ "uris": uri_list(tracks) });
        if let Some(position) = position
            && let Some(map) = body.as_object_mut()
        {
            map.insert("position".to_owned(), json!(position));
        }
        let reply: wire::SnapshotReply = self
            .http
            .request(
                Method::Post,
                &format!("/playlists/{id}/items"),
                &[],
                Some(&body),
            )
            .await?;
        Ok(reply.snapshot_id)
    }

    /// Removes tracks from a playlist and returns the new version mark.
    pub async fn remove_tracks(
        &self,
        id: &PlaylistId,
        tracks: &[TrackId],
        snapshot_id: Option<&str>,
    ) -> Result<String, ApiError> {
        let entries: Vec<serde_json::Value> = tracks
            .iter()
            .map(|track| json!({ "uri": track.uri() }))
            .collect();
        let mut body = json!({ "items": entries });
        if let Some(snapshot) = snapshot_id
            && let Some(map) = body.as_object_mut()
        {
            map.insert("snapshot_id".to_owned(), json!(snapshot));
        }
        let reply: wire::SnapshotReply = self
            .http
            .request(
                Method::Delete,
                &format!("/playlists/{id}/items"),
                &[],
                Some(&body),
            )
            .await?;
        Ok(reply.snapshot_id)
    }

    /// Moves a run of tracks inside a playlist.
    ///
    /// Nothing in the interface drags a row yet. The endpoint is here because reordering is what
    /// the gesture will call.
    #[allow(dead_code, reason = "the endpoint is ready and the gesture is not")]
    pub async fn reorder_tracks(
        &self,
        id: &PlaylistId,
        range_start: u32,
        range_length: u32,
        insert_before: u32,
    ) -> Result<String, ApiError> {
        let reply: wire::SnapshotReply = self
            .http
            .request(
                Method::Put,
                &format!("/playlists/{id}/items"),
                &[],
                Some(&json!({
                    "range_start": range_start,
                    "range_length": range_length,
                    "insert_before": insert_before,
                })),
            )
            .await?;
        Ok(reply.snapshot_id)
    }

    // -- saved tracks ------------------------------------------------------------------------

    /// One page of the saved tracks.
    pub async fn saved_tracks(&self, offset: u32) -> Result<Page<Track>, ApiError> {
        let page: wire::Paged<wire::PlaylistItem> = self
            .http
            .request(
                Method::Get,
                "/me/tracks",
                &[
                    ("limit", PAGE.to_string()),
                    ("offset", offset.to_string()),
                ],
                NO_BODY,
            )
            .await?;
        Ok(page.map_into(|entry| entry.item.and_then(wire::FullTrack::into_track)))
    }

    /// Whether each of these tracks is saved.
    ///
    /// The answer follows the order asked for. Ask about no more than fifty at a time.
    pub async fn saved_contains(&self, ids: &[TrackId]) -> Result<Vec<bool>, ApiError> {
        let mut all = Vec::with_capacity(ids.len());
        for batch in ids.chunks(BATCH) {
            let reply: Vec<bool> = self
                .http
                .request(
                    Method::Get,
                    "/me/library/contains",
                    &[("uris", join_uris(batch))],
                    NO_BODY,
                )
                .await?;
            all.extend(reply);
        }
        Ok(all)
    }

    /// Saves tracks.
    pub async fn save_tracks(&self, ids: &[TrackId]) -> Result<(), ApiError> {
        let body = empty_body();
        for batch in ids.chunks(BATCH) {
            self.http
                .call(
                    Method::Put,
                    "/me/library",
                    &[("uris", join_uris(batch))],
                    Some(&body),
                )
                .await?;
        }
        Ok(())
    }

    /// Removes tracks from the saved ones.
    pub async fn unsave_tracks(&self, ids: &[TrackId]) -> Result<(), ApiError> {
        let body = empty_body();
        for batch in ids.chunks(BATCH) {
            self.http
                .call(
                    Method::Delete,
                    "/me/library",
                    &[("uris", join_uris(batch))],
                    Some(&body),
                )
                .await?;
        }
        Ok(())
    }

    // -- browse ------------------------------------------------------------------------------

    /// What matches `query`.
    pub async fn search(&self, query: &str) -> Result<SearchResults, ApiError> {
        let me = self.me().await?.id.clone();
        let reply: wire::SearchReply = self
            .http
            .request(
                Method::Get,
                "/search",
                &[
                    ("q", query.to_owned()),
                    ("type", "track,artist,album,playlist".to_owned()),
                    ("limit", SEARCH_LIMIT.to_string()),
                ],
                NO_BODY,
            )
            .await?;

        Ok(SearchResults {
            tracks: reply
                .tracks
                .map(|it| it.map_into(wire::FullTrack::into_track).items)
                .unwrap_or_default(),
            artists: reply
                .artists
                .map(|it| it.map_into(wire::FullArtist::into_artist).items)
                .unwrap_or_default(),
            albums: reply
                .albums
                .map(|it| it.map_into(wire::SimpleAlbum::into_album).items)
                .unwrap_or_default(),
            playlists: reply
                .playlists
                .map(|it| it.map_into(|p| p.into_playlist(me.as_str())).items)
                .unwrap_or_default(),
        })
    }

    /// One artist.
    pub async fn artist(&self, id: &ArtistId) -> Result<Artist, ApiError> {
        let reply: wire::FullArtist = self
            .http
            .request(Method::Get, &format!("/artists/{id}"), &[], NO_BODY)
            .await?;
        reply.into_artist().ok_or(ApiError::NotFound)
    }

    /// What this artist is best known for.
    ///
    /// Spotify closed this endpoint to applications registered after 2024. Return nothing when it
    /// refuses, so the artist page shows the rest of what it has.
    pub async fn artist_top_tracks(&self, id: &ArtistId) -> Result<Vec<Track>, ApiError> {
        let reply: Result<wire::TopTracks, ApiError> = self
            .http
            .request(
                Method::Get,
                &format!("/artists/{id}/top-tracks"),
                &[],
                NO_BODY,
            )
            .await;

        match reply {
            Ok(reply) => Ok(reply
                .tracks
                .into_iter()
                .filter_map(wire::FullTrack::into_track)
                .collect()),
            Err(error) if error.is_forbidden() => {
                tracing::info!(%id, "top tracks are closed to this application");
                Ok(Vec::new())
            }
            Err(error) => Err(error),
        }
    }

    /// One page of what this artist released.
    pub async fn artist_albums(&self, id: &ArtistId, offset: u32) -> Result<Page<Album>, ApiError> {
        let page: wire::Paged<wire::SimpleAlbum> = self
            .http
            .request(
                Method::Get,
                &format!("/artists/{id}/albums"),
                &[
                    ("include_groups", "album,single".to_owned()),
                    ("limit", BROWSE.to_string()),
                    ("offset", offset.to_string()),
                ],
                NO_BODY,
            )
            .await?;
        Ok(page.map_into(wire::SimpleAlbum::into_album))
    }

    /// These tracks, read back.
    ///
    /// The answer follows the order asked for. Spotify closed the batch endpoint to applications
    /// registered after 2024, so a track the cache does not hold is a request of its own, a few at
    /// a time. A track the cache holds costs nothing: tracks are kept by identifier, so one read
    /// for a playlist answers for every station and weekly mix that carries it.
    ///
    /// A track that cannot be read is left out. A rate limit is different — it means *ask later*
    /// rather than *there is no such track* — so it fails the whole list and the caller keeps what
    /// it had.
    /// Returns the tracks, and whether every one of them was actually read.
    pub async fn tracks(&self, ids: &[TrackId]) -> Result<(Vec<Track>, bool), ApiError> {
        use futures_util::stream::{self, StreamExt as _};

        let known = match self.store.get() {
            Some(cache) => cache.known_tracks(ids).await,
            None => std::collections::HashMap::new(),
        };
        let wanted: Vec<&TrackId> = ids.iter().filter(|id| !known.contains_key(*id)).collect();
        if !wanted.is_empty() {
            tracing::debug!(
                asked = wanted.len(),
                cached = known.len(),
                "reading tracks one at a time"
            );
        }

        let replies: Vec<Reply> = stream::iter(wanted.into_iter().map(|id| async move {
            let reply: Result<wire::FullTrack, ApiError> = self
                .http
                .request(Method::Get, &format!("/tracks/{id}"), &[], NO_BODY)
                .await;
            match reply {
                Ok(track) => match track.into_track() {
                    Some(track) => Reply::Read(Box::new(track)),
                    None => Reply::Gone,
                },
                // A rate limit means *ask later* rather than *there is no such track*. The rest of
                // the list is still worth showing, and the whole of it is no longer worth keeping.
                Err(ApiError::RateLimited { .. }) => Reply::Later,
                Err(error) => {
                    tracing::warn!(%error, %id, "cannot read a track");
                    Reply::Gone
                }
            }
        }))
        .buffered(TRACK_FETCHES)
        .collect()
        .await;

        let mut whole = true;
        let mut fetched = Vec::new();
        for reply in replies {
            match reply {
                Reply::Read(track) => fetched.push(*track),
                Reply::Later => whole = false,
                Reply::Gone => {}
            }
        }
        if let Some(cache) = self.store.get() {
            cache.put_known_tracks(&fetched).await;
        }

        // Back into the order asked for.
        let mut by_id: std::collections::HashMap<_, _> = known;
        by_id.extend(fetched.into_iter().map(|track| (track.id.clone(), track)));
        let found = ids.iter().filter_map(|id| by_id.remove(id)).collect();
        Ok((found, whole))
    }

    /// One album, and the tracks on it.
    pub async fn album(&self, id: &AlbumId) -> Result<(Album, Vec<Track>), ApiError> {
        let reply: wire::FullAlbum = self
            .http
            .request(Method::Get, &format!("/albums/{id}"), &[], NO_BODY)
            .await?;
        reply.split().ok_or(ApiError::NotFound)
    }

    // -- radio -------------------------------------------------------------------------------

    /// Tracks that go with this one.
    ///
    /// This is what "song radio" plays. The token comes from the session, so this endpoint answers
    /// even though it is closed to applications registered after 2024.
    pub async fn track_radio(&self, seed: &TrackId) -> Result<Vec<Track>, ApiError> {
        let reply: wire::Recommendations = self
            .http
            .request(
                Method::Get,
                "/recommendations",
                &[
                    ("seed_tracks", seed.as_str().to_owned()),
                    ("limit", "50".to_owned()),
                ],
                NO_BODY,
            )
            .await?;
        Ok(reply
            .tracks
            .into_iter()
            .filter_map(wire::FullTrack::into_track)
            .collect())
    }
}

/// What a request with no body sends.
///
/// The type has to be named for the generic to resolve. This is the name.
const NO_BODY: Option<&serde_json::Value> = None;

/// A body that says nothing, for a request that must carry one anyway.
///
/// `/me/library` takes the tracks as `uris` in the query, the way its `contains` sibling does —
/// a `{"uris": […]}` body is read by nothing there and answered with `400 Missing required
/// field: uris`. It still refuses a request carrying no body at all, with `411 Length Required`,
/// so the query needs an empty object beside it to put a length on the wire.
fn empty_body() -> serde_json::Value {
    json!({})
}

/// The fields a playlist request asks for.
const FIELDS_PLAYLIST: &str = "id,name,description,images,owner(id,display_name),tracks(total),snapshot_id";

/// These tracks as URIs, separated by commas.
fn join_uris(ids: &[TrackId]) -> String {
    ids.iter().map(TrackId::uri).collect::<Vec<_>>().join(",")
}

/// These tracks as URIs, as a list for a JSON body.
fn uri_list(ids: &[TrackId]) -> Vec<String> {
    ids.iter().map(TrackId::uri).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::pick_image;

    #[test]
    fn reads_a_track() {
        let json = r#"{
            "id": "4uLU6hMCjMI75M1A2tKUQC",
            "name": "Never Gonna Give You Up",
            "duration_ms": 213573,
            "explicit": false,
            "track_number": 1,
            "artists": [{"id": "0gxyHStUsqpMadRV0Di1Qt", "name": "Rick Astley"}],
            "album": {
                "id": "6N9PS4QXF1D0OWPk0Sxtb4",
                "name": "Whenever You Need Somebody",
                "images": [{"url": "https://i.scdn.co/image/ab1", "width": 640, "height": 640}]
            }
        }"#;
        let wire: wire::FullTrack = serde_json::from_str(json).expect("track parses");
        let track = wire.into_track().expect("track has an identifier");

        assert_eq!(track.name, "Never Gonna Give You Up");
        assert_eq!(track.artist_line(), "Rick Astley");
        assert_eq!(track.duration, Duration::from_millis(213_573));
        assert!(track.playable);
        assert_eq!(track.id.uri(), "spotify:track:4uLU6hMCjMI75M1A2tKUQC");
    }

    #[test]
    fn drops_a_local_file() {
        // A local file carries no identifier and cannot be played from here.
        let wire: wire::FullTrack =
            serde_json::from_str(r#"{"id": null, "name": "Demo", "duration_ms": 1000}"#)
                .expect("track parses");
        assert!(wire.into_track().is_none());
    }

    #[test]
    fn separates_an_owned_playlist_from_a_followed_one() {
        let json = r#"{
            "id": "37i9dQZEVXcJZyENOWUFo7",
            "name": "Discover Weekly",
            "owner": {"id": "spotify", "display_name": "Spotify"},
            "tracks": {"total": 30},
            "snapshot_id": "abc"
        }"#;
        let wire: wire::SimplePlaylist = serde_json::from_str(json).expect("playlist parses");
        let playlist = wire.into_playlist("leo").expect("playlist has an identifier");

        assert!(!playlist.kind.is_editable());
        assert_eq!(playlist.total_tracks, 30);

        let wire: wire::SimplePlaylist = serde_json::from_str(json).expect("playlist parses");
        let mine = wire.into_playlist("spotify").expect("playlist parses");
        assert!(mine.kind.is_editable());
    }

    #[test]
    fn counts_a_page_that_has_more() {
        let json = r#"{"items": [{"id": "a", "name": "A", "duration_ms": 1}], "total": 40, "offset": 0}"#;
        let paged: wire::Paged<wire::FullTrack> = serde_json::from_str(json).expect("page parses");
        let page = paged.map_into(wire::FullTrack::into_track);

        assert_eq!(page.items.len(), 1);
        assert!(page.has_more());
    }

    #[test]
    fn a_page_pages_by_what_it_covered_rather_than_by_what_it_kept() {
        // This is what used to hold a reading open forever. A playlist of two tracks, one of them
        // a local file: the page keeps one item and covers both. Counting the items left the
        // reader one short of the total, so it asked for the same page again, and again.
        let json = r#"{"items": [null, {"track": {"id": "a", "name": "A", "duration_ms": 1}}],
                       "total": 2, "offset": 0}"#;
        let paged: wire::Paged<wire::PlaylistItem> = serde_json::from_str(json).expect("parses");
        let page = paged.map_into(|entry| entry.item.and_then(wire::FullTrack::into_track));

        assert_eq!(page.items.len(), 1);
        assert_eq!(page.covered, 2);
        assert_eq!(page.next_offset(), 2);
        assert!(!page.has_more(), "two of two were read");
    }

    #[test]
    fn a_page_that_covered_nothing_is_the_end() {
        // Spotify's totals and its pages disagree often enough that the total alone cannot stop a
        // loop. An empty reply is the end whatever the total claims.
        let json = r#"{"items": [], "total": 400, "offset": 0}"#;
        let paged: wire::Paged<wire::FullTrack> = serde_json::from_str(json).expect("parses");
        let page = paged.map_into(wire::FullTrack::into_track);

        assert_eq!(page.covered, 0);
        assert!(!page.has_more());
    }

    #[test]
    fn skips_a_null_item() {
        // A playlist can hold an entry whose track Spotify no longer serves.
        let json = r#"{"items": [null, {"track": {"id": "a", "name": "A", "duration_ms": 1}}], "total": 2, "offset": 0}"#;
        let paged: wire::Paged<wire::PlaylistItem> = serde_json::from_str(json).expect("parses");
        let page = paged.map_into(|entry| entry.item.and_then(wire::FullTrack::into_track));

        assert_eq!(page.items.len(), 1);
    }

    #[test]
    fn picks_the_nearest_picture() {
        let images = vec![
            crate::models::ImageRef {
                url: "big".to_owned(),
                width: Some(640),
                height: Some(640),
            },
            crate::models::ImageRef {
                url: "small".to_owned(),
                width: Some(64),
                height: Some(64),
            },
            crate::models::ImageRef {
                url: "mid".to_owned(),
                width: Some(300),
                height: Some(300),
            },
        ];
        assert_eq!(pick_image(&images, 300).map(|it| it.url.as_str()), Some("mid"));
        assert_eq!(pick_image(&images, 640).map(|it| it.url.as_str()), Some("big"));
        assert_eq!(pick_image(&images, 50).map(|it| it.url.as_str()), Some("small"));
    }

    #[test]
    fn splits_an_album_and_puts_the_cover_on_its_tracks() {
        let json = r#"{
            "id": "alb", "name": "Album", "total_tracks": 1, "album_type": "album",
            "release_date": "1987-11-16",
            "images": [{"url": "cover", "width": 640, "height": 640}],
            "artists": [{"id": "art", "name": "Artist"}],
            "tracks": {"items": [{"id": "trk", "name": "Track", "duration_ms": 1000, "track_number": 1}], "total": 1, "offset": 0}
        }"#;
        let wire: wire::FullAlbum = serde_json::from_str(json).expect("album parses");
        let (album, tracks) = wire.split().expect("album has an identifier");

        assert_eq!(album.year, Some(1987));
        assert_eq!(tracks.len(), 1);
        let cover = tracks[0]
            .album
            .as_ref()
            .and_then(|album| pick_image(&album.images, 640))
            .map(|image| image.url.as_str());
        assert_eq!(
            cover,
            Some("cover"),
            "a track on an album page carries that album's cover"
        );
    }
}
