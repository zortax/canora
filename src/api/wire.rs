//! The shapes the Web API sends, and how they become the types the interface reads.
//!
//! Every field the application does not draw is left out. A field Spotify may omit is an
//! [`Option`], because a missing field must not fail a whole page of tracks.

use std::time::Duration;

use serde::Deserialize;

use crate::models::{
    Album, AlbumKind, AlbumRef, Artist, ArtistRef, ImageRef, Page, Playlist, PlaylistKind, Track,
    UserProfile,
};

/// One page of anything.
#[derive(Debug, Deserialize)]
pub struct Paged<T> {
    pub items: Vec<Option<T>>,
    #[serde(default)]
    pub total: u32,
    #[serde(default)]
    pub offset: u32,
}

impl<T> Paged<T> {
    /// This page, with whatever the API left out dropped.
    pub fn map_into<U>(self, mut convert: impl FnMut(T) -> Option<U>) -> Page<U> {
        let total = self.total;
        let offset = self.offset;
        // What the reply held, before anything was dropped. This is how far the list was read,
        // and the caller pages by it.
        let covered = u32::try_from(self.items.len()).unwrap_or(u32::MAX);
        let items = self
            .items
            .into_iter()
            .flatten()
            .filter_map(&mut convert)
            .collect();
        Page {
            items,
            total,
            offset,
            covered,
            whole: true,
        }
    }
}

/// A picture.
#[derive(Debug, Deserialize)]
pub struct Image {
    pub url: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

impl From<Image> for ImageRef {
    fn from(image: Image) -> Self {
        Self {
            url: image.url,
            width: image.width,
            height: image.height,
        }
    }
}

/// An artist, as named by something else.
#[derive(Debug, Deserialize)]
pub struct SimpleArtist {
    pub id: Option<String>,
    pub name: String,
}

impl SimpleArtist {
    /// This artist, if the API gave an identifier for them.
    fn into_ref(self) -> Option<ArtistRef> {
        Some(ArtistRef {
            id: self.id?.into(),
            name: self.name,
        })
    }
}

/// An album, as named by something else.
#[derive(Debug, Deserialize)]
pub struct SimpleAlbum {
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub images: Vec<Image>,
    pub release_date: Option<String>,
    #[serde(default)]
    pub total_tracks: u32,
    pub album_type: Option<String>,
    #[serde(default)]
    pub artists: Vec<SimpleArtist>,
}

impl SimpleAlbum {
    /// This album as a reference, if the API gave an identifier for it.
    fn into_ref(self) -> Option<AlbumRef> {
        Some(AlbumRef {
            id: self.id?.into(),
            name: self.name,
            images: self.images.into_iter().map(Into::into).collect(),
        })
    }

