//! The streaming session, and how the application replaces it.
//!
//! librespot connects a session one time. When the connection to the access point stops, librespot
//! marks the session invalid, and the session stays invalid. The usual causes are a network fault,
//! a machine that went to sleep, an access point that closes the socket, and a keep-alive that
//! gets no answer.
//!
//! An invalid session gives no decryption keys. The audio pipeline then sends encrypted bytes to
//! the decoder. The decoder fails, and the track ends at the moment it starts. The queue continues
//! to the next track, which fails in the same way. A full queue can empty in one second.
//!
//! Tracks that played before show this fault most clearly. The disk cache holds the encrypted
//! file, so the file opens and only the key is absent.
//!
//! The repair has two parts. [`SessionCell`] holds the one session that all other code reads, so a
//! replacement reaches every reader at the same moment. [`supervise`] finds the dead session and
//! builds that replacement from the credentials on the disk.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use librespot_core::session::Session;

use crate::player::PlaybackEvent;
use crate::player::events::Connection;
use crate::services::Services;

/// The message for a poisoned lock.
///
/// The lock holds one handle. A panic while a thread holds it is a fault in the program.
const POISONED: &str = "the session lock was poisoned";

/// How often the watchdog looks at the connection.
const CHECK: Duration = Duration::from_secs(2);

/// How long the watchdog waits after a failed attempt.
const FIRST_WAIT: Duration = Duration::from_secs(2);

/// The longest wait between two attempts.
///
/// A machine that comes back from a week of sleep must find the network quickly.
const LONGEST_WAIT: Duration = Duration::from_secs(60);

/// The session that all other code reads.
///
/// Copy this cell freely. Every copy shows the same session, and every copy shows the session that
/// arrives after a reconnection. Read the session at the point of use. A private copy of
/// [`Session`] stays dead after one lost connection.
#[derive(Clone)]
pub struct SessionCell(Arc<RwLock<Session>>);

impl SessionCell {
    /// A cell that holds the session from the login.
    #[must_use]
    pub fn new(session: Session) -> Self {
        Self(Arc::new(RwLock::new(session)))
    }

    /// The session as it is now.
    #[must_use]
    pub fn get(&self) -> Session {
        self.0.read().expect(POISONED).clone()
    }

    /// Puts a new session in the place of the dead one.
    pub fn set(&self, session: Session) {
        *self.0.write().expect(POISONED) = session;
    }

    /// Whether librespot marked the session invalid.
    ///
    /// A session that never connected is valid, so this answer alone does not show a fault. See
    /// [`crate::auth::Standby::is_dead`].
    #[must_use]
    pub fn is_invalid(&self) -> bool {
        self.0.read().expect(POISONED).is_invalid()
    }
}

/// Watches the connection and replaces a session that died.
///
/// librespot does not report the death of a session, so this task asks for the state. Each
/// question costs one lock and one boolean, which is cheap enough to ask every two seconds while
/// the window is open.
///
/// Each failed attempt waits longer than the one before it, to a maximum of one minute. A laptop
/// with a closed lid has no network, and the task must be ready when the network comes back.
pub fn supervise(
    runtime: &tokio::runtime::Handle,
    services: Services,
    events: tokio::sync::mpsc::UnboundedSender<PlaybackEvent>,
) {
    runtime.spawn(async move {
        let mut wait = FIRST_WAIT;
        // Report each outage one time, at the first attempt.
        let mut reported = false;

        loop {
            tokio::time::sleep(CHECK).await;

            // The window is gone, and with it the reason to watch.
            if events.is_closed() {
                break;
            }

            if !services.standby.is_dead() {
                wait = FIRST_WAIT;
                reported = false;
                continue;
            }

            if !reported {
                tracing::warn!("the connection to Spotify is gone; building another session");
                let _ = events.send(PlaybackEvent::Connection(Connection::Lost));
                reported = true;
            }

            match services.reconnect().await {
                Ok(()) => {
                    tracing::info!("the connection to Spotify is back");
                    let _ = events.send(PlaybackEvent::Connection(Connection::Restored));
                    wait = FIRST_WAIT;
                    reported = false;
                }
                Err(error) => {
                    tracing::warn!(%error, ?wait, "cannot connect again yet");
                    tokio::time::sleep(wait).await;
                    wait = (wait * 2).min(LONGEST_WAIT);
                }
            }
        }
    });
}
