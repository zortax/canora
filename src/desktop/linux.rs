//! What the desktop sees of the player.
//!
//! MPRIS is the interface a desktop reads to show a media player in its shell and to answer the
//! keys on a keyboard. It runs on D-Bus, away from the interface thread.
//!
//! The engine reports what it does to one place, so this holds a copy of that and answers from it.
//! Nothing here asks the engine a question: a D-Bus call that had to wait for the audio thread
//! would stall the desktop's own shell.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use mpris_server::zbus::fdo;
use mpris_server::{
    LoopStatus, Metadata, PlaybackRate, PlaybackStatus, PlayerInterface, Property, RootInterface,
    Server, Signal, Time, TrackId, Volume,
};

use crate::models::Track;
use crate::player::state::PlayStatus;
use crate::player::{PlaybackEvent, PlayerHandle, RepeatMode};

/// What the desktop is told about.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// The track that is playing.
    pub track: Option<Track>,
    /// What the player is doing.
    pub status: PlayStatus,
    /// Where the track is.
    pub position: Duration,
    /// How loud it is, from zero to one.
    pub volume: f64,
    /// Whether the order is shuffled.
    pub shuffle: bool,
    /// What happens at the end of the queue.
    pub repeat: RepeatMode,
    /// Whether anything follows the track that is playing.
    pub has_next: bool,
}

impl Snapshot {
    /// Writes one event, and says which properties it changed.
    ///
    /// A desktop that is told nothing shows a stale track, and one that is told everything on
    /// every position report wakes for no reason twice a second.
    fn apply(&mut self, event: &PlaybackEvent) -> Vec<Property> {
        match event {
            PlaybackEvent::TrackStarted { track, .. } => {
                self.track = Some((**track).clone());
                self.position = Duration::ZERO;
                vec![
                    Property::Metadata(self.metadata()),
                    Property::CanSeek(true),
                    Property::CanPlay(true),
                    Property::CanPause(true),
                    Property::CanGoPrevious(true),
                ]
            }
            PlaybackEvent::Playing { position } => {
                self.status = PlayStatus::Playing;
                self.position = *position;
                vec![Property::PlaybackStatus(PlaybackStatus::Playing)]
            }
            PlaybackEvent::Paused { position } => {
                self.status = PlayStatus::Paused;
                self.position = *position;
                vec![Property::PlaybackStatus(PlaybackStatus::Paused)]
            }
            PlaybackEvent::Moved { position } => {
                self.position = *position;
                Vec::new()
            }
            PlaybackEvent::Loading => Vec::new(),
            PlaybackEvent::Stopped => {
                self.status = PlayStatus::Stopped;
                self.position = Duration::ZERO;
                vec![Property::PlaybackStatus(PlaybackStatus::Stopped)]
            }
            PlaybackEvent::Unavailable { .. } => Vec::new(),
            PlaybackEvent::VolumeChanged { volume } => {
                self.volume = *volume;
                vec![Property::Volume(*volume)]
            }
            PlaybackEvent::ShuffleChanged(on) => {
                self.shuffle = *on;
                vec![Property::Shuffle(*on)]
            }
            PlaybackEvent::RepeatChanged(repeat) => {
                self.repeat = *repeat;
                vec![Property::LoopStatus(match repeat {
                    RepeatMode::Off => LoopStatus::None,
                    RepeatMode::All => LoopStatus::Playlist,
                    RepeatMode::One => LoopStatus::Track,
                })]
            }
            PlaybackEvent::QueueChanged { upcoming } => {
                self.has_next = !upcoming.is_empty();
                vec![Property::CanGoNext(self.has_next)]
            }
            // The desktop reads the state of the connection from the playback status.
            PlaybackEvent::Connection(_) => Vec::new(),
        }
    }

    /// What the desktop shows about the track.
    fn metadata(&self) -> Metadata {
        let mut metadata = Metadata::new();
        let Some(track) = &self.track else {
            return metadata;
        };

        // A track identifier is a D-Bus object path, so it takes the shape of one.
        if let Ok(id) = TrackId::try_from(format!("/dev/canora/track/{}", sanitise(&track.id.0))) {
            metadata.set_trackid(Some(id));
        }
        metadata.set_title(Some(track.name.clone()));
        metadata.set_artist(Some(track.artists.iter().map(|artist| artist.name.clone())));
        if let Some(album) = &track.album {
            metadata.set_album(Some(album.name.clone()));
            if let Some(image) = crate::models::pick_image(&album.images, 640) {
                metadata.set_art_url(Some(image.url.clone()));
            }
        }
        metadata.set_length(Some(Time::from_micros(
            track.duration.as_micros().min(i64::MAX as u128) as i64,
        )));
        metadata
    }
}

