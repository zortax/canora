//! What the desktop sees of the player.
//!
//! Each platform asks in its own way: Linux answers MPRIS on D-Bus, macOS fills a Now Playing
//! dictionary. The engine reports once, and the platform below decides what to do with it.
//!
//! Each one answers from a copy of what the engine last reported. A call that waited for the
//! audio thread would stall the desktop's own shell.

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
pub use linux::relay;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::relay;

/// Reports playback to the desktop.
///
/// The platforms with no integration do nothing. Dropping the receiver closes the channel, and the
/// caller already ignores a send that fails.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn relay(
    _runtime: &tokio::runtime::Handle,
    _player: crate::player::PlayerHandle,
    _events: tokio::sync::mpsc::UnboundedReceiver<crate::player::PlaybackEvent>,
) {
}
