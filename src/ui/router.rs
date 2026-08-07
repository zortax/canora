//! Where the interface is, and how it goes back.
//!
//! One stack holds where it has been. Going back moves the top of that stack onto a second one, so
//! going forward puts it back.

use zgui::prelude::*;
use zgui::reactive::{LocalStorage, RwSignal};

use crate::models::{AlbumId, ArtistId, PlaylistId, TrackId};

/// What the main area shows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Route {
    /// The saved tracks.
    #[default]
    Liked,
    /// One playlist.
    Playlist(PlaylistId),
    /// One album.
    Album(AlbumId),
    /// One artist.
    Artist(ArtistId),
    /// A station built around one track.
    Radio(TrackId),
    /// What matches the search.
    Search,
}

/// Where the interface has been.
#[derive(Debug, Clone, Copy)]
pub struct Router {
    /// Where it is, with where it came from underneath.
    stack: RwSignal<Vec<Route>, LocalStorage>,
    /// Where it was before going back.
    forward: RwSignal<Vec<Route>, LocalStorage>,
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}

impl Router {
    /// A router showing the saved tracks.
    #[must_use]
    pub fn new() -> Self {
        Self {
            stack: RwSignal::new_local(vec![Route::default()]),
            forward: RwSignal::new_local(Vec::new()),
        }
    }

    /// Where the interface is.
    #[must_use]
    pub fn current(&self) -> Route {
        self.stack
            .with(|stack| stack.last().cloned())
            .unwrap_or_default()
    }

    /// Goes to `route`.
    ///
    /// Going where the interface already is does nothing, so a second click on the same playlist
    /// leaves the history alone.
    pub fn go(&self, route: Route) {
        if self.current() == route {
            return;
        }
        self.stack.update(|stack| stack.push(route));
        self.forward.update(Vec::clear);
    }

    /// Whether there is anywhere to go back to.
    #[must_use]
    pub fn can_go_back(&self) -> bool {
        self.stack.with(Vec::len) > 1
    }

    /// Whether there is anywhere to go forward to.
    #[must_use]
    pub fn can_go_forward(&self) -> bool {
        !self.forward.with(Vec::is_empty)
    }

    /// Goes back one step.
    pub fn back(&self) {
        if !self.can_go_back() {
            return;
        }
        if let Some(route) = self.stack.try_update(Vec::pop).flatten() {
            self.forward.update(|forward| forward.push(route));
        }
    }

    /// Goes forward one step.
    pub fn forward(&self) {
        if let Some(route) = self.forward.try_update(Vec::pop).flatten() {
            self.stack.update(|stack| stack.push(route));
        }
    }
}
