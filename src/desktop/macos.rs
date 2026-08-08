//! What the desktop sees of the player, on macOS.
//!
//! `MPNowPlayingInfoCenter` holds what plays, and `MPRemoteCommandCenter` gives the commands the
//! system calls back on. Together they fill the Now Playing panel and receive the media keys.
//!
//! Two conditions apply on macOS. The program sets the playback state itself, and the panel shows
//! only a bundled application. See `README.md`.
//!
//! MediaPlayer belongs to the main thread. Each report becomes plain data and goes to the main
//! queue, so no Objective-C object crosses an await.

use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use block2::RcBlock;
use objc2::AllocAnyThread;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2_app_kit::NSImage;
use objc2_core_foundation::CGSize;
use objc2_foundation::{NSData, NSMutableDictionary, NSNumber, NSString};
use objc2_media_player::{
    MPChangePlaybackPositionCommandEvent, MPMediaItemArtwork, MPMediaItemPropertyAlbumTitle,
    MPMediaItemPropertyArtist, MPMediaItemPropertyArtwork, MPMediaItemPropertyPlaybackDuration,
    MPMediaItemPropertyTitle, MPNowPlayingInfoCenter, MPNowPlayingInfoPropertyElapsedPlaybackTime,
    MPNowPlayingInfoPropertyPlaybackRate, MPNowPlayingPlaybackState, MPRemoteCommandCenter,
    MPRemoteCommandEvent, MPRemoteCommandHandlerStatus,
};

use crate::models::Track;
use crate::player::state::PlayStatus;
use crate::player::{PlaybackEvent, PlayerHandle};

/// How far the position must move to count as a seek.
///
/// The system carries the bar forward from an elapsed time and a rate, so routine reports stay
/// silent. Seeks arrive on the same event, and their size tells them apart.
const JUMP: Duration = Duration::from_millis(1500);

/// The cover size to ask Spotify for. Control Center draws it small.
const COVER: u32 = 640;

/// How long to wait for a cover.
const COVER_TIMEOUT: Duration = Duration::from_secs(10);

/// What the engine last reported.
#[derive(Debug, Clone, Default)]
struct Snapshot {
    /// The track that is playing.
    track: Option<Track>,
    /// What the player is doing.
    status: PlayStatus,
    /// Where the track is.
    position: Duration,
}

/// What the system is told, in a shape that crosses a thread.
///
/// `show` turns this into Objective-C objects on the main queue.
#[derive(Debug, Clone)]
struct NowPlaying {
    /// The name of the track, or nothing when there is no track.
    title: Option<String>,
    /// Every artist on it, in one line.
    artist: String,
    /// The album it came from.
    album: String,
    /// How long it runs.
    duration: Duration,
    /// How far in it is.
    elapsed: Duration,
    /// The rate the system carries the bar at: one while audio runs, zero while it is held.
    rate: f64,
    /// What the panel says the player is doing.
    state: MPNowPlayingPlaybackState,
    /// The cover, as the bytes came off the network.
    artwork: Option<Arc<[u8]>>,
}

impl Default for NowPlaying {
    /// Nothing playing. `MPNowPlayingPlaybackState` has no `Default`.
    fn default() -> Self {
        Self {
            title: None,
            artist: String::new(),
            album: String::new(),
            duration: Duration::ZERO,
            elapsed: Duration::ZERO,
            rate: 0.0,
            state: MPNowPlayingPlaybackState::Stopped,
            artwork: None,
        }
    }
}

impl Snapshot {
    /// Writes one event, and says whether the system needs telling.
    ///
    /// Position reports arrive twice a second. Most of them leave the panel as it stands.
    fn apply(&mut self, event: &PlaybackEvent) -> bool {
        match event {
            PlaybackEvent::TrackStarted { track, .. } => {
                self.track = Some((**track).clone());
                self.position = Duration::ZERO;
                true
            }
            PlaybackEvent::Playing { position } => {
                self.status = PlayStatus::Playing;
                self.position = *position;
                true
            }
            PlaybackEvent::Paused { position } => {
                self.status = PlayStatus::Paused;
                self.position = *position;
                true
            }
            PlaybackEvent::Moved { position } => {
                // Report a step the system cannot make on its own.
                let jumped = position.abs_diff(self.position) > JUMP;
                self.position = *position;
                jumped
            }
            PlaybackEvent::Stopped => {
                self.status = PlayStatus::Stopped;
                self.position = Duration::ZERO;
                true
            }
            PlaybackEvent::Loading => {
                self.status = PlayStatus::Loading;
                false
            }
            // The panel shows none of these.
            PlaybackEvent::Unavailable { .. }
            | PlaybackEvent::VolumeChanged { .. }
            | PlaybackEvent::ShuffleChanged(_)
            | PlaybackEvent::RepeatChanged(_)
            | PlaybackEvent::QueueChanged { .. }
            | PlaybackEvent::Connection(_) => false,
        }
    }

