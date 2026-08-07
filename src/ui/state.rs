//! What the interface holds while it runs.
//!
//! The playlists, which tracks are saved, and the search text. Everything here is a signal, and
//! everything is written on the interface thread.
//!
//! A like is written to Spotify and to the set below at the same time. The set changes first, so
//! the heart fills under the pointer; a refusal puts it back.

use std::collections::HashSet;
use std::rc::Rc;

use zgui::prelude::*;
use zgui::reactive::{LocalStorage, RwSignal};

use crate::models::{Playlist, Track, TrackId};
use crate::services::Services;
use crate::ui::router::Router;

/// How many of Spotify's own playlists are read at once.
const MADE_FETCHES: usize = 6;

/// Whether the account is within reach.
///
/// The window opens on the cached library, so it is drawn before this says `Live`. A failure keeps
/// the library on the screen and puts a mark in the header.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Link {
    /// Connecting, behind whatever the window is already showing.
    #[default]
    Connecting,
    /// Connected.
    Live,
    /// Out of reach, with the reason.
    Broken(String),
}

/// What stopped the connection, in a few words.
///
/// The header has room for a line rather than a paragraph. The log keeps the whole thing.
fn plainly(error: &crate::services::StartError) -> String {
    match error {
        crate::services::StartError::Auth(_) => "Cannot sign in".to_owned(),
        crate::services::StartError::Api(_) => "Spotify is not answering".to_owned(),
        other => other.to_string(),
    }
}

/// What the interface holds.
#[derive(Clone, Copy)]
pub struct AppState {
    /// Every playlist, in the order Spotify lists them.
    pub playlists: RwSignal<Vec<Playlist>, LocalStorage>,
    /// How many tracks are saved.
    pub liked_total: RwSignal<u32, LocalStorage>,
    /// Which tracks are saved, of the ones the interface has asked about.
    pub liked: RwSignal<HashSet<TrackId>, LocalStorage>,
    /// Which tracks the interface has asked about.
    pub liked_known: RwSignal<HashSet<TrackId>, LocalStorage>,
    /// What is in the search field.
    pub query: RwSignal<String, LocalStorage>,
    /// Whether the library is being read again.
    pub syncing: RwSignal<bool, LocalStorage>,
    /// Whether the account is within reach.
    pub link: RwSignal<Link, LocalStorage>,
    /// Whether a connection attempt is running. One at a time.
    pub linking: RwSignal<bool, LocalStorage>,
    /// What the account is called.
    ///
    /// Seeded from the cache, so the header has a name before the account is read again.
    pub who: RwSignal<String, LocalStorage>,
    /// How the account arranges its playlists, and which folders are open.
    pub layout: RwSignal<Vec<crate::api::direct::Entry>, LocalStorage>,
    /// Which folders the sidebar shows the inside of.
    pub open_folders: RwSignal<HashSet<String>, LocalStorage>,
    /// The tracks the view is showing.
    ///
    /// One view is on the screen at a time, and it fills this. An edit takes a row out here, so
    /// the list answers at once rather than waiting for Spotify to agree with itself.
    pub tracks: RwSignal<std::rc::Rc<Vec<Track>>, LocalStorage>,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    /// A state with nothing loaded.
    #[must_use]
    pub fn new() -> Self {
        Self {
            playlists: RwSignal::new_local(Vec::new()),
            liked_total: RwSignal::new_local(0),
            liked: RwSignal::new_local(HashSet::new()),
            liked_known: RwSignal::new_local(HashSet::new()),
            query: RwSignal::new_local(String::new()),
            syncing: RwSignal::new_local(false),
            link: RwSignal::new_local(Link::default()),
            linking: RwSignal::new_local(false),
            who: RwSignal::new_local(String::new()),
            layout: RwSignal::new_local(Vec::new()),
            open_folders: RwSignal::new_local(HashSet::new()),
            tracks: RwSignal::new_local(std::rc::Rc::new(Vec::new())),
        }
    }

