//! The local copy of the library.
//!
//! Everything the interface shows is kept here, so a second start draws a full window before it
//! has said a word to Spotify. Nothing here is authoritative: a sync replaces it, and a row that
//! disagrees with Spotify is stale rather than wrong.
//!
//! The queries go through `sqlx`'s macros, so every statement is checked against the schema the
//! migrations build. A column that moves is a build error.

use std::path::Path;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Pool, Sqlite};

use crate::models::{
    Album, AlbumRef, ArtistRef, ImageRef, Playlist, PlaylistId, PlaylistKind, Track, TrackId,
    UserProfile,
};

/// The name the saved tracks are filed under.
///
/// Saved tracks are a list like any other here, and Spotify gives them no identifier, so they take
/// one that no playlist can have.
const LIKED: &str = "~liked";

/// What the sidebar's own reading is filed under.
pub const LIBRARY: &str = "~library";

/// How long a cached list stands on its own.
///
/// Inside this, opening a list asks Spotify nothing. The button in the header is what reads a list
/// again before then, and an edit changes the cached copy in place.
pub const FRESH: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// What stopped the cache.
#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    /// The database could not be opened or read.
    #[error("library cache: {0}")]
    Db(#[from] sqlx::Error),
    /// The migrations could not be applied.
    #[error("library cache migration: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    /// The cache directory is unusable.
    #[error(transparent)]
    Config(#[from] crate::config::ConfigError),
}

/// The library, as it was last read.
#[derive(Debug)]
pub struct Cache {
    pool: Pool<Sqlite>,
}

impl Cache {
    /// Opens the cache under the application's cache directory.
    ///
    /// Applies any migration the file has not seen. A cache that cannot be opened is reported, and
    /// the caller carries on without one.
    pub async fn open() -> Result<Self, CacheError> {
        let path = crate::config::cache_dir()?.join("library.db");
        Self::open_at(&path).await
    }

    /// Opens the cache at `path`.
    pub async fn open_at(path: &Path) -> Result<Self, CacheError> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            // The cache is read far more than it is written, and a reader must never wait for a
            // writer to finish.
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .foreign_keys(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await?;

        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self { pool })
    }

    // -- playlists ---------------------------------------------------------------------------

    /// Every playlist, in the order the sidebar shows them.
    pub async fn playlists(&self) -> Vec<Playlist> {
        let rows = sqlx::query!(
            r#"
            SELECT id, name, description, kind, owner_name, total_tracks, snapshot_id, images
            FROM playlist
            ORDER BY position
            "#
        )
        .fetch_all(&self.pool)
        .await;

        match rows {
            Ok(rows) => rows
                .into_iter()
                .map(|row| Playlist {
                    id: Some(PlaylistId(row.id)),
                    name: row.name,
                    kind: if row.kind == "owned" {
                        PlaylistKind::Owned
                    } else {
                        PlaylistKind::Followed {
                            owner_name: row.owner_name,
                        }
                    },
                    description: row.description,
                    images: images_from(&row.images),
                    total_tracks: row.total_tracks as u32,
                    snapshot_id: row.snapshot_id,
                })
                .collect(),
            Err(error) => {
                tracing::warn!(%error, "cannot read the playlists from the cache");
                Vec::new()
            }
        }
    }

    /// Replaces the playlists with `playlists`.
    pub async fn put_playlists(&self, playlists: &[Playlist]) {
        let result = async {
            let mut tx = self.pool.begin().await?;
            sqlx::query!("DELETE FROM playlist").execute(&mut *tx).await?;

            for (position, playlist) in playlists.iter().enumerate() {
                let Some(id) = playlist.id.as_ref() else {
                    continue;
                };
                let id = id.as_str();
                let (kind, owner) = match &playlist.kind {
                    PlaylistKind::Owned => ("owned", String::new()),
                    PlaylistKind::Followed { owner_name } => ("followed", owner_name.clone()),
                    PlaylistKind::Liked => continue,
                };
                let images = images_to(&playlist.images);
                let total = i64::from(playlist.total_tracks);
                let position = position as i64;

                sqlx::query!(
                    r#"
                    INSERT INTO playlist
                        (id, name, description, kind, owner_name, total_tracks, snapshot_id,
                         position, images)
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
                    "#,
                    id,
                    playlist.name,
                    playlist.description,
                    kind,
                    owner,
                    total,
                    playlist.snapshot_id,
                    position,
                    images,
                )
                .execute(&mut *tx)
                .await?;
            }

            tx.commit().await?;
            Ok::<(), sqlx::Error>(())
        }
        .await;

        if let Err(error) = result {
            tracing::warn!(%error, "cannot keep the playlists");
            return;
        }
        self.mark_read(LIBRARY).await;
    }

    // -- how the library is arranged -----------------------------------------------------------

    /// The arrangement, as it was last read. Empty when nothing has been read yet.
    pub async fn layout(&self) -> Vec<crate::api::direct::Entry> {
        use crate::api::direct::Entry;
        use std::collections::HashMap;

        let rows = sqlx::query!(
            "SELECT kind, id, name, folder_id FROM library_entry ORDER BY position"
        )
        .fetch_all(&self.pool)
        .await;

        let Ok(rows) = rows else {
            return Vec::new();
        };

        // Where each folder sits, so a playlist naming one goes straight into it.
        let mut entries: Vec<Entry> = Vec::new();
        let mut folders: HashMap<String, usize> = HashMap::new();

        for row in rows {
            if row.kind == "folder" {
                folders.insert(row.id.clone(), entries.len());
                entries.push(Entry::Folder {
                    id: row.id,
                    name: row.name,
                    playlists: Vec::new(),
                });
                continue;
            }

            let id = PlaylistId(row.id);
            match row.folder_id.and_then(|folder| folders.get(&folder).copied()) {
                Some(at) => {
                    if let Some(Entry::Folder { playlists, .. }) = entries.get_mut(at) {
                        playlists.push(id);
                    }
                }
                None => entries.push(Entry::Playlist(id)),
            }
        }
        entries
    }

    /// Replaces the arrangement.
    pub async fn put_layout(&self, entries: &[crate::api::direct::Entry]) {
        use crate::api::direct::Entry;

        let result = async {
            let mut tx = self.pool.begin().await?;
            sqlx::query!("DELETE FROM library_entry")
                .execute(&mut *tx)
                .await?;

            let mut position = 0_i64;
            for entry in entries {
                match entry {
                    Entry::Playlist(id) => {
                        let id = id.as_str();
                        sqlx::query!(
                            "INSERT INTO library_entry (position, kind, id, name, folder_id)
                             VALUES (?, 'playlist', ?, '', NULL)",
                            position,
                            id
                        )
                        .execute(&mut *tx)
                        .await?;
                        position += 1;
                    }
                    Entry::Folder {
                        id,
                        name,
                        playlists,
                    } => {
                        sqlx::query!(
                            "INSERT INTO library_entry (position, kind, id, name, folder_id)
                             VALUES (?, 'folder', ?, ?, NULL)",
                            position,
                            id,
                            name
                        )
                        .execute(&mut *tx)
                        .await?;
                        position += 1;

                        for playlist in playlists {
                            let playlist = playlist.as_str();
                            sqlx::query!(
                                "INSERT INTO library_entry (position, kind, id, name, folder_id)
                                 VALUES (?, 'playlist', ?, '', ?)",
                                position,
                                playlist,
                                id
                            )
                            .execute(&mut *tx)
                            .await?;
                            position += 1;
                        }
                    }
                }
            }

            tx.commit().await?;
            Ok::<(), sqlx::Error>(())
        }
        .await;

        if let Err(error) = result {
            tracing::warn!(%error, "cannot keep how the library is arranged");
        }
    }

    // -- tracks ------------------------------------------------------------------------------

    /// The tracks of one list, in order.
    pub async fn tracks(&self, list: &ListKey) -> Vec<Track> {
        let key = list.as_str();
        let rows = sqlx::query!(
            r#"
            SELECT t.id, t.name, t.artists, t.album_id, t.album_name, t.album_images,
                   t.duration_ms, t.explicit, t.playable, t.track_number
            FROM playlist_track pt
            JOIN track t ON t.id = pt.track_id
            WHERE pt.playlist_id = ?
            ORDER BY pt.position
            "#,
            key
        )
        .fetch_all(&self.pool)
        .await;

        match rows {
            Ok(rows) => rows
                .into_iter()
                .map(|row| Track {
                    id: TrackId(row.id),
                    name: row.name,
                    artists: artists_from(&row.artists),
                    album: row.album_id.map(|id| AlbumRef {
                        id: crate::models::AlbumId(id),
                        name: row.album_name,
                        images: images_from(&row.album_images),
                    }),
                    duration: std::time::Duration::from_millis(row.duration_ms as u64),
                    explicit: row.explicit != 0,
                    playable: row.playable != 0,
                    track_number: row.track_number as u32,
                })
                .collect(),
            Err(error) => {
                tracing::warn!(%error, "cannot read the tracks from the cache");
                Vec::new()
            }
        }
    }

    /// Replaces the tracks of one list.
    pub async fn put_tracks(&self, list: &ListKey, tracks: &[Track]) {
        let key = list.as_str();
        let result = async {
            let mut tx = self.pool.begin().await?;
            sqlx::query!("DELETE FROM playlist_track WHERE playlist_id = ?", key)
                .execute(&mut *tx)
                .await?;

            for (position, track) in tracks.iter().enumerate() {
                write_track(&mut tx, track).await?;

                let id = track.id.as_str();
                let position = position as i64;
                sqlx::query!(
                    "INSERT INTO playlist_track (playlist_id, position, track_id) VALUES (?, ?, ?)",
                    key,
                    position,
                    id,
                )
                .execute(&mut *tx)
                .await?;
            }

            let now = now();
            sqlx::query!(
                "INSERT INTO synced (what, at) VALUES (?, ?)
                 ON CONFLICT(what) DO UPDATE SET at = excluded.at",
                key,
                now
            )
            .execute(&mut *tx)
            .await?;

            tx.commit().await?;
            Ok::<(), sqlx::Error>(())
        }
        .await;

        if let Err(error) = result {
            tracing::warn!(%error, "cannot keep the tracks");
        }
    }

    // -- who is signed in ----------------------------------------------------------------------

    /// Who was signed in when the window last closed.
    ///
    /// The header shows this before the account has been read again, so a run that starts without
    /// a network still says whose library it is showing.
    pub async fn profile(&self) -> Option<UserProfile> {
        let row = sqlx::query!(
            "SELECT id, display_name, product, images FROM account WHERE only_row = 1"
        )
        .fetch_optional(&self.pool)
        .await;

        match row {
            Ok(Some(row)) => Some(UserProfile {
                id: crate::models::UserId(row.id),
                display_name: row.display_name,
                images: images_from(&row.images),
                product: row.product,
            }),
            Ok(None) => None,
            Err(error) => {
                tracing::warn!(%error, "cannot read the account from the cache");
                None
            }
        }
    }

    /// Remembers who is signed in.
    pub async fn put_profile(&self, profile: &UserProfile) {
        let id = profile.id.as_str();
        let images = images_to(&profile.images);
        let result = sqlx::query!(
            "INSERT INTO account (only_row, id, display_name, product, images)
             VALUES (1, ?, ?, ?, ?)
             ON CONFLICT(only_row) DO UPDATE SET
                 id = excluded.id,
                 display_name = excluded.display_name,
                 product = excluded.product,
                 images = excluded.images",
            id,
            profile.display_name,
            profile.product,
            images
        )
        .execute(&self.pool)
        .await;

        if let Err(error) = result {
            tracing::warn!(%error, "cannot keep the account");
        }
    }

    /// The tracks out of `ids` this cache already holds.
    ///
    /// Tracks are kept by identifier rather than by list, so a track read for one playlist is
    /// known to every other. This is what keeps the playlists Spotify makes cheap: the Web API
    /// serves those one track at a time, and most of a station or a weekly mix is music the
    /// account has already seen.
    pub async fn known_tracks(
        &self,
        ids: &[TrackId],
    ) -> std::collections::HashMap<TrackId, Track> {
        let mut found = std::collections::HashMap::new();
        // SQLite takes a limited number of bound values, so ask in batches.
        //
        // Built rather than written out: the number of identifiers decides the number of
        // placeholders, so this is the one statement here the query macros cannot check. A builder
        // keeps the text to literals and everything else to a binding.
        for chunk in ids.chunks(400) {
            let mut builder = sqlx::QueryBuilder::<Sqlite>::new(
                "SELECT id, name, artists, album_id, album_name, album_images, duration_ms, \
                 explicit, playable, track_number FROM track WHERE id IN (",
            );
            let mut list = builder.separated(", ");
            for id in chunk {
                list.push_bind(id.as_str());
            }
            list.push_unseparated(")");

            match builder.build().fetch_all(&self.pool).await {
                Ok(rows) => {
                    use sqlx::Row as _;
                    for row in rows {
                        let id: String = row.get("id");
                        let album_id: Option<String> = row.get("album_id");
                        let album_name: String = row.get("album_name");
                        let album_images: String = row.get("album_images");
                        let duration: i64 = row.get("duration_ms");
                        let explicit: i64 = row.get("explicit");
                        let playable: i64 = row.get("playable");
                        let number: i64 = row.get("track_number");
                        let artists: String = row.get("artists");
                        let track = Track {
                            id: TrackId(id.clone()),
                            name: row.get("name"),
                            artists: artists_from(&artists),
                            album: album_id.map(|id| AlbumRef {
                                id: crate::models::AlbumId(id),
                                name: album_name,
                                images: images_from(&album_images),
                            }),
                            duration: std::time::Duration::from_millis(
                                u64::try_from(duration).unwrap_or_default(),
                            ),
                            explicit: explicit != 0,
                            playable: playable != 0,
                            track_number: u32::try_from(number).unwrap_or_default(),
                        };
                        found.insert(TrackId(id), track);
                    }
                }
                Err(error) => tracing::warn!(%error, "cannot read tracks from the cache"),
            }
        }
        found
    }

    /// Writes tracks that belong to no list of their own.
    ///
    /// A station and a playlist Spotify makes are read track by track. Keeping each one means the
    /// next list holding it costs nothing.
    pub async fn put_known_tracks(&self, tracks: &[Track]) {
        let result = async {
            let mut tx = self.pool.begin().await?;
            for track in tracks {
                write_track(&mut tx, track).await?;
            }
            tx.commit().await?;
            Ok::<(), sqlx::Error>(())
        }
        .await;

        if let Err(error) = result {
            tracing::warn!(%error, "cannot keep the tracks");
        }
    }

    /// Whether the cached copy of `list` is young enough to show on its own.
    ///
    /// A view that gets `true` shows the cache and asks Spotify nothing. This is what keeps a
    /// playlist Spotify makes affordable: the Web API refuses those, so the session gives the
    /// track identifiers and every track is then a request of its own. Reading one twice in a
    /// minute is what puts the account over its rate limit.
    pub async fn is_fresh(&self, list: &ListKey, within: std::time::Duration) -> bool {
        self.read_within(&list.as_str(), within).await
    }

    /// Whether whatever is filed under `what` was read recently.
    pub async fn read_within(&self, what: &str, within: std::time::Duration) -> bool {
        let key = what;
        let row = sqlx::query!("SELECT at FROM synced WHERE what = ?", key)
            .fetch_optional(&self.pool)
            .await;

        match row {
            Ok(Some(row)) => {
                let age = now().saturating_sub(row.at);
                age >= 0 && age < i64::try_from(within.as_secs()).unwrap_or(i64::MAX)
            }
            Ok(None) => false,
            Err(error) => {
                tracing::warn!(%error, "cannot read when the list was last read");
                false
            }
        }
    }

    /// Records that whatever is filed under `what` was just read.
    pub async fn mark_read(&self, what: &str) {
        let now = now();
        let result = sqlx::query!(
            "INSERT INTO synced (what, at) VALUES (?, ?)
             ON CONFLICT(what) DO UPDATE SET at = excluded.at",
            what,
            now
        )
        .execute(&self.pool)
        .await;
        if let Err(error) = result {
            tracing::warn!(%error, %what, "cannot record the reading");
        }
    }

    /// Marks every list as due to be read again.
    ///
    /// The button in the header calls this, so a sync means what it says whatever the cache holds.
    pub async fn stale(&self) {
        if let Err(error) = sqlx::query!("DELETE FROM synced").execute(&self.pool).await {
            tracing::warn!(%error, "cannot mark the cache as due");
        }
    }

    // -- saved tracks ------------------------------------------------------------------------

    /// Which tracks are saved.
    pub async fn saved(&self) -> Vec<TrackId> {
        match sqlx::query!("SELECT track_id FROM saved_track")
            .fetch_all(&self.pool)
            .await
        {
            Ok(rows) => rows.into_iter().map(|row| TrackId(row.track_id)).collect(),
            Err(error) => {
                tracing::warn!(%error, "cannot read the saved tracks from the cache");
                Vec::new()
            }
        }
    }

    /// Records that a track is saved, or is not.
    pub async fn set_saved(&self, id: &TrackId, saved: bool) {
        let key = id.as_str();
        let result = if saved {
            sqlx::query!(
                "INSERT INTO saved_track (track_id) VALUES (?) ON CONFLICT DO NOTHING",
                key
            )
            .execute(&self.pool)
            .await
        } else {
            sqlx::query!("DELETE FROM saved_track WHERE track_id = ?", key)
                .execute(&self.pool)
                .await
        };
        if let Err(error) = result {
            tracing::warn!(%error, "cannot record whether the track is saved");
        }
    }

    /// Replaces the saved tracks with `tracks`.
    ///
    /// The tracks themselves are written first, because a saved row names one.
    pub async fn put_saved(&self, tracks: &[Track]) {
        self.put_tracks(&ListKey::Liked, tracks).await;

        let result = async {
            let mut tx = self.pool.begin().await?;
            sqlx::query!("DELETE FROM saved_track")
                .execute(&mut *tx)
                .await?;
            for track in tracks {
                let id = track.id.as_str();
                sqlx::query!(
                    "INSERT INTO saved_track (track_id) VALUES (?) ON CONFLICT DO NOTHING",
                    id
                )
                .execute(&mut *tx)
                .await?;
            }
            tx.commit().await?;
            Ok::<(), sqlx::Error>(())
        }
        .await;

        if let Err(error) = result {
            tracing::warn!(%error, "cannot keep the saved tracks");
        }
    }

    /// Writes one album's tracks, so an album page opens from the cache.
    pub async fn put_album(&self, album: &Album, tracks: &[Track]) {
        self.put_tracks(&ListKey::Album(album.id.0.clone()), tracks)
            .await;
    }
}