    /// The address of the cover for the track that is playing.
    fn cover(&self) -> Option<&str> {
        let album = self.track.as_ref()?.album.as_ref()?;
        Some(crate::models::pick_image(&album.images, COVER)?.url.as_str())
    }

    /// What the system is told, with `artwork` for the cover.
    fn published(&self, artwork: Option<Arc<[u8]>>) -> NowPlaying {
        let Some(track) = &self.track else {
            return NowPlaying::default();
        };

        NowPlaying {
            title: Some(track.name.clone()),
            artist: track.artist_line(),
            album: track
                .album
                .as_ref()
                .map(|album| album.name.clone())
                .unwrap_or_default(),
            duration: track.duration,
            elapsed: self.position,
            // A rate of zero holds the bar where it stands.
            rate: if self.status.is_playing() { 1.0 } else { 0.0 },
            state: match self.status {
                PlayStatus::Playing => MPNowPlayingPlaybackState::Playing,
                PlayStatus::Paused | PlayStatus::Loading => MPNowPlayingPlaybackState::Paused,
                PlayStatus::Stopped => MPNowPlayingPlaybackState::Stopped,
            },
            artwork,
        }
    }
}

/// Tells the system what plays. Runs on the main queue.
fn show(info: &NowPlaying) {
    // Safety: MediaPlayer runs on the main thread, and this call owns every object it passes.
    unsafe {
        let center = MPNowPlayingInfoCenter::defaultCenter();

        let Some(title) = &info.title else {
            // Nothing plays. An empty dictionary clears the panel.
            center.setNowPlayingInfo(None);
            center.setPlaybackState(MPNowPlayingPlaybackState::Stopped);
            return;
        };

        let dictionary = NSMutableDictionary::<NSString, AnyObject>::new();
        // The keys are MediaPlayer's own strings, and the dictionary copies them.
        let set = |key: &NSString, value: &AnyObject| {
            dictionary.setObject_forKey(value, ProtocolObject::from_ref(key));
        };

        set(MPMediaItemPropertyTitle, &NSString::from_str(title));
        set(MPMediaItemPropertyArtist, &NSString::from_str(&info.artist));
        set(MPMediaItemPropertyAlbumTitle, &NSString::from_str(&info.album));
        set(
            MPMediaItemPropertyPlaybackDuration,
            &NSNumber::new_f64(info.duration.as_secs_f64()),
        );
        set(
            MPNowPlayingInfoPropertyElapsedPlaybackTime,
            &NSNumber::new_f64(info.elapsed.as_secs_f64()),
        );
        set(
            MPNowPlayingInfoPropertyPlaybackRate,
            &NSNumber::new_f64(info.rate),
        );

        if let Some(bytes) = &info.artwork
            && let Some(artwork) = artwork(bytes)
        {
            set(MPMediaItemPropertyArtwork, &artwork);
        }

        center.setNowPlayingInfo(Some(&dictionary));
        center.setPlaybackState(info.state);
    }
}

/// The cover, in the shape MediaPlayer takes.
///
/// The system requests the image through a block, which owns it and lives as long as the artwork.
fn artwork(bytes: &[u8]) -> Option<Retained<MPMediaItemArtwork>> {
    // Safety: `NSData` copies the slice, and the image reads that copy.
    unsafe {
        let data = NSData::with_bytes(bytes);
        let image = NSImage::initWithData(NSImage::alloc(), &data)?;
        let size = image.size();

        let handler = RcBlock::new(move |_: CGSize| NonNull::from(&*image));
        Some(MPMediaItemArtwork::initWithBoundsSize_requestHandler(
            MPMediaItemArtwork::alloc(),
            size,
            &handler,
        ))
    }
}

