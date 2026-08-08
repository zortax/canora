//! What the player reports back.
//!
//! These are the events the interface reads. They carry what the interface draws, and none of
//! librespot's own types.

use std::time::Duration;

use crate::models::{Track, TrackId};
use crate::player::queue::RepeatMode;

/// Something the player did.
#[derive(Debug, Clone)]
pub enum PlaybackEvent {
    /// A track started.
    TrackStarted {
        /// Which track.
        track: Box<Track>,
        /// Where it sits in the play order.
        index: usize,
        /// How long the play order is.
        queue_len: usize,
    },
    /// The track is playing, from this position.
    Playing {
        /// Where it is.
        position: Duration,
    },
    /// The track is paused, at this position.
    Paused {
        /// Where it is.
        position: Duration,
    },
    /// The position moved.
    Moved {
        /// Where it is now.
        position: Duration,
    },
    /// A track is loading.
    Loading,
    /// Nothing is playing.
    Stopped,
    /// A track cannot be played, so it was passed over.
    Unavailable {
        /// Which track.
        track_id: TrackId,
    },
    /// The volume changed, from zero to one.
    VolumeChanged {
        /// How loud it is.
        volume: f64,
    },
    /// Shuffle went on or off.
    ShuffleChanged(bool),
    /// What happens at the end of the queue changed.
    RepeatChanged(RepeatMode),
    /// What plays next changed.
    QueueChanged {
        /// The next few tracks.
        upcoming: Vec<Track>,
    },
    /// The connection to Spotify went, or came back.
    Connection(Connection),
}

/// The state of the connection that carries the audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connection {
    /// The connection is gone, and playback holds its position.
    ///
    /// Nothing plays until the connection comes back. A track that loads without its decryption
    /// key is noise, and it ends at the moment it starts. A queue that continues past each of
    /// those tracks empties itself in one second.
    Lost,
    /// The connection is back, and playback continues.
    Restored,
}