    /// Connects the session and reads the account, behind whatever the window is showing.
    ///
    /// This is both the first attempt and the retry the header offers. A failure leaves the cached
    /// library on the screen and puts a mark in the header.
    pub fn link_up(&self, services: &Services) {
        if self.linking.get_untracked() {
            return;
        }
        self.linking.set(true);
        self.link.set(Link::Connecting);

        let state = *self;
        let services = services.clone();
        zgui::reactive::spawn_detached(async move {
            let result = services
                .link(|phase| tracing::info!(?phase, "login phase"))
                .await;
            state.linking.set(false);
            match result {
                Ok(me) => {
                    state.who.set(me.display_name);
                    state.link.set(Link::Live);
                }
                Err(error) => {
                    tracing::warn!(%error, "cannot reach the account");
                    state.link.set(Link::Broken(plainly(&error)));
                }
            }
        });
    }

    /// Reads the library again, from Spotify rather than from the cache.
    pub fn sync(&self, services: &Services) {
        if self.syncing.get_untracked() {
            return;
        }
        self.syncing.set(true);

        let state = *self;
        let services = services.clone();
        zgui::reactive::spawn_detached(async move {
            // Everything cached is due again, so the next view read goes to Spotify.
            services.cache.stale().await;
            match services.api.my_playlists().await {
                Ok(playlists) => {
                    services.cache.put_playlists(&playlists).await;
                    state.playlists.set(playlists);
                }
                Err(error) => tracing::warn!(%error, "cannot read the playlists"),
            }
            let layout = services.rootlist().await;
            if !layout.is_empty() {
                services.cache.put_layout(&layout).await;
                state.layout.set(layout);
            }
            state.load_made_playlists(&services).await;
            match services.api.saved_tracks(0).await {
                Ok(page) => state.liked_total.set(page.total),
                Err(error) => tracing::warn!(%error, "cannot count the saved tracks"),
            }
            // Ask about every heart again.
            state.liked_known.update(std::collections::HashSet::clear);
            state.syncing.set(false);
        });
    }

    /// The playlist the sidebar already holds, when it holds it.
    ///
    /// The sidebar reads every playlist at the start, so opening one needs no request of its own.
    #[must_use]
    pub fn known_playlist(&self, id: &crate::models::PlaylistId) -> Option<Playlist> {
        self.playlists.with_untracked(|playlists| {
            playlists
                .iter()
                .find(|it| it.id.as_ref() == Some(id))
                .cloned()
        })
    }

    /// Whether `id` is saved.
    #[must_use]
    pub fn is_liked(&self, id: &TrackId) -> bool {
        self.liked.with(|liked| liked.contains(id))
    }

    /// Fills the window from the cache. Asks Spotify nothing.
    ///
    /// This is what the window draws on before it has connected. Everything here is a read from
    /// the local copy, so it finishes in milliseconds and works with no network at all.
    pub fn show_cached(&self, services: &Services) {
        let state = *self;
        let services = services.clone();
        spawn_local(async move {
            if let Some(me) = services.cache.profile().await {
                state.who.set(me.display_name);
            }
            // Both are read before either is written, and the arrangement is written first. The
            // sidebar reads the two together: with playlists and no arrangement it draws a flat
            // list, so writing them the other way round shows the whole library unfoldered for a
            // frame and then folds it.
            let cached = services.cache.playlists().await;
            if !cached.is_empty() {
                let layout = services.cache.layout().await;
                state.layout.set(layout);
                state.playlists.set(cached);
            }
            // The hearts fill from the cache, so a list drawn from it is already right.
            let saved = services.cache.saved().await;
            if !saved.is_empty() {
                state.liked_total.set(u32::try_from(saved.len()).unwrap_or(u32::MAX));
                state.liked.update(|liked| liked.extend(saved.iter().cloned()));
                state.liked_known.update(|known| known.extend(saved));
            }
        });
    }