/// Only what a D-Bus object path allows.
fn sanitise(id: &str) -> String {
    id.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// The player, as the desktop sees it.
struct Media {
    /// What the engine last reported.
    snapshot: Arc<Mutex<Snapshot>>,
    /// Where an instruction goes.
    player: PlayerHandle,
}

impl Media {
    /// Reads the snapshot, or the default if another thread left it poisoned.
    fn read(&self) -> Snapshot {
        self.snapshot
            .lock()
            .map(|snapshot| snapshot.clone())
            .unwrap_or_default()
    }
}

impl RootInterface for Media {
    async fn raise(&self) -> fdo::Result<()> {
        Ok(())
    }

    async fn quit(&self) -> fdo::Result<()> {
        Ok(())
    }

    async fn can_quit(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn fullscreen(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn set_fullscreen(&self, _: bool) -> mpris_server::zbus::Result<()> {
        Ok(())
    }

    async fn can_set_fullscreen(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn can_raise(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn has_track_list(&self) -> fdo::Result<bool> {
        Ok(false)
    }

    async fn identity(&self) -> fdo::Result<String> {
        Ok("canora".to_owned())
    }

    async fn desktop_entry(&self) -> fdo::Result<String> {
        Ok("canora".to_owned())
    }

    async fn supported_uri_schemes(&self) -> fdo::Result<Vec<String>> {
        Ok(Vec::new())
    }

    async fn supported_mime_types(&self) -> fdo::Result<Vec<String>> {
        Ok(Vec::new())
    }
}

impl PlayerInterface for Media {
    async fn next(&self) -> fdo::Result<()> {
        self.player.next();
        Ok(())
    }

    async fn previous(&self) -> fdo::Result<()> {
        self.player.previous();
        Ok(())
    }

    async fn pause(&self) -> fdo::Result<()> {
        if self.read().status.is_playing() {
            self.player.toggle_play();
        }
        Ok(())
    }

    async fn play_pause(&self) -> fdo::Result<()> {
        self.player.toggle_play();
        Ok(())
    }

    async fn stop(&self) -> fdo::Result<()> {
        if self.read().status.is_playing() {
            self.player.toggle_play();
        }
        Ok(())
    }

    async fn play(&self) -> fdo::Result<()> {
        if !self.read().status.is_playing() {
            self.player.toggle_play();
        }
        Ok(())
    }

    async fn seek(&self, offset: Time) -> fdo::Result<()> {
        let now = self.read().position.as_micros() as i64;
        let wanted = (now + offset.as_micros()).max(0);
        self.player
            .seek(Duration::from_micros(wanted.unsigned_abs()));
        Ok(())
    }

    async fn set_position(&self, _: TrackId, position: Time) -> fdo::Result<()> {
        self.player
            .seek(Duration::from_micros(position.as_micros().max(0).unsigned_abs()));
        Ok(())
    }

    async fn open_uri(&self, _: String) -> fdo::Result<()> {
        Ok(())
    }

    async fn playback_status(&self) -> fdo::Result<PlaybackStatus> {
        Ok(match self.read().status {
            PlayStatus::Playing => PlaybackStatus::Playing,
            PlayStatus::Paused | PlayStatus::Loading => PlaybackStatus::Paused,
            PlayStatus::Stopped => PlaybackStatus::Stopped,
        })
    }

    async fn loop_status(&self) -> fdo::Result<LoopStatus> {
        Ok(match self.read().repeat {
            RepeatMode::Off => LoopStatus::None,
            RepeatMode::All => LoopStatus::Playlist,
            RepeatMode::One => LoopStatus::Track,
        })
    }

    async fn set_loop_status(&self, status: LoopStatus) -> mpris_server::zbus::Result<()> {
        self.player.set_repeat(match status {
            LoopStatus::None => RepeatMode::Off,
            LoopStatus::Playlist => RepeatMode::All,
            LoopStatus::Track => RepeatMode::One,
        });
        Ok(())
    }

    async fn rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn set_rate(&self, _: PlaybackRate) -> mpris_server::zbus::Result<()> {
        Ok(())
    }

    async fn shuffle(&self) -> fdo::Result<bool> {
        Ok(self.read().shuffle)
    }

    async fn set_shuffle(&self, on: bool) -> mpris_server::zbus::Result<()> {
        self.player.set_shuffle(on);
        Ok(())
    }

    async fn metadata(&self) -> fdo::Result<Metadata> {
        Ok(self.read().metadata())
    }

    async fn volume(&self) -> fdo::Result<Volume> {
        Ok(self.read().volume)
    }

    async fn set_volume(&self, volume: Volume) -> mpris_server::zbus::Result<()> {
        self.player.set_volume(volume.clamp(0.0, 1.0));
        Ok(())
    }

    async fn position(&self) -> fdo::Result<Time> {
        Ok(Time::from_micros(
            self.read().position.as_micros().min(i64::MAX as u128) as i64,
        ))
    }

    async fn minimum_rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn maximum_rate(&self) -> fdo::Result<PlaybackRate> {
        Ok(1.0)
    }

    async fn can_go_next(&self) -> fdo::Result<bool> {
        Ok(self.read().has_next)
    }

    async fn can_go_previous(&self) -> fdo::Result<bool> {
        Ok(self.read().track.is_some())
    }

    async fn can_play(&self) -> fdo::Result<bool> {
        Ok(self.read().track.is_some())
    }

    async fn can_pause(&self) -> fdo::Result<bool> {
        Ok(self.read().track.is_some())
    }

    async fn can_seek(&self) -> fdo::Result<bool> {
        Ok(self.read().track.is_some())
    }

    async fn can_control(&self) -> fdo::Result<bool> {
        Ok(true)
    }
}

/// Tells the desktop about the player, and takes what it asks for.
///
/// Reads the engine's events from `rx`. A desktop with no session bus is reported and the rest of
/// the program carries on: MPRIS is a convenience, not a requirement.
pub fn relay(
    runtime: &tokio::runtime::Handle,
    player: PlayerHandle,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<PlaybackEvent>,
) {
    let snapshot = Arc::new(Mutex::new(Snapshot::default()));

    runtime.spawn({
        let snapshot = snapshot.clone();
        async move {
            let media = Media {
                snapshot: snapshot.clone(),
                player,
            };
            let server = match Server::new("canora", media).await {
                Ok(server) => server,
                Err(error) => {
                    tracing::info!(%error, "no MPRIS: the desktop will not show the player");
                    // Drain, so the sender never fills.
                    while rx.recv().await.is_some() {}
                    return;
                }
            };
            tracing::info!("MPRIS is up");

            while let Some(event) = rx.recv().await {
                let seeked = matches!(event, PlaybackEvent::Moved { .. });
                let changed = match snapshot.lock() {
                    Ok(mut held) => held.apply(&event),
                    Err(_) => Vec::new(),
                };

                if !changed.is_empty()
                    && let Err(error) = server.properties_changed(changed).await
                {
                    tracing::warn!(%error, "cannot tell the desktop what changed");
                }

                // A seek is a signal rather than a property: a desktop that only heard the new
                // number could not tell a seek from the track running on.
                if seeked {
                    let at = snapshot
                        .lock()
                        .map(|held| held.position)
                        .unwrap_or_default();
                    let at = Time::from_micros(at.as_micros().min(i64::MAX as u128) as i64);
                    if let Err(error) = server.emit(Signal::Seeked { position: at }).await {
                        tracing::warn!(%error, "cannot tell the desktop about the seek");
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AlbumRef, ArtistRef, ImageRef, TrackId};

    /// A track, for a test.
    fn track() -> Track {
        Track {
            id: TrackId("abc123".to_owned()),
            name: "Blinding Lights".to_owned(),
            artists: vec![ArtistRef {
                id: crate::models::ArtistId("art".to_owned()),
                name: "The Weeknd".to_owned(),
            }],
            album: Some(AlbumRef {
                id: crate::models::AlbumId("alb".to_owned()),
                name: "After Hours".to_owned(),
                images: vec![ImageRef {
                    url: "https://i.scdn.co/image/cover".to_owned(),
                    width: Some(640),
                    height: Some(640),
                }],
            }),
            duration: Duration::from_secs(200),
            explicit: false,
            playable: true,
            track_number: 9,
        }
    }

    #[test]
    fn a_position_report_changes_no_property() {
        // Twice a second, and a desktop woken by each one would be woken for nothing.
        let mut snapshot = Snapshot::default();
        let changed = snapshot.apply(&PlaybackEvent::Moved {
            position: Duration::from_secs(3),
        });

        assert!(changed.is_empty());
        assert_eq!(snapshot.position, Duration::from_secs(3));
    }

    #[test]
    fn a_new_track_changes_the_metadata() {
        let mut snapshot = Snapshot::default();
        let changed = snapshot.apply(&PlaybackEvent::TrackStarted {
            track: Box::new(track()),
            index: 0,
            queue_len: 3,
        });

        assert!(
            changed
                .iter()
                .any(|property| matches!(property, Property::Metadata(_)))
        );
        assert_eq!(snapshot.position, Duration::ZERO);
    }

    #[test]
    fn the_metadata_carries_what_a_desktop_shows() {
        let mut snapshot = Snapshot::default();
        snapshot.apply(&PlaybackEvent::TrackStarted {
            track: Box::new(track()),
            index: 0,
            queue_len: 1,
        });
        let metadata = snapshot.metadata();

        assert_eq!(metadata.title(), Some("Blinding Lights"));
        assert_eq!(metadata.album(), Some("After Hours"));
        assert_eq!(metadata.length(), Some(Time::from_secs(200)));
        assert_eq!(
            metadata.art_url().as_deref(),
            Some("https://i.scdn.co/image/cover"),
            "the desktop shows the cover"
        );
    }

    #[test]
    fn an_identifier_becomes_something_a_bus_accepts() {
        assert_eq!(sanitise("4uLU6hMCjMI75M1A2tKUQC"), "4uLU6hMCjMI75M1A2tKUQC");
        assert_eq!(sanitise("a-b.c"), "a_b_c");
    }
}