    /// This album in full, if the API gave an identifier for it.
    pub fn into_album(self) -> Option<Album> {
        let kind = match self.album_type.as_deref() {
            Some("single") => AlbumKind::Single,
            Some("compilation") => AlbumKind::Compilation,
            _ => AlbumKind::Album,
        };
        let year = self
            .release_date
            .as_deref()
            .and_then(|date| date.get(..4))
            .and_then(|year| year.parse().ok());
        Some(Album {
            id: self.id?.into(),
            name: self.name,
            artists: self
                .artists
                .into_iter()
                .filter_map(SimpleArtist::into_ref)
                .collect(),
            images: self.images.into_iter().map(Into::into).collect(),
            year,
            total_tracks: self.total_tracks,
            kind,
        })
    }
}

/// An artist in full.
#[derive(Debug, Deserialize)]
pub struct FullArtist {
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub images: Vec<Image>,
    #[serde(default)]
    pub genres: Vec<String>,
    pub followers: Option<Followers>,
}

/// How many people follow something.
#[derive(Debug, Deserialize)]
pub struct Followers {
    pub total: Option<u64>,
}

impl FullArtist {
    /// This artist, if the API gave an identifier for them.
    pub fn into_artist(self) -> Option<Artist> {
        Some(Artist {
            id: self.id?.into(),
            name: self.name,
            images: self.images.into_iter().map(Into::into).collect(),
            genres: self.genres,
            followers: self.followers.and_then(|it| it.total),
        })
    }
}

/// A track.
#[derive(Debug, Deserialize)]
pub struct FullTrack {
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub artists: Vec<SimpleArtist>,
    pub album: Option<SimpleAlbum>,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub explicit: bool,
    pub is_playable: Option<bool>,
    #[serde(default)]
    pub track_number: u32,
}

impl FullTrack {
    /// This track, if the API gave an identifier for it.
    ///
    /// A local file has no identifier and cannot be played from here, so it is dropped.
    pub fn into_track(self) -> Option<Track> {
        Some(Track {
            id: self.id?.into(),
            name: self.name,
            artists: self
                .artists
                .into_iter()
                .filter_map(SimpleArtist::into_ref)
                .collect(),
            album: self.album.and_then(SimpleAlbum::into_ref),
            duration: Duration::from_millis(self.duration_ms),
            explicit: self.explicit,
            playable: self.is_playable.unwrap_or(true),
            track_number: self.track_number,
        })
    }
}

/// One entry of a playlist.
///
/// The endpoint calls this field `item`. It held `track` before the 2026 migration, so read either
/// one and take whichever is present.
#[derive(Debug, Deserialize)]
pub struct PlaylistItem {
    #[serde(alias = "track")]
    pub item: Option<FullTrack>,
}

/// A playlist.
#[derive(Debug, Deserialize)]
pub struct SimplePlaylist {
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub images: Vec<Image>,
    pub owner: Option<Owner>,
    pub tracks: Option<TrackCount>,
    pub snapshot_id: Option<String>,
}

/// Who owns a playlist.
#[derive(Debug, Deserialize)]
pub struct Owner {
    pub id: Option<String>,
    pub display_name: Option<String>,
}

/// How many tracks something holds.
#[derive(Debug, Deserialize)]
pub struct TrackCount {
    #[serde(default)]
    pub total: u32,
}

impl SimplePlaylist {
    /// This playlist, seen by the person whose identifier is `me`.
    ///
    /// Ownership decides what the interface offers. A playlist Spotify builds, such as Discover
    /// Weekly, is owned by Spotify and is read-only here.
    pub fn into_playlist(self, me: &str) -> Option<Playlist> {
        let owner_id = self.owner.as_ref().and_then(|it| it.id.clone());
        let kind = if owner_id.as_deref() == Some(me) {
            PlaylistKind::Owned
        } else {
            PlaylistKind::Followed {
                owner_name: self
                    .owner
                    .and_then(|it| it.display_name)
                    .unwrap_or_else(|| "Spotify".to_owned()),
            }
        };
        Some(Playlist {
            id: Some(self.id?.into()),
            name: self.name,
            kind,
            description: self.description.unwrap_or_default(),
            images: self.images.into_iter().map(Into::into).collect(),
            total_tracks: self.tracks.map(|it| it.total).unwrap_or_default(),
            snapshot_id: self.snapshot_id,
        })
    }
}

/// Who is signed in.
#[derive(Debug, Deserialize)]
pub struct PrivateUser {
    pub id: String,
    pub display_name: Option<String>,
    #[serde(default)]
    pub images: Vec<Image>,
    pub product: Option<String>,
}

impl From<PrivateUser> for UserProfile {
    fn from(user: PrivateUser) -> Self {
        Self {
            display_name: user.display_name.clone().unwrap_or_else(|| user.id.clone()),
            id: user.id.into(),
            images: user.images.into_iter().map(Into::into).collect(),
            product: user.product.unwrap_or_default(),
        }
    }
}

/// What an edit returns.
#[derive(Debug, Deserialize)]
pub struct SnapshotReply {
    pub snapshot_id: String,
}

/// What a search returns.
#[derive(Debug, Deserialize)]
pub struct SearchReply {
    pub tracks: Option<Paged<FullTrack>>,
    pub artists: Option<Paged<FullArtist>>,
    pub albums: Option<Paged<SimpleAlbum>>,
    pub playlists: Option<Paged<SimplePlaylist>>,
}

/// What the top-tracks endpoint returns.
#[derive(Debug, Deserialize)]
pub struct TopTracks {
    #[serde(default)]
    pub tracks: Vec<FullTrack>,
}

/// What the recommendations endpoint returns.
#[derive(Debug, Deserialize)]
pub struct Recommendations {
    #[serde(default)]
    pub tracks: Vec<FullTrack>,
}

/// An album with its tracks attached.
#[derive(Debug, Deserialize)]
pub struct FullAlbum {
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub images: Vec<Image>,
    pub release_date: Option<String>,
    #[serde(default)]
    pub total_tracks: u32,
    pub album_type: Option<String>,
    #[serde(default)]
    pub artists: Vec<SimpleArtist>,
    pub tracks: Option<Paged<FullTrack>>,
}

impl FullAlbum {
    /// The album, and the tracks on it.
    ///
    /// The tracks an album lists carry no album of their own, so put this album on each one. The
    /// interface reads the cover through the track.
    pub fn split(self) -> Option<(Album, Vec<Track>)> {
        let tracks = self.tracks;
        let album = SimpleAlbum {
            id: self.id,
            name: self.name,
            images: self.images,
            release_date: self.release_date,
            total_tracks: self.total_tracks,
            album_type: self.album_type,
            artists: self.artists,
        }
        .into_album()?;

        let reference = AlbumRef {
            id: album.id.clone(),
            name: album.name.clone(),
            images: album.images.clone(),
        };
        let tracks = tracks
            .map(|paged| {
                paged
                    .items
                    .into_iter()
                    .flatten()
                    .filter_map(FullTrack::into_track)
                    .map(|mut track| {
                        track.album = Some(reference.clone());
                        track
                    })
                    .collect()
            })
            .unwrap_or_default();

        Some((album, tracks))
    }
}
