//! What the interface knows about playback.
//!
//! Every field is a signal the interface reads. The engine runs on another thread and never writes
//! here: it reports an event, the application hands the event to the interface thread, and
//! [`PlaybackStore::apply`] writes it.
//!
//! The position is kept as an anchor rather than a number. The engine reports a position twice a
//! second, and the interface fills the gaps from the clock, so the bar moves smoothly.

use std::time::{Duration, Instant};

use zgui::prelude::*;
use zgui::reactive::{LocalStorage, RwSignal};

use crate::models::{Track, TrackId};
use crate::player::events::PlaybackEvent;
use crate::player::queue::RepeatMode;

/// What the player is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlayStatus {
    /// Nothing is loaded.
    #[default]
    Stopped,
    /// A track is on its way.
    Loading,
    /// Audio is running.
    Playing,
    /// Audio is held.
    Paused,
}

impl PlayStatus {
    /// Whether audio is running.
    #[must_use]
    pub fn is_playing(self) -> bool {
        matches!(self, Self::Playing)
    }
}

/// The track that is playing, and where it sits in the queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Playing {
    /// Which track.
    pub track: Track,
    /// Where it sits in the play order.
    pub index: usize,
    /// How long the play order is.
    pub queue_len: usize,
}

/// What the interface reads about playback.
///
/// Copy it freely: every field is a signal handle.
#[derive(Debug, Clone, Copy)]
pub struct PlaybackStore {
    /// The track that is playing.
    pub playing: RwSignal<Option<Playing>, LocalStorage>,
    /// What the player is doing.
    pub status: RwSignal<PlayStatus, LocalStorage>,
    /// The last reported position, and when it was reported.
    pub anchor: RwSignal<(Duration, Instant), LocalStorage>,
    /// How loud it is, from zero to one.
    pub volume: RwSignal<f64, LocalStorage>,
    /// Whether the order is shuffled.
    pub shuffle: RwSignal<bool, LocalStorage>,
    /// What happens at the end of the queue.
    pub repeat: RwSignal<RepeatMode, LocalStorage>,
    /// The next few tracks.
    pub upcoming: RwSignal<Vec<Track>, LocalStorage>,
    /// Ticks while audio runs, so a reader of the position wakes up.
    pub tick: RwSignal<u64, LocalStorage>,
}

impl Default for PlaybackStore {
    fn default() -> Self {
        Self::new()
    }
}

impl PlaybackStore {
    /// A store with nothing playing.
    ///
    /// Build this on the interface thread.
    #[must_use]
    pub fn new() -> Self {
        Self {
            playing: RwSignal::new_local(None),
            status: RwSignal::new_local(PlayStatus::Stopped),
            anchor: RwSignal::new_local((Duration::ZERO, Instant::now())),
            volume: RwSignal::new_local(0.5),
            shuffle: RwSignal::new_local(false),
            repeat: RwSignal::new_local(RepeatMode::Off),
            upcoming: RwSignal::new_local(Vec::new()),
            tick: RwSignal::new_local(0),
        }
    }

    /// Where the track is now.
    ///
    /// Read the anchor and add the time since it was set. Reading this subscribes to the tick, so
    /// whatever draws the position redraws while audio runs.
    #[must_use]
    pub fn position(&self) -> Duration {
        // Reading the tick subscribes to it, so a redraw follows every wake.
        let _ = self.tick.get();
        let (position, at) = self.anchor.get();
        if self.status.get().is_playing() {
            position + at.elapsed()
        } else {
            position
        }
    }

    /// How long the track that is playing runs.
    #[must_use]
    pub fn duration(&self) -> Duration {
        self.playing
            .with(|playing| playing.as_ref().map(|it| it.track.duration))
            .unwrap_or(Duration::ZERO)
    }

    /// Which track is playing.
    #[must_use]
    pub fn current_id(&self) -> Option<TrackId> {
        self.playing
            .with(|playing| playing.as_ref().map(|it| it.track.id.clone()))
    }

    /// Writes one event.
    ///
    /// Call this on the interface thread.
    pub fn apply(&self, event: PlaybackEvent) {
        match event {
            PlaybackEvent::TrackStarted {
                track,
                index,
                queue_len,
            } => {
                self.playing.set(Some(Playing {
                    track: *track,
                    index,
                    queue_len,
                }));
                self.anchor.set((Duration::ZERO, Instant::now()));
            }
            PlaybackEvent::Playing { position } => {
                self.status.set(PlayStatus::Playing);
                self.anchor.set((position, Instant::now()));
            }
            PlaybackEvent::Paused { position } => {
                self.status.set(PlayStatus::Paused);
                self.anchor.set((position, Instant::now()));
            }
            PlaybackEvent::Moved { position } => {
                self.anchor.set((position, Instant::now()));
            }
            PlaybackEvent::Loading => self.status.set(PlayStatus::Loading),
            PlaybackEvent::Stopped => {
                self.status.set(PlayStatus::Stopped);
                self.anchor.set((Duration::ZERO, Instant::now()));
            }
            PlaybackEvent::Unavailable { track_id } => {
                tracing::warn!(%track_id, "the track cannot be played");
            }
            PlaybackEvent::VolumeChanged { volume } => self.volume.set(volume),
            PlaybackEvent::ShuffleChanged(on) => self.shuffle.set(on),
            PlaybackEvent::RepeatChanged(repeat) => self.repeat.set(repeat),
            PlaybackEvent::QueueChanged { upcoming } => self.upcoming.set(upcoming),
        }
    }
}