/// Writes one track, replacing what was known about it.
async fn write_track(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    track: &Track,
) -> Result<(), sqlx::Error> {
    let id = track.id.as_str();
    let artists = artists_to(&track.artists);
    let album_id = track.album.as_ref().map(|album| album.id.0.clone());
    let album_name = track
        .album
        .as_ref()
        .map(|album| album.name.clone())
        .unwrap_or_default();
    let album_images = track
        .album
        .as_ref()
        .map(|album| images_to(&album.images))
        .unwrap_or_else(|| "[]".to_owned());
    let duration = track.duration.as_millis().min(i64::MAX as u128) as i64;
    let explicit = i64::from(track.explicit);
    let playable = i64::from(track.playable);
    let number = i64::from(track.track_number);

    sqlx::query!(
        r#"
        INSERT INTO track
            (id, name, artists, album_id, album_name, album_images, duration_ms,
             explicit, playable, track_number)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(id) DO UPDATE SET
            name = excluded.name,
            artists = excluded.artists,
            album_id = excluded.album_id,
            album_name = excluded.album_name,
            album_images = excluded.album_images,
            duration_ms = excluded.duration_ms,
            explicit = excluded.explicit,
            playable = excluded.playable,
            track_number = excluded.track_number
        "#,
        id,
        track.name,
        artists,
        album_id,
        album_name,
        album_images,
        duration,
        explicit,
        playable,
        number,
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Which list a set of tracks belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListKey {
    /// The saved tracks.
    Liked,
    /// One playlist.
    Playlist(String),
    /// One album.
    Album(String),
}

impl ListKey {
    /// The text this list is filed under.
    #[must_use]
    pub fn as_str(&self) -> String {
        match self {
            Self::Liked => LIKED.to_owned(),
            Self::Playlist(id) => id.clone(),
            Self::Album(id) => format!("album:{id}"),
        }
    }
}

/// Seconds since the epoch, for a sync mark.
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or_default()
}

/// Pictures, as one column.
fn images_to(images: &[ImageRef]) -> String {
    let rows: Vec<_> = images
        .iter()
        .map(|image| (image.url.as_str(), image.width, image.height))
        .collect();
    serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_owned())
}

