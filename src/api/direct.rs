//! What the streaming session serves and the Web API will not.
//!
//! The session speaks to the hosts the desktop client uses. Those hosts answer for things the Web
//! API refuses a Development Mode app:
//!
//! * **The shape of the library.** `/me/playlists` is flat at every API version. Folders live only
//!   in the account's *rootlist*.
//! * **Spotify's own playlists.** Discover Weekly, Release Radar, the Daily Mixes, Blends and the
//!   Wrapped lists all answer `404` on the Web API. The session serves every one of them.
//!
//! Both replies are the same protobuf message, `SelectedListContent`: a set of attributes and a
//! list of item URIs. A folder is a pair of marker URIs around the playlists it holds.

use librespot_core::session::Session;
use librespot_protocol::playlist4_external::SelectedListContent;
use protobuf::Message;

use crate::models::{ImageRef, PlaylistId, TrackId};

/// How many entries the rootlist is asked for at once.
const PAGE: usize = 500;

/// One entry of the library, in the order the account keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// A playlist at this level.
    Playlist(PlaylistId),
    /// A folder, and what is inside it.
    Folder {
        /// What the account calls it. Folders have no other identifier.
        id: String,
        /// What it is named.
        name: String,
        /// What it holds, in order. A folder inside a folder is flattened into its parent:
        /// Spotify allows one level in its own clients, and a deeper one would be a tree the
        /// sidebar has no room for.
        playlists: Vec<PlaylistId>,
    },
}

/// One playlist, as the session describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    /// What it is called.
    pub name: String,
    /// What it says about itself.
    pub description: String,
    /// Its cover, at every size offered.
    pub images: Vec<ImageRef>,
    /// What it holds, in play order.
    pub tracks: Vec<TrackId>,
}

/// The library's shape, as the account keeps it.
///
/// Returns nothing when the session cannot answer. The caller then shows the flat list, which is
/// what the Web API gives and is never wrong, only unsorted.
pub async fn tree(session: &Session) -> Vec<Entry> {
    let bytes = match session.spclient().get_rootlist(0, Some(PAGE)).await {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::info!(%error, "the session will not say how the library is arranged");
            return Vec::new();
        }
    };
    match SelectedListContent::parse_from_bytes(&bytes) {
        Ok(content) => arrange(uris(&content)),
        Err(error) => {
            tracing::warn!(%error, "the rootlist will not decode");
            Vec::new()
        }
    }
}

/// One playlist, whoever owns it.
///
/// This is the path for the playlists Spotify makes: the Web API answers `404` for every one of
/// them, and the session answers in full.
pub async fn listing(session: &Session, id: &PlaylistId) -> Option<Listing> {
    let spotify_id = librespot_core::SpotifyId::from_base62(&id.0).ok()?;
    let bytes = match session.spclient().get_playlist(&spotify_id).await {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::info!(playlist = %id.0, %error, "the session will not serve this playlist");
            return None;
        }
    };
    let content = SelectedListContent::parse_from_bytes(&bytes)
        .inspect_err(|error| tracing::warn!(%error, "the playlist will not decode"))
        .ok()?;

    let attributes = content.attributes.as_ref();
    let tracks = uris(&content)
        .filter_map(|uri| uri.strip_prefix("spotify:track:"))
        .map(|id| TrackId(id.to_owned()))
        .collect();

    Some(Listing {
        name: attributes
            .and_then(|it| it.name.clone())
            .unwrap_or_default(),
        description: attributes
            .and_then(|it| it.description.clone())
            .unwrap_or_default(),
        images: attributes.map(cover).unwrap_or_default(),
        tracks,
    })
}

/// The item URIs of a reply, in order.
fn uris(content: &SelectedListContent) -> impl Iterator<Item = &str> {
    content
        .contents
        .as_ref()
        .into_iter()
        .flat_map(|items| items.items.iter())
        .map(|item| item.uri())
}

/// The cover of a playlist, at every size the reply offers.
///
/// Spotify's own playlists carry their cover three ways, and which one arrives varies by playlist.
/// Named sizes come first, because only those state a width.
fn cover(attributes: &librespot_protocol::playlist4_external::ListAttributes) -> Vec<ImageRef> {
    let mut images: Vec<ImageRef> = attributes
        .picture_size
        .iter()
        .filter(|size| size.has_url())
        .map(|size| ImageRef {
            url: size.url().to_owned(),
            width: width_of(size.target_name()),
            height: width_of(size.target_name()),
        })
        .collect();

    // An editorial playlist states its cover as a plain URL among its format attributes.
    if let Some(url) = attributes
        .format_attributes
        .iter()
        .find(|attribute| attribute.key() == "image_url")
    {
        images.push(ImageRef {
            url: url.value().to_owned(),
            width: None,
            height: None,
        });
    }

    // An algorithmic playlist carries an image identifier instead. The identifier is the last
    // segment of the address the covers are served from.
    if images.is_empty() && attributes.has_picture() {
        images.push(ImageRef {
            url: format!("https://i.scdn.co/image/{}", hex(attributes.picture())),
            width: None,
            height: None,
        });
    }
    images
}

