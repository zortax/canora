//! What the interface shows.
//!
//! These types hold what the Web API returns, in the shape the interface reads. Identifiers are
//! newtypes: a playlist identifier and a track identifier are both text, and the type system keeps
//! them apart.

use std::time::Duration;

/// Declares one identifier type.
macro_rules! id {
    ($name:ident, $kind:literal, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub String);

        impl $name {
            /// The identifier as text.
            #[must_use]
            #[allow(dead_code, reason = "every identifier type offers the same two accessors")]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// The Spotify URI for this identifier.
            #[must_use]
            #[allow(dead_code, reason = "every identifier type offers the same two accessors")]
            pub fn uri(&self) -> String {
                format!("spotify:{}:{}", $kind, self.0)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

id!(TrackId, "track", "A track.");
id!(AlbumId, "album", "An album.");
id!(ArtistId, "artist", "An artist.");
id!(PlaylistId, "playlist", "A playlist.");
id!(UserId, "user", "A person.");

/// One picture, at one size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRef {
    /// Where to fetch it.
    pub url: String,
    /// How wide it is, when the API says.
    pub width: Option<u32>,
    /// How tall it is, when the API says.
    pub height: Option<u32>,
}

/// Picks the picture nearest `want` pixels wide.
///
/// A list wants a small picture and a header wants a large one. Fetching the large one for a list
/// costs bandwidth and decode time for pixels nothing draws.
#[must_use]
pub fn pick_image(images: &[ImageRef], want: u32) -> Option<&ImageRef> {
    images.iter().min_by_key(|image| {
        let width = image.width.unwrap_or(640);
        width.abs_diff(want)
    })
}

/// An artist, as named by something else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtistRef {
    /// Which artist.
    pub id: ArtistId,
    /// What they are called.
    pub name: String,
}

/// An album, as named by something else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlbumRef {
    /// Which album.
    pub id: AlbumId,
    /// What it is called.
    pub name: String,
    /// Its cover.
    pub images: Vec<ImageRef>,
}

/// One track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    /// Which track.
    pub id: TrackId,
    /// What it is called.
    pub name: String,
    /// Who made it.
    pub artists: Vec<ArtistRef>,
    /// Which album it is on. A local file has none.
    pub album: Option<AlbumRef>,
    /// How long it runs.
    pub duration: Duration,
    /// Whether it carries an explicit-content mark.
    pub explicit: bool,
    /// Whether this market can play it.
    pub playable: bool,
    /// Which number it is on its album.
    pub track_number: u32,
}

impl Track {
    /// Every artist, in one line.
    #[must_use]
    pub fn artist_line(&self) -> String {
        self.artists
            .iter()
            .map(|artist| artist.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

}

/// What kind of release an album is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AlbumKind {
    /// A full album.
    #[default]
    Album,
    /// A single or an EP.
    Single,
    /// A compilation.
    Compilation,
}

/// One album.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Album {
    /// Which album.
    pub id: AlbumId,
    /// What it is called.
    pub name: String,
    /// Who made it.
    pub artists: Vec<ArtistRef>,
    /// Its cover, at every size offered.
    pub images: Vec<ImageRef>,
    /// The year it came out.
    pub year: Option<u16>,
    /// How many tracks it holds.
    pub total_tracks: u32,
    /// What kind of release it is.
    pub kind: AlbumKind,
}

/// One artist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artist {
    /// Which artist.
    pub id: ArtistId,
    /// What they are called.
    pub name: String,
    /// Their picture, at every size offered.
    pub images: Vec<ImageRef>,
    /// What they are filed under.
    pub genres: Vec<String>,
    /// How many people follow them.
    pub followers: Option<u64>,
}

/// What kind of playlist this is, and what may be done to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlaylistKind {
    /// Saved tracks. Spotify builds this one and the API reaches it through its own endpoints.
    ///
    /// The saved tracks have a view of their own rather than a row in the playlist list, so
    /// nothing builds this yet. It stays because the kind is what decides read-only.
    #[allow(dead_code, reason = "the saved tracks reach the interface through their own view")]
    Liked,
    /// A playlist this person owns. Everything may be edited.
    Owned,
    /// A playlist somebody else owns, including the ones Spotify makes. Read it only.
    Followed {
        /// Who owns it.
        owner_name: String,
    },
}

impl PlaylistKind {
    /// Whether this person may change the tracks in it.
    #[must_use]
    pub fn is_editable(&self) -> bool {
        matches!(self, Self::Owned)
    }
}

/// One playlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Playlist {
    /// Which playlist. Saved tracks have no identifier.
    pub id: Option<PlaylistId>,
    /// What it is called.
    pub name: String,
    /// What kind it is.
    pub kind: PlaylistKind,
    /// What it says about itself.
    pub description: String,
    /// Its cover, at every size offered.
    pub images: Vec<ImageRef>,
    /// How many tracks it holds.
    pub total_tracks: u32,
    /// Which version of it was read. Send this back with an edit.
    pub snapshot_id: Option<String>,
}

/// One row of the sidebar.
///
/// The account arranges its playlists, sometimes in folders. This is that arrangement, with the
/// playlists themselves rather than their identifiers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibraryNode {
    /// A playlist at the top level.
    Playlist(Playlist),
    /// A folder, and what is inside it.
    Folder {
        /// What the account calls it.
        id: String,
        /// What it is named.
        name: String,
        /// What it holds, in order.
        playlists: Vec<Playlist>,
    },
}

/// One page of a list the API hands out in parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    /// What this page holds.
    pub items: Vec<T>,
    /// How many items there are in total.
    pub total: u32,
    /// Where this page starts.
    pub offset: u32,
    /// Whether everything in this range was actually read.
    ///
    /// False when Spotify rate limited part of it. The items are still worth showing, and they are
    /// not worth keeping: a short list written to the cache and marked fresh is what the reader
    /// would then see until it went stale.
    pub whole: bool,
    /// How much of the list this page got through.
    ///
    /// This is larger than `items` whenever something in the range could not be read: a local
    /// file, a track Spotify no longer serves, a reply this application does not understand. What
    /// pages through a list must count this rather than the items, or a list holding one
    /// unreadable track never reaches its end and asks Spotify for the same page forever.
    pub covered: u32,
}

impl<T> Page<T> {
    /// Where the page after this one starts.
    #[must_use]
    pub fn next_offset(&self) -> u32 {
        self.offset.saturating_add(self.covered)
    }

    /// Whether another page follows this one.
    ///
    /// A page that covered nothing is the end, whatever the total says. Spotify's totals and its
    /// pages disagree often enough that only one of them can be trusted to stop a loop.
    #[must_use]
    pub fn has_more(&self) -> bool {
        self.covered > 0 && self.next_offset() < self.total
    }
}

/// Who is signed in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserProfile {
    /// Which account.
    pub id: UserId,
    /// What they are called.
    pub display_name: String,
    /// Their picture.
    pub images: Vec<ImageRef>,
    /// Which plan they are on.
    pub product: String,
}

/// What a search returned.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchResults {
    /// Tracks that matched.
    pub tracks: Vec<Track>,
    /// Artists that matched.
    pub artists: Vec<Artist>,
    /// Albums that matched.
    pub albums: Vec<Album>,
    /// Playlists that matched.
    pub playlists: Vec<Playlist>,
}

impl SearchResults {
    /// Whether nothing matched.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
            && self.artists.is_empty()
            && self.albums.is_empty()
            && self.playlists.is_empty()
    }
}