    /// Reads the library from Spotify, behind whatever the window is showing.
    ///
    /// Called once the session is connected. A recent reading is left alone: the sidebar changes
    /// rarely, and reading it again costs a request per page.
    pub fn refresh(&self, services: &Services) {
        let state = *self;
        let services = services.clone();
        spawn_local(async move {
            let fresh = services
                .cache
                .read_within(crate::cache::LIBRARY, crate::cache::FRESH)
                .await
                && !state.playlists.get_untracked().is_empty();

            if !fresh {
                match services.api.my_playlists().await {
                    Ok(playlists) => {
                        services.cache.put_playlists(&playlists).await;
                        state.playlists.set(playlists);
                    }
                    Err(error) => tracing::warn!(%error, "cannot read the playlists"),
                }
            }

            // The folders come from the session: the Web API does not know about them.
            if !fresh || state.layout.get_untracked().is_empty() {
                let layout = services.rootlist().await;
                if !layout.is_empty() {
                    services.cache.put_layout(&layout).await;
                    state.layout.set(layout);
                    state.load_made_playlists(&services).await;
                }
            }

            match services.api.saved_tracks(0).await {
                Ok(page) => state.liked_total.set(page.total),
                Err(error) => tracing::warn!(%error, "cannot count the saved tracks"),
            }
        });
    }

    /// Reads the playlists Spotify makes and the Web API leaves out.
    ///
    /// Discover Weekly, Release Radar, the Daily Mixes and the rest are in the arrangement and
    /// absent from `/me/playlists`. The session describes every one of them.
    async fn load_made_playlists(&self, services: &Services) {
        use crate::api::direct::Entry;
        use futures_util::stream::{self, StreamExt as _};

        let known: HashSet<_> = self
            .playlists
            .with_untracked(|playlists| playlists.iter().filter_map(|it| it.id.clone()).collect());

        let missing: Vec<_> = self.layout.with_untracked(|layout| {
            layout
                .iter()
                .flat_map(|entry| match entry {
                    Entry::Playlist(id) => std::slice::from_ref(id),
                    Entry::Folder { playlists, .. } => playlists.as_slice(),
                })
                .filter(|id| !known.contains(id))
                .cloned()
                .collect()
        });
        if missing.is_empty() {
            return;
        }

        let found: Vec<Playlist> = stream::iter(
            missing
                .iter()
                .map(|id| async move { services.api.made_playlist(id).await }),
        )
        .buffered(MADE_FETCHES)
        .filter_map(std::future::ready)
        .collect()
        .await;

        if found.is_empty() {
            return;
        }
        self.playlists.update(|playlists| playlists.extend(found));
        services
            .cache
            .put_playlists(&self.playlists.get_untracked())
            .await;
    }

    /// The sidebar's rows: the playlists, in folders where the account puts them.
    ///
    /// A playlist the arrangement does not mention still shows, at the end. The two lists come
    /// from different places and one can be newer than the other.
    #[must_use]
    pub fn library(&self) -> Vec<crate::models::LibraryNode> {
        use crate::api::direct::Entry;
        use crate::models::LibraryNode;

        let playlists = self.playlists.get();
        let layout = self.layout.get();
        if layout.is_empty() {
            return playlists.into_iter().map(LibraryNode::Playlist).collect();
        }

        let by_id: std::collections::HashMap<_, _> = playlists
            .iter()
            .filter_map(|playlist| Some((playlist.id.clone()?, playlist.clone())))
            .collect();
        let mut placed = HashSet::new();
        let mut nodes = Vec::new();

        for entry in &layout {
            match entry {
                Entry::Playlist(id) => {
                    if let Some(playlist) = by_id.get(id) {
                        placed.insert(id.clone());
                        nodes.push(LibraryNode::Playlist(playlist.clone()));
                    }
                }
                Entry::Folder {
                    id,
                    name,
                    playlists: inside,
                } => {
                    let held: Vec<_> = inside
                        .iter()
                        .filter_map(|id| {
                            let playlist = by_id.get(id)?;
                            placed.insert(id.clone());
                            Some(playlist.clone())
                        })
                        .collect();
                    if !held.is_empty() {
                        nodes.push(LibraryNode::Folder {
                            id: id.clone(),
                            name: name.clone(),
                            playlists: held,
                        });
                    }
                }
            }
        }

        // Anything the arrangement has not caught up with.
        for playlist in playlists {
            if playlist
                .id
                .as_ref()
                .is_some_and(|id| !placed.contains(id))
            {
                nodes.push(LibraryNode::Playlist(playlist));
            }
        }
        nodes
    }