/// How wide a named cover size is.
fn width_of(name: &str) -> Option<u32> {
    match name {
        "small" => Some(60),
        "default" => Some(300),
        "large" | "xlarge" => Some(640),
        _ => None,
    }
}

/// `bytes` written out as lowercase hexadecimal.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;

    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// The entries these URIs describe, in order.
fn arrange<'a>(uris: impl Iterator<Item = &'a str>) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    let mut open: Option<(String, String, Vec<PlaylistId>)> = None;

    for uri in uris {
        if let Some(rest) = uri.strip_prefix("spotify:start-group:") {
            // A second start before an end closes the first: one level is all this shows.
            if let Some((id, name, playlists)) = open.take() {
                entries.push(Entry::Folder {
                    id,
                    name,
                    playlists,
                });
            }
            let (id, name) = rest.split_once(':').unwrap_or((rest, ""));
            open = Some((id.to_owned(), decode(name), Vec::new()));
        } else if uri.starts_with("spotify:end-group:") {
            if let Some((id, name, playlists)) = open.take() {
                entries.push(Entry::Folder {
                    id,
                    name,
                    playlists,
                });
            }
        } else if let Some(id) = uri.strip_prefix("spotify:playlist:") {
            let id = PlaylistId(id.to_owned());
            match open.as_mut() {
                Some((_, _, playlists)) => playlists.push(id),
                None => entries.push(Entry::Playlist(id)),
            }
        }
    }

    // A folder left open by a truncated reply still holds what was read.
    if let Some((id, name, playlists)) = open {
        entries.push(Entry::Folder {
            id,
            name,
            playlists,
        });
    }
    entries
}

/// A folder name, with its escapes read back.
///
/// A name travels percent-encoded, because the URI it sits in is separated by colons.
fn decode(name: &str) -> String {
    let bytes = name.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' && at + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[at + 1..at + 3]).unwrap_or("");
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                at += 3;
                continue;
            }
        }
        if bytes[at] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[at]);
        }
        at += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The entries a list of URIs describes.
    fn parse(uris: &[&str]) -> Vec<Entry> {
        arrange(uris.iter().copied())
    }

    #[test]
    fn reads_a_folder_and_what_it_holds() {
        assert_eq!(
            parse(&[
                "spotify:start-group:abc:everynoise",
                "spotify:playlist:one",
                "spotify:playlist:two",
                "spotify:end-group:abc",
                "spotify:playlist:three",
            ]),
            vec![
                Entry::Folder {
                    id: "abc".to_owned(),
                    name: "everynoise".to_owned(),
                    playlists: vec![PlaylistId("one".to_owned()), PlaylistId("two".to_owned())],
                },
                Entry::Playlist(PlaylistId("three".to_owned())),
            ]
        );
    }

    #[test]
    fn a_library_with_no_folder_is_a_flat_list() {
        assert_eq!(
            parse(&["spotify:playlist:a", "spotify:playlist:b"]),
            vec![
                Entry::Playlist(PlaylistId("a".to_owned())),
                Entry::Playlist(PlaylistId("b".to_owned())),
            ]
        );
    }

    #[test]
    fn a_folder_left_open_still_holds_what_was_read() {
        // A reply cut short mid-folder must not lose the playlists it did carry.
        assert_eq!(
            parse(&["spotify:start-group:abc:name", "spotify:playlist:one"]),
            vec![Entry::Folder {
                id: "abc".to_owned(),
                name: "name".to_owned(),
                playlists: vec![PlaylistId("one".to_owned())],
            }]
        );
    }

    #[test]
    fn a_name_with_spaces_reads_back() {
        assert_eq!(decode("late+night"), "late night");
        assert_eq!(decode("caf%C3%A9"), "café");
        assert_eq!(decode("plain"), "plain");
    }

    #[test]
    fn a_second_folder_closes_the_first() {
        let entries = parse(&[
            "spotify:start-group:a:one",
            "spotify:playlist:x",
            "spotify:start-group:b:two",
            "spotify:playlist:y",
            "spotify:end-group:b",
        ]);

        assert_eq!(entries.len(), 2);
        assert!(matches!(&entries[0], Entry::Folder { name, .. } if name == "one"));
        assert!(matches!(&entries[1], Entry::Folder { name, .. } if name == "two"));
    }

    #[test]
    fn a_cover_identifier_becomes_an_address() {
        assert_eq!(hex(&[0xab, 0x67, 0x0f]), "ab670f");
    }
}
