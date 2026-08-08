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
use crate::player::events::{Connection, PlaybackEvent};
use crate::player::queue::{Queue, RepeatMode};
use crate::session::SessionCell;

/// How many tracks the queue panel shows.
const UPCOMING: usize = 50;

/// How far into a track "previous" restarts it rather than going back.
const RESTART_AFTER: Duration = Duration::from_secs(3);

/// The loudest the mixer goes.
const MAX_VOLUME: f64 = u16::MAX as f64;

/// How much of a track may be left when it ends for it to count as finished.
///
/// A track that ends with more time than this left has failed. The usual cause is a connection
/// that went, which leaves the pipeline with bytes that it holds no key for.
const NEARLY_OVER: Duration = Duration::from_secs(5);

/// How many tracks may fail in a row before the queue waits.
///
/// The engine asks the session first, and the session usually answers. This limit holds for the
/// moment before librespot marks the session invalid. A failure then looks like a short track.
const FAULTS_ALLOWED: u8 = 3;

/// Everything the task owns.
pub(crate) struct Engine<F> {
    player: Arc<Player>,
    mixer: Arc<dyn Mixer>,
    cache: Cache,
    /// The session the pipeline streams over. The engine asks it for its state.
    session: SessionCell,
    queue: Queue,
    report: F,
    /// Whether audio is running. The player has no such question to ask.
    playing: bool,
    /// Where the current track is, as of the last event.
    position: Duration,
    /// How long the current track runs.
    duration: Duration,
    /// How many tracks have failed in a row.
    faults: u8,
    /// Set while a dead connection holds playback. The value says whether audio was running.
    ///
    /// The engine reads this when a session arrives to replace the one that died.
    held: Option<bool>,
}

impl<F: FnMut(PlaybackEvent)> Engine<F> {
    /// Builds the engine around a player that is already running.
    pub(crate) fn new(
        player: Arc<Player>,
        mixer: Arc<dyn Mixer>,
        cache: Cache,
        session: SessionCell,
        report: F,
    ) -> Self {
        Self {
            player,
            mixer,
            cache,
            session,
            queue: Queue::default(),
            report,
            playing: false,
            position: Duration::ZERO,
            duration: Duration::ZERO,
            faults: 0,
            held: None,
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
        // A person gave this instruction. Forget the faults that came before it.
        if matches!(
            command,
            PlayerCommand::PlayTracks { .. } | PlayerCommand::Next | PlayerCommand::Previous
        ) {
            self.faults = 0;
        }

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
                // No track is in the pipeline while the connection is down. A press sets what
                // happens at the moment the connection comes back.
                if let Some(resume) = &mut self.held {
                    *resume = !*resume;
                    (self.report)(PlaybackEvent::Paused {
                        position: self.position,
                    });
                    return;
                }
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
            PlayerCommand::SessionReplaced(session) => self.carry_on(session.0),
            PlayerCommand::Shutdown => {}
        }
    }