/// Registers the commands the system calls back on.
///
/// Runs once, on the main queue. Each block holds a copy of the handle.
fn commands(player: PlayerHandle, playing: Arc<AtomicBool>) {
    /// The answer for a command the player accepted.
    fn took() -> MPRemoteCommandHandlerStatus {
        MPRemoteCommandHandlerStatus::Success
    }

    // Safety: this runs on the main thread, and the centre retains each block.
    unsafe {
        let centre = MPRemoteCommandCenter::sharedCommandCenter();

        // The panel greys out a disabled command. The last one has a class of its own.
        for command in [
            centre.playCommand(),
            centre.pauseCommand(),
            centre.togglePlayPauseCommand(),
            centre.nextTrackCommand(),
            centre.previousTrackCommand(),
        ] {
            command.setEnabled(true);
        }
        centre.changePlaybackPositionCommand().setEnabled(true);

        // The panel sends play and pause as separate commands. The engine only toggles, so each
        // block reads the state first.
        let handle = player.clone();
        let state = playing.clone();
        centre
            .playCommand()
            .addTargetWithHandler(&RcBlock::new(move |_: NonNull<MPRemoteCommandEvent>| {
                if !state.load(Ordering::Relaxed) {
                    handle.toggle_play();
                }
                took()
            }));

        let handle = player.clone();
        let state = playing;
        centre
            .pauseCommand()
            .addTargetWithHandler(&RcBlock::new(move |_: NonNull<MPRemoteCommandEvent>| {
                if state.load(Ordering::Relaxed) {
                    handle.toggle_play();
                }
                took()
            }));

        let handle = player.clone();
        centre.togglePlayPauseCommand().addTargetWithHandler(&RcBlock::new(
            move |_: NonNull<MPRemoteCommandEvent>| {
                handle.toggle_play();
                took()
            },
        ));

        let handle = player.clone();
        centre
            .nextTrackCommand()
            .addTargetWithHandler(&RcBlock::new(move |_: NonNull<MPRemoteCommandEvent>| {
                handle.next();
                took()
            }));

        let handle = player.clone();
        centre.previousTrackCommand().addTargetWithHandler(&RcBlock::new(
            move |_: NonNull<MPRemoteCommandEvent>| {
                handle.previous();
                took()
            },
        ));

        // The panel sends the position its bar was dragged to.
        centre.changePlaybackPositionCommand().addTargetWithHandler(&RcBlock::new(
            move |event: NonNull<MPRemoteCommandEvent>| {
                let seconds = event
                    .cast::<MPChangePlaybackPositionCommandEvent>()
                    .as_ref()
                    .positionTime();
                if seconds.is_finite() && seconds >= 0.0 {
                    player.seek(Duration::from_secs_f64(seconds));
                    took()
                } else {
                    MPRemoteCommandHandlerStatus::CommandFailed
                }
            },
        ));
    }
}

/// Tells the desktop about the player, and takes what it asks for.
///
/// Reads the engine's events from `rx`. A failure here leaves the panel empty and stops nothing.
pub fn relay(
    runtime: &tokio::runtime::Handle,
    player: PlayerHandle,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<PlaybackEvent>,
) {
    // What the play and pause blocks read.
    let playing = Arc::new(AtomicBool::new(false));

    let handle = player.clone();
    let state = playing.clone();
    dispatch2::DispatchQueue::main().exec_async(move || commands(handle, state));
    tracing::info!("Now Playing is up");

    // The interface caches covers as registrations. This needs the bytes, and one cover per track
    // is cheap to fetch again.
    let covers = reqwest::Client::builder()
        .user_agent(concat!("canora/", env!("CARGO_PKG_VERSION")))
        .timeout(COVER_TIMEOUT)
        .build()
        .unwrap_or_default();

    runtime.spawn(async move {
        let mut snapshot = Snapshot::default();
        // The cover for the track that plays, and the address it came from.
        let mut artwork: Option<Arc<[u8]>> = None;
        let mut showing: Option<String> = None;
        // A cover arrives long after its track, so it returns on its own channel and holds up
        // nothing.
        let (art_tx, mut art_rx) = tokio::sync::mpsc::unbounded_channel::<(String, Arc<[u8]>)>();

        loop {
            tokio::select! {
                event = rx.recv() => {
                    let Some(event) = event else { break };

                    // A new track drops the old cover at once.
                    if matches!(event, PlaybackEvent::TrackStarted { .. }) {
                        artwork = None;
                    }
                    let told = snapshot.apply(&event);
                    playing.store(snapshot.status.is_playing(), Ordering::Relaxed);
                    if !told {
                        continue;
                    }

                    // Ask for the cover once per track.
                    let wanted = snapshot.cover().map(str::to_owned);
                    if wanted != showing {
                        showing.clone_from(&wanted);
                        if let Some(url) = wanted {
                            let covers = covers.clone();
                            let art_tx = art_tx.clone();
                            tokio::spawn(async move {
                                if let Some(bytes) = fetch(&covers, &url).await {
                                    let _ = art_tx.send((url, bytes));
                                }
                            });
                        }
                    }

                    let info = snapshot.published(artwork.clone());
                    dispatch2::DispatchQueue::main().exec_async(move || show(&info));
                }
                cover = art_rx.recv() => {
                    let Some((url, bytes)) = cover else { continue };
                    // The track can move on while the picture is on its way.
                    if showing.as_deref() != Some(url.as_str()) {
                        continue;
                    }
                    artwork = Some(bytes);

                    let info = snapshot.published(artwork.clone());
                    dispatch2::DispatchQueue::main().exec_async(move || show(&info));
                }
            }
        }

        // The window is gone. Clear the panel.
        dispatch2::DispatchQueue::main().exec_async(|| show(&NowPlaying::default()));
    });
}

