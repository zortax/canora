//! The task that owns the audio pipeline.
//!
//! One task holds the librespot player, the mixer and the queue. It takes instructions on a
//! channel and reads the player's own events on another. Nothing else touches the pipeline, so
//! there is no lock around it.
//!
//! The task reports through a callback. The callback runs on this task, so what it does must be
//! quick: the interface hands the event to its own thread and returns.

use std::sync::Arc;
use std::time::Duration;

use librespot_core::cache::Cache;
use librespot_core::session::Session;
use librespot_core::spotify_uri::SpotifyUri;
use librespot_playback::mixer::Mixer;
use librespot_playback::player::{Player, PlayerEvent};

use crate::models::{Track, TrackId};
use crate::player::commands::PlayerCommand;
use crate::player::events::PlaybackEvent;
use crate::player::queue::{Queue, RepeatMode};

/// How many tracks the queue panel shows.
const UPCOMING: usize = 50;

/// How far into a track "previous" restarts it rather than going back.
const RESTART_AFTER: Duration = Duration::from_secs(3);

/// The loudest the mixer goes.
const MAX_VOLUME: f64 = u16::MAX as f64;

/// Everything the task owns.
pub(crate) struct Engine<F> {
    player: Arc<Player>,
    mixer: Arc<dyn Mixer>,
    cache: Cache,
    queue: Queue,
    report: F,
    /// Whether audio is running. The player has no such question to ask.
    playing: bool,
    /// Where the current track is, as of the last event.
    position: Duration,
    /// How long the current track runs.
    duration: Duration,
}

impl<F: FnMut(PlaybackEvent)> Engine<F> {
    /// Builds the engine around a player that is already running.
    pub(crate) fn new(player: Arc<Player>, mixer: Arc<dyn Mixer>, cache: Cache, report: F) -> Self {
        Self {
            player,
            mixer,
            cache,
            queue: Queue::default(),
            report,
            playing: false,
            position: Duration::ZERO,
            duration: Duration::ZERO,
        }
    }

    /// Takes instructions and events until the interface asks it to stop.
    pub(crate) async fn run(
        mut self,
        mut commands: tokio::sync::mpsc::UnboundedReceiver<PlayerCommand>,
        mut events: librespot_playback::player::PlayerEventChannel,
    ) {
        // Tell the interface how loud it already is, from the volume the cache kept.
        let volume = f64::from(self.mixer.volume()) / MAX_VOLUME;
        (self.report)(PlaybackEvent::VolumeChanged { volume });

        loop {
            tokio::select! {
                command = commands.recv() => match command {
                    Some(PlayerCommand::Shutdown) | None => break,
                    Some(command) => self.handle_command(command),
                },
                event = events.recv() => match event {
                    Some(event) => self.handle_event(event),
                    None => break,
                },
            }
        }

        self.player.stop();
        tracing::info!("the player engine stopped");
    }

    /// Does what the interface asked.
    fn handle_command(&mut self, command: PlayerCommand) {
        match command {
            PlayerCommand::PlayTracks {
                tracks,
                start,
                context,
            } => {
                self.queue.replace(tracks, start, context);
                match self.queue.current().cloned() {
                    Some(track) => self.load(&track, true),
                    None => {
                        self.player.stop();
                        self.playing = false;
                        (self.report)(PlaybackEvent::Stopped);
                    }
                }
                self.report_queue();
            }
            PlayerCommand::TogglePlay => {
                // Say so before the pipeline agrees. Every sink drains its buffer before it
                // pauses, so the audio stops a moment later; the control should not wait for it.
                if self.playing {
                    self.playing = false;
                    self.player.pause();
                    (self.report)(PlaybackEvent::Paused {
                        position: self.position,
                    });
                } else {
                    self.playing = true;
                    self.player.play();
                    (self.report)(PlaybackEvent::Playing {
                        position: self.position,
                    });
                }
            }
            PlayerCommand::Next => self.advance(false),
            PlayerCommand::Previous => {
                // Far enough into a track, "previous" means "start this one again".
                if self.position > RESTART_AFTER {
                    self.player.seek(0);
                    return;
                }
                if let Some(track) = self.queue.previous() {
                    self.load(&track, true);
                    self.report_queue();
                }
            }
            PlayerCommand::Seek(position) => {
                let millis = position.as_millis().min(u128::from(u32::MAX)) as u32;
                self.player.seek(millis);
                // Report at once. The interface moves the bar without waiting for the pipeline.
                self.position = position;
                (self.report)(PlaybackEvent::Moved { position });
            }
            PlayerCommand::SetVolume(volume) => {
                let level = (volume.clamp(0.0, 1.0) * MAX_VOLUME) as u16;
                self.mixer.set_volume(level);
                self.cache.save_volume(level);
                (self.report)(PlaybackEvent::VolumeChanged {
                    volume: volume.clamp(0.0, 1.0),
                });
            }
            PlayerCommand::SetShuffle(on) => {
                self.queue.set_shuffle(on);
                (self.report)(PlaybackEvent::ShuffleChanged(on));
                self.report_queue();
            }
            PlayerCommand::SetRepeat(repeat) => {
                self.queue.set_repeat(repeat);
                (self.report)(PlaybackEvent::RepeatChanged(repeat));
            }
            PlayerCommand::PlayNext(track) => {
                self.queue.play_next(track);
                self.report_queue();
            }
            PlayerCommand::Shutdown => {}
        }
    }