/// Pictures, read back.
fn images_from(text: &str) -> Vec<ImageRef> {
    serde_json::from_str::<Vec<(String, Option<u32>, Option<u32>)>>(text)
        .unwrap_or_default()
        .into_iter()
        .map(|(url, width, height)| ImageRef { url, width, height })
        .collect()
}

/// Artists, as one column.
fn artists_to(artists: &[ArtistRef]) -> String {
    let rows: Vec<_> = artists
        .iter()
        .map(|artist| (artist.id.0.as_str(), artist.name.as_str()))
        .collect();
    serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_owned())
}

/// Artists, read back.
fn artists_from(text: &str) -> Vec<ArtistRef> {
    serde_json::from_str::<Vec<(String, String)>>(text)
        .unwrap_or_default()
        .into_iter()
        .map(|(id, name)| ArtistRef {
            id: crate::models::ArtistId(id),
            name,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A track called `name`, for a test.
    fn track(name: &str) -> Track {
        Track {
            id: TrackId(name.to_owned()),
            name: name.to_owned(),
            artists: vec![ArtistRef {
                id: crate::models::ArtistId("art".to_owned()),
                name: "Artist".to_owned(),
            }],
            album: Some(AlbumRef {
                id: crate::models::AlbumId("alb".to_owned()),
                name: "Album".to_owned(),
                images: vec![ImageRef {
                    url: "cover".to_owned(),
                    width: Some(64),
                    height: Some(64),
                }],
            }),
            duration: std::time::Duration::from_secs(180),
            explicit: false,
            playable: true,
            track_number: 1,
        }
    }

    /// A cache in a file of its own, removed when the test ends.
    async fn open() -> (Cache, tempdir::Dir) {
        let dir = tempdir::Dir::new();
        let cache = Cache::open_at(&dir.path().join("test.db"))
            .await
            .expect("the cache opens");
        (cache, dir)
    }

    /// The smallest temporary directory that serves.
    mod tempdir {
        use std::path::{Path, PathBuf};

        /// A directory removed when it is dropped.
        pub struct Dir(PathBuf);

        impl Dir {
            /// Makes one under the system's temporary directory.
            pub fn new() -> Self {
                static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                let count = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let path = std::env::temp_dir().join(format!("canora-test-{}-{count}", std::process::id()));
                std::fs::create_dir_all(&path).expect("the directory is made");
                Self(path)
            }

            /// Where it is.
            pub fn path(&self) -> &Path {
                &self.0
            }
        }

        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    #[tokio::test]
    async fn keeps_playlists_in_order() {
        let (cache, _dir) = open().await;
        let playlists = vec![
            Playlist {
                id: Some(PlaylistId("one".to_owned())),
                name: "One".to_owned(),
                kind: PlaylistKind::Owned,
                description: String::new(),
                images: Vec::new(),
                total_tracks: 3,
                snapshot_id: Some("s".to_owned()),
            },
            Playlist {
                id: Some(PlaylistId("two".to_owned())),
                name: "Two".to_owned(),
                kind: PlaylistKind::Followed {
                    owner_name: "Spotify".to_owned(),
                },
                description: String::new(),
                images: Vec::new(),
                total_tracks: 0,
                snapshot_id: None,
            },
        ];

        cache.put_playlists(&playlists).await;
        let read = cache.playlists().await;

        assert_eq!(read.len(), 2);
        assert_eq!(read[0].name, "One");
        assert!(read[0].kind.is_editable());
        assert!(!read[1].kind.is_editable(), "a followed playlist stays read-only");
    }

    #[tokio::test]
    async fn keeps_tracks_in_order_with_their_album() {
        let (cache, _dir) = open().await;
        let tracks = vec![track("a"), track("b"), track("c")];

        cache
            .put_tracks(&ListKey::Playlist("p".to_owned()), &tracks)
            .await;
        let read = cache.tracks(&ListKey::Playlist("p".to_owned())).await;

        let names: Vec<_> = read.iter().map(|it| it.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
        assert_eq!(read[0].artist_line(), "Artist");
        assert_eq!(
            read[0].album.as_ref().map(|it| it.images.len()),
            Some(1),
            "a cover survives the round trip"
        );
    }

    #[tokio::test]
    async fn replacing_a_list_drops_what_left_it() {
        let (cache, _dir) = open().await;
        let key = ListKey::Playlist("p".to_owned());

        cache.put_tracks(&key, &[track("a"), track("b")]).await;
        cache.put_tracks(&key, &[track("a")]).await;

        assert_eq!(cache.tracks(&key).await.len(), 1);
    }

    #[tokio::test]
    async fn remembers_which_tracks_are_saved() {
        let (cache, _dir) = open().await;
        cache.put_saved(&[track("a"), track("b")]).await;
        assert_eq!(cache.saved().await.len(), 2);

        cache.set_saved(&TrackId("a".to_owned()), false).await;
        let saved = cache.saved().await;
        assert_eq!(saved, vec![TrackId("b".to_owned())]);
    }

    #[tokio::test]
    async fn two_lists_do_not_mix() {
        let (cache, _dir) = open().await;
        cache
            .put_tracks(&ListKey::Playlist("one".to_owned()), &[track("a")])
            .await;
        cache
            .put_tracks(&ListKey::Playlist("two".to_owned()), &[track("b"), track("c")])
            .await;

        assert_eq!(cache.tracks(&ListKey::Playlist("one".to_owned())).await.len(), 1);
        assert_eq!(cache.tracks(&ListKey::Playlist("two".to_owned())).await.len(), 2);
    }
}