    /// Takes the session that replaced the one that died, and starts the track again.
    ///
    /// The pipeline holds a session of its own, so the replacement turns the audio back on. The
    /// track in the pipeline came down the dead connection and holds no value. The engine
    /// therefore loads the track again, at the position where it stopped.
    fn carry_on(&mut self, session: Session) {
        self.player.set_session(session);
        self.faults = 0;
        (self.report)(PlaybackEvent::Connection(Connection::Restored));

        let was_playing = self.held.take();
        let Some(track) = self.queue.current().cloned() else {
            return;
        };
        let play = was_playing.unwrap_or(self.playing);
        let from = self.position;
        tracing::info!(name = %track.name, at = ?from, play, "carrying on where the connection went");
        self.load_at(&track, play, from);
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
                // The track played to this position, so the faults before it are over.
                if self.position > NEARLY_OVER {
                    self.faults = 0;
                }
                (self.report)(PlaybackEvent::Moved {
                    position: self.position,
                });
            }
            PlayerEvent::Loading { .. } => (self.report)(PlaybackEvent::Loading),
            PlayerEvent::EndOfTrack { .. } => {
                // The pipeline sends this event for a track that finished and for one that
                // failed.
                if finished(self.position, self.duration) {
                    self.faults = 0;
                    self.advance(true);
                } else {
                    self.faulted();
                }
            }
            PlayerEvent::TimeToPreloadNextTrack { .. } => self.preload_next(),
            PlayerEvent::Unavailable { track_id, .. } => {
                // A dead connection makes every track unavailable. Report one track only while
                // the connection is alive.
                if !self.session.is_invalid()
                    && let Ok(id) = track_id.to_id()
                {
                    (self.report)(PlaybackEvent::Unavailable {
                        track_id: TrackId(id),
                    });
                }
                // A track nobody can play must not stop the queue.
                self.faulted();
            }
            PlayerEvent::Stopped { .. } => {
                self.playing = false;
                if self.held.is_some() {
                    // The hold took the track out of the pipeline, and the queue keeps its place.
                    // The bar shows the track until the connection comes back.
                    (self.report)(PlaybackEvent::Paused {
                        position: self.position,
                    });
                } else {
                    (self.report)(PlaybackEvent::Stopped);
                }
            }
            PlayerEvent::VolumeChanged { volume } => {
                (self.report)(PlaybackEvent::VolumeChanged {
                    volume: f64::from(volume) / MAX_VOLUME,
                });
            }
            _ => {}
        }
    }

    /// Answers a track that would not play.
    ///
    /// One track that will not play is a fault in that track, and the queue goes past it. A
    /// connection that has gone makes every track fail in the same way. A queue that goes past
    /// each of those tracks empties in one second, and the person hears silence. The engine
    /// therefore asks the session for its state, and counts the failures.
    fn faulted(&mut self) {
        self.faults = self.faults.saturating_add(1);
        if self.session.is_invalid() || self.faults >= FAULTS_ALLOWED {
            self.hold();
        } else {
            self.advance(true);
        }
    }

    /// Stops where it stands and waits for a connection.
    ///
    /// The queue keeps its place. The track that stopped is the track that plays again.
    fn hold(&mut self) {
        if self.held.is_none() {
            tracing::warn!(
                at = ?self.position,
                "nothing will play; holding the queue until the connection is back"
            );
            self.held = Some(self.playing);
        }

        // Tracks that will not play show a fault in the connection, whatever the session says
        // about itself. librespot finds a socket that died quietly when its keep-alive runs out,
        // and that takes a minute and more. End the session here, and the watchdog has another
        // one in about two seconds.
        if !self.session.is_invalid() {
            tracing::warn!("the session says it is well and nothing plays; ending it");
            self.session.get().shutdown();
        }
        self.faults = 0;
        self.playing = false;
        // Take the track out of the pipeline. It came down a connection that has gone, and it
        // makes the same noise at every play.
        self.player.stop();
        (self.report)(PlaybackEvent::Connection(Connection::Lost));
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
        self.load_at(track, play, Duration::ZERO);
    }

    /// Starts a track at `from`.
    ///
    /// Playback after an outage needs a position inside the track. The track is the one that
    /// stopped, and it starts again at the position where it stopped.
    fn load_at(&mut self, track: &Track, play: bool, from: Duration) {
        let Some(uri) = track_uri(&track.id) else {
            tracing::warn!(id = %track.id, "cannot read the track identifier");
            self.advance(true);
            return;
        };

        // A dead session serves no keys. The track decodes into noise and ends at once. Keep
        // the track as the place of the queue, and let the replacement session start it.
        if self.session.is_invalid() {
            self.duration = track.duration;
            self.position = from;
            let (index, queue_len) = self.queue.position();
            (self.report)(PlaybackEvent::TrackStarted {
                track: Box::new(track.clone()),
                index,
                queue_len,
            });
            // This request holds until the connection comes back.
            self.held = Some(play);
            self.hold();
            return;
        }

        self.duration = track.duration;
        self.position = from;
        let millis = from.as_millis().min(u128::from(u32::MAX)) as u32;
        self.player.load(uri, play, millis);

        let (index, queue_len) = self.queue.position();
        (self.report)(PlaybackEvent::TrackStarted {
            track: Box::new(track.clone()),
            index,
            queue_len,
        });
        if from > Duration::ZERO {
            // `TrackStarted` puts the bar at the start. Report the true position.
            (self.report)(PlaybackEvent::Moved { position: from });
        }
    }

    /// Tells the interface what plays next.
    fn report_queue(&mut self) {
        (self.report)(PlaybackEvent::QueueChanged {
            upcoming: self.queue.upcoming(UPCOMING),
        });
    }
}

/// Whether a track that ended had played to its end.
///
/// A track that fails plays no audio, so it ends at the position where it started. A track that
/// finished ends near its length. A healthy track does not stop four minutes early.
fn finished(position: Duration, duration: Duration) -> bool {
    position + NEARLY_OVER >= duration
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The length of the track in these tests.
    const LONG: Duration = Duration::from_secs(240);

    #[test]
    fn a_track_that_ran_out_finished() {
        assert!(finished(LONG, LONG));
        // The last position report comes 500 ms before the end.
        assert!(finished(LONG - Duration::from_millis(500), LONG));
    }

    #[test]
    fn a_track_that_ended_where_it_started_did_not() {
        // A track with no decryption key ends this way. The decoder makes nothing of it, and
        // the pipeline reports the end. A queue that goes past these tracks empties.
        assert!(!finished(Duration::ZERO, LONG));
        assert!(!finished(Duration::from_secs(2), LONG));
    }

    #[test]
    fn a_track_of_no_length_is_never_held_back() {
        // The length is unknown, so the engine cannot see an early stop.
        assert!(finished(Duration::ZERO, Duration::ZERO));
    }
}