    /// Reads what the pipeline did.
    fn handle_event(&mut self, event: PlayerEvent) {
        match event {
            PlayerEvent::Playing { position_ms, .. } => {
                self.playing = true;
                self.position = Duration::from_millis(u64::from(position_ms));
                (self.report)(PlaybackEvent::Playing {
                    position: self.position,
                });
            }
            PlayerEvent::Paused { position_ms, .. } => {
                self.playing = false;
                self.position = Duration::from_millis(u64::from(position_ms));
                (self.report)(PlaybackEvent::Paused {
                    position: self.position,
                });
            }
            PlayerEvent::Seeked { position_ms, .. }
            | PlayerEvent::PositionCorrection { position_ms, .. }
            | PlayerEvent::PositionChanged { position_ms, .. } => {
                self.position = Duration::from_millis(u64::from(position_ms));
                (self.report)(PlaybackEvent::Moved {
                    position: self.position,
                });
            }
            PlayerEvent::Loading { .. } => (self.report)(PlaybackEvent::Loading),
            PlayerEvent::EndOfTrack { .. } => self.advance(true),
            PlayerEvent::TimeToPreloadNextTrack { .. } => self.preload_next(),
            PlayerEvent::Unavailable { track_id, .. } => {
                if let Ok(id) = track_id.to_id() {
                    (self.report)(PlaybackEvent::Unavailable {
                        track_id: TrackId(id),
                    });
                }
                // A track nobody can play must not stop the queue.
                self.advance(true);
            }
            PlayerEvent::Stopped { .. } => {
                self.playing = false;
                (self.report)(PlaybackEvent::Stopped);
            }
            PlayerEvent::VolumeChanged { volume } => {
                (self.report)(PlaybackEvent::VolumeChanged {
                    volume: f64::from(volume) / MAX_VOLUME,
                });
            }
            _ => {}
        }
    }

    /// Moves to the next track, or stops.
    fn advance(&mut self, automatic: bool) {
        match self.queue.next(automatic) {
            Some(track) => {
                self.load(&track, true);
                self.report_queue();
            }
            None => {
                self.player.stop();
                self.playing = false;
                self.position = Duration::ZERO;
                (self.report)(PlaybackEvent::Stopped);
            }
        }
    }

    /// Tells the pipeline to read the next track before it is needed.
    ///
    /// This is what makes one track follow another with no gap.
    fn preload_next(&self) {
        if self.queue.repeat() == RepeatMode::One {
            return;
        }
        if let Some(track) = self.queue.upcoming(1).first()
            && let Some(uri) = track_uri(&track.id)
        {
            self.player.preload(uri);
        }
    }

    /// Starts a track.
    fn load(&mut self, track: &Track, play: bool) {
        let Some(uri) = track_uri(&track.id) else {
            tracing::warn!(id = %track.id, "cannot read the track identifier");
            self.advance(true);
            return;
        };

        self.duration = track.duration;
        self.position = Duration::ZERO;
        self.player.load(uri, play, 0);

        let (index, queue_len) = self.queue.position();
        (self.report)(PlaybackEvent::TrackStarted {
            track: Box::new(track.clone()),
            index,
            queue_len,
        });
    }

    /// Tells the interface what plays next.
    fn report_queue(&mut self) {
        (self.report)(PlaybackEvent::QueueChanged {
            upcoming: self.queue.upcoming(UPCOMING),
        });
    }
}

/// The URI the pipeline loads a track by.
fn track_uri(id: &TrackId) -> Option<SpotifyUri> {
    match SpotifyUri::from_uri(&id.uri()) {
        Ok(uri) => Some(uri),
        Err(error) => {
            tracing::warn!(%error, %id, "cannot read the track identifier");
            None
        }
    }
}

/// The audio pipeline: what plays, what sets the volume, and what it reports through.
pub(crate) type Pipeline = (
    Arc<Player>,
    Arc<dyn Mixer>,
    librespot_playback::player::PlayerEventChannel,
);

/// Builds the pipeline and starts the task.
///
/// The session is the one the login made. The cache holds the volume from last time.
pub(crate) fn build(session: Session, cache: Cache) -> Result<Pipeline, crate::player::PlayerError> {
    use librespot_playback::audio_backend;
    use librespot_playback::config::{AudioFormat, PlayerConfig};
    use librespot_playback::mixer::{self, MixerConfig};

    let backend = audio_backend::find(None).ok_or(crate::player::PlayerError::NoBackend)?;
    let make_mixer = mixer::find(None).ok_or(crate::player::PlayerError::NoMixer)?;
    let mixer = make_mixer(MixerConfig::default())?;

    // Start at the volume the last run ended on, or half way.
    let volume = cache.volume().unwrap_or(u16::MAX / 2);
    mixer.set_volume(volume);

    let config = PlayerConfig {
        // Ask for the position while a track plays, so the bar follows without guessing.
        position_update_interval: Some(Duration::from_millis(500)),
        ..PlayerConfig::default()
    };

    let player = Player::new(config, session, mixer.get_soft_volume(), move || {
        backend(None, AudioFormat::default())
    });
    let events = player.get_player_event_channel();

    Ok((player, mixer, events))
}
