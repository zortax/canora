//! Playing audio.
//!
//! The interface holds a [`PlayerHandle`] and sends instructions. One task owns the audio pipeline
//! and reports back through a callback. See [`engine`] for what the task does.

pub mod commands;
pub mod engine;
pub mod events;
pub mod queue;
pub mod state;

use librespot_core::cache::Cache;
use librespot_core::session::Session;

pub use crate::player::commands::{PlayerCommand, PlayerHandle};
pub use crate::player::events::PlaybackEvent;
pub use crate::player::queue::{PlayContext, RepeatMode};

/// What stopped the player from starting.
#[derive(Debug, thiserror::Error)]
pub enum PlayerError {
    /// This machine offers no way to play audio.
    #[error("no audio backend is available")]
    NoBackend,
    /// librespot offers no mixer.
    #[error("no mixer is available")]
    NoMixer,
    /// The mixer refused to start.
    #[error("cannot start the mixer: {0}")]
    Mixer(#[from] librespot_core::Error),
}

/// Starts the audio pipeline and the task that owns it.
///
/// `report` runs on the engine's task for every event. Keep it short: hand the event to the
/// interface thread and return.
pub fn spawn(
    session: Session,
    cache: Cache,
    runtime: &tokio::runtime::Handle,
    report: impl FnMut(PlaybackEvent) + Send + 'static,
) -> Result<PlayerHandle, PlayerError> {
    let (player, mixer, events) = engine::build(session, cache.clone())?;
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

    runtime.spawn(async move {
        engine::Engine::new(player, mixer, cache, report)
            .run(rx, events)
            .await;
    });

    Ok(PlayerHandle::new(tx))
}