/// Fetches one cover.
async fn fetch(client: &reqwest::Client, url: &str) -> Option<Arc<[u8]>> {
    match client.get(url).send().await {
        Ok(response) => match response.error_for_status() {
            Ok(response) => match response.bytes().await {
                Ok(bytes) => Some(Arc::from(bytes.as_ref())),
                Err(error) => {
                    tracing::debug!(%error, %url, "the cover stopped short");
                    None
                }
            },
            Err(error) => {
                tracing::debug!(%error, %url, "the cover was refused");
                None
            }
        },
        Err(error) => {
            tracing::debug!(%error, %url, "cannot fetch the cover");
            None
        }
    }
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

    /// A snapshot with the track playing from the start.
    fn playing() -> Snapshot {
        let mut snapshot = Snapshot::default();
        snapshot.apply(&PlaybackEvent::TrackStarted {
            track: Box::new(track()),
            index: 0,
            queue_len: 3,
        });
        snapshot.apply(&PlaybackEvent::Playing {
            position: Duration::ZERO,
        });
        snapshot
    }

    #[test]
    fn a_position_report_tells_the_system_nothing() {
        // The system already carries the bar forward on its own.
        let mut snapshot = playing();
        let told = snapshot.apply(&PlaybackEvent::Moved {
            position: Duration::from_millis(500),
        });

        assert!(!told);
        assert_eq!(snapshot.position, Duration::from_millis(500));
    }

    #[test]
    fn a_seek_does() {
        let mut snapshot = playing();
        let told = snapshot.apply(&PlaybackEvent::Moved {
            position: Duration::from_secs(60),
        });

        assert!(told, "a jump of a minute is beyond the system's arithmetic");
        assert_eq!(snapshot.position, Duration::from_secs(60));
    }

    #[test]
    fn a_seek_backwards_does_too() {
        // `abs_diff` catches a step in both directions.
        let mut snapshot = playing();
        snapshot.apply(&PlaybackEvent::Moved {
            position: Duration::from_secs(60),
        });
        let told = snapshot.apply(&PlaybackEvent::Moved {
            position: Duration::from_secs(2),
        });

        assert!(told);
    }

    #[test]
    fn a_new_track_is_worth_telling() {
        let mut snapshot = Snapshot::default();
        let told = snapshot.apply(&PlaybackEvent::TrackStarted {
            track: Box::new(track()),
            index: 0,
            queue_len: 3,
        });

        assert!(told);
        assert_eq!(snapshot.position, Duration::ZERO);
    }

    #[test]
    fn what_the_panel_shows() {
        let info = playing().published(None);

        assert_eq!(info.title.as_deref(), Some("Blinding Lights"));
        assert_eq!(info.artist, "The Weeknd");
        assert_eq!(info.album, "After Hours");
        assert_eq!(info.duration, Duration::from_secs(200));
        assert_eq!(info.rate, 1.0, "the bar runs while the track does");
    }

    #[test]
    fn a_held_track_stops_the_bar() {
        let mut snapshot = playing();
        snapshot.apply(&PlaybackEvent::Paused {
            position: Duration::from_secs(30),
        });
        let info = snapshot.published(None);

        assert_eq!(info.rate, 0.0);
        assert_eq!(info.elapsed, Duration::from_secs(30));
    }

    #[test]
    fn nothing_playing_carries_no_title() {
        let info = Snapshot::default().published(None);

        assert!(info.title.is_none(), "the panel clears");
    }

    #[test]
    fn the_cover_is_the_one_the_panel_has_a_use_for() {
        assert_eq!(playing().cover(), Some("https://i.scdn.co/image/cover"));
    }
}
