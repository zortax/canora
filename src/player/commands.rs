//! What the interface asks the player to do.

use std::time::Duration;

use crate::models::Track;
use crate::player::queue::{PlayContext, RepeatMode};

/// One instruction for the player.
#[derive(Debug)]
pub enum PlayerCommand {
    /// Replaces the queue and starts playing.
    PlayTracks {
        /// What to play.
        tracks: Vec<Track>,
        /// Which one to start with.
        start: usize,
        /// Where the tracks came from.
        context: PlayContext,
    },
    /// Plays if paused. Pauses if playing.
    TogglePlay,
    /// Moves to the next track.
    Next,
    /// Moves to the previous track, or back to the start of this one.
    Previous,
    /// Moves the position inside the track.
    Seek(Duration),
    /// Sets how loud it is, from zero to one.
    SetVolume(f64),
    /// Turns shuffle on or off.
    SetShuffle(bool),
    /// Sets what happens at the end of the queue.
    SetRepeat(RepeatMode),
    /// Puts a track next.
    PlayNext(Track),
    /// Stops the engine.
    Shutdown,
}

/// Sends instructions to the player.
///
/// Every copy writes to the same engine. Sending after the engine stops does nothing.
#[derive(Debug, Clone)]
pub struct PlayerHandle {
    tx: tokio::sync::mpsc::UnboundedSender<PlayerCommand>,
}

impl PlayerHandle {
    /// Sends to this channel.
    pub(crate) fn new(tx: tokio::sync::mpsc::UnboundedSender<PlayerCommand>) -> Self {
        Self { tx }
    }

    /// Sends one instruction.
    ///
    /// A closed engine is reported and dropped. The interface has nothing to do about it.
    pub fn send(&self, command: PlayerCommand) {
        if self.tx.send(command).is_err() {
            tracing::warn!("the player engine is gone");
        }
    }

    /// Replaces the queue and starts playing.
    pub fn play_tracks(&self, tracks: Vec<Track>, start: usize, context: PlayContext) {
        self.send(PlayerCommand::PlayTracks {
            tracks,
            start,
            context,
        });
    }

    /// Plays if paused. Pauses if playing.
    pub fn toggle_play(&self) {
        self.send(PlayerCommand::TogglePlay);
    }

    /// Moves to the next track.
    pub fn next(&self) {
        self.send(PlayerCommand::Next);
    }

    /// Moves to the previous track.
    pub fn previous(&self) {
        self.send(PlayerCommand::Previous);
    }

    /// Moves the position inside the track.
    pub fn seek(&self, position: Duration) {
        self.send(PlayerCommand::Seek(position));
    }

    /// Sets how loud it is, from zero to one.
    pub fn set_volume(&self, volume: f64) {
        self.send(PlayerCommand::SetVolume(volume));
    }

    /// Turns shuffle on or off.
    pub fn set_shuffle(&self, on: bool) {
        self.send(PlayerCommand::SetShuffle(on));
    }

    /// Sets what happens at the end of the queue.
    pub fn set_repeat(&self, repeat: RepeatMode) {
        self.send(PlayerCommand::SetRepeat(repeat));
    }

    /// Puts a track next.
    pub fn play_next(&self, track: Track) {
        self.send(PlayerCommand::PlayNext(track));
    }
}