    /// Opens or closes a folder.
    pub fn toggle_folder(&self, id: &str) {
        self.open_folders.update(|open| {
            if !open.remove(id) {
                open.insert(id.to_owned());
            }
        });
    }

    /// Asks Spotify which of `tracks` are saved.
    ///
    /// Skip the ones already asked about. A list that scrolls asks about each row once.
    pub fn load_liked_for(&self, services: &Services, tracks: &[Track]) {
        let wanted: Vec<TrackId> = self.liked_known.with(|known| {
            tracks
                .iter()
                .map(|track| track.id.clone())
                .filter(|id| !known.contains(id))
                .collect()
        });
        if wanted.is_empty() {
            return;
        }

        self.liked_known.update(|known| {
            known.extend(wanted.iter().cloned());
        });

        let state = *self;
        let api = services.api.clone();
        spawn_local(async move {
            match api.saved_contains(&wanted).await {
                Ok(answers) => state.liked.update(|liked| {
                    for (id, saved) in wanted.iter().zip(answers) {
                        if saved {
                            liked.insert(id.clone());
                        } else {
                            liked.remove(id);
                        }
                    }
                }),
                Err(error) => {
                    tracing::warn!(%error, "cannot read which tracks are saved");
                    // Ask again next time this row is drawn.
                    state.liked_known.update(|known| {
                        for id in &wanted {
                            known.remove(id);
                        }
                    });
                }
            }
        });
    }

    /// Saves or unsaves a track.
    ///
    /// Change the set first, so the heart answers the click. Put it back if Spotify refuses.
    pub fn set_liked(&self, services: &Services, id: TrackId, on: bool) {
        let state = *self;
        self.liked.update(|liked| {
            if on {
                liked.insert(id.clone());
            } else {
                liked.remove(&id);
            }
        });
        self.liked_known.update(|known| {
            known.insert(id.clone());
        });
        self.liked_total.update(|total| {
            if on {
                *total += 1;
            } else {
                *total = total.saturating_sub(1);
            }
        });

        let services = services.clone();
        let ids = vec![id.clone()];
        zgui::reactive::spawn_detached(async move {
            let result = if on {
                services.api.save_tracks(&ids).await
            } else {
                services.api.unsave_tracks(&ids).await
            };
            if result.is_ok() {
                services.cache.set_saved(&id, on).await;
            }
            if let Err(error) = result {
                tracing::warn!(%error, %id, "cannot change the saved tracks");
                // Spotify said no. Show what Spotify holds.
                state.liked.update(|liked| {
                    if on {
                        liked.remove(&id);
                    } else {
                        liked.insert(id.clone());
                    }
                });
                state.liked_total.update(|total| {
                    if on {
                        *total = total.saturating_sub(1);
                    } else {
                        *total += 1;
                    }
                });
            }
        });
    }
}

/// Everything a component reaches through context.
#[derive(Clone)]
pub struct Ui {
    /// What the interface talks to.
    pub services: Services,
    /// What it holds.
    pub state: AppState,
    /// Where it is.
    pub router: Router,
    /// What the player is doing.
    pub playback: crate::player::state::PlaybackStore,
}

/// The context a component reads.
///
/// Every view needs this. The root provides it before it draws anything, so a component that
/// cannot find it is a fault in this application rather than a state to handle.
#[must_use]
pub fn ui() -> Rc<Ui> {
    use_local_context::<Rc<Ui>>().expect("the interface context is provided at the root")
}
