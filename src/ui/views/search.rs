//! What matches the search.
//!
//! The field writes on every keystroke. The request waits a moment after the last one, so typing a
//! word costs one search rather than one per letter.

use std::rc::Rc;
use std::time::Duration;

use zgui::prelude::*;
use zgui::reactive::{RenderEffect, RwSignal};
use zgui::{component, view};
use zgui_ui::prelude::*;

use crate::models::SearchResults;
use crate::player::PlayContext;
use crate::ui::art::ArtProps;
use crate::ui::format;
use crate::ui::router::Route;
use crate::ui::Erase;
use crate::ui::state::ui;
use crate::ui::views::WaitingProps;
use crate::ui::track::TracklistProps;
use crate::ui::views::artist::AlbumGridProps;

/// How long the field rests before the search goes out.
const SETTLE: Duration = Duration::from_millis(250);

/// What matches the search.
#[component]
pub fn SearchView() -> impl IntoView {
    let app = ui();
    let state = app.state;
    let results = RwSignal::new_local(SearchResults::default());
    let searching = RwSignal::new_local(false);

    // Search when the field settles. A keystroke during the wait replaces the request.
    let watching = RenderEffect::new({
        let app = app.clone();
        move |_| {
            let query = state.query.get().trim().to_owned();
            if query.is_empty() {
                results.set(SearchResults::default());
                searching.set(false);
                return;
            }

            searching.set(true);
            let app = app.clone();
            spawn_local(async move {
                tokio::time::sleep(SETTLE).await;
                // A newer keystroke owns the field now. Drop this one.
                if state.query.get_untracked().trim() != query {
                    return;
                }
                match app.services.api.search(&query).await {
                    Ok(found) => {
                        app.state.load_liked_for(&app.services, &found.tracks);
                        if state.query.get_untracked().trim() == query {
                            results.set(found);
                        }
                    }
                    Err(error) => tracing::warn!(%error, "cannot search"),
                }
                searching.set(false);
            });
        }
    });
    on_cleanup_local(move || drop(watching));

    let tracks = Signal::derive_local(move || results.with(|it| Rc::new(it.tracks.clone())));
    let albums = Signal::derive_local(move || results.with(|it| Rc::new(it.albums.clone())));
    let empty = Signal::derive_local(move || results.with(SearchResults::is_empty));
    let asked = Signal::derive_local(move || !state.query.get().trim().is_empty());

    view! {
        column(class = "view view-pad") {
            {move || {
                if !asked.get() {
                    return view! {
                        Empty {
                            EmptyTitle {"Search"}
                            EmptyDescription {"Find songs, artists and albums."}
                        }
                    }
                    .any();
                }
                if empty.get() {
                    return if searching.get() {
                        view! { Waiting() }.any()
                    } else {
                        view! {
                            Empty {
                                EmptyTitle {"Nothing found"}
                                EmptyDescription {"Try another spelling."}
                            }
                        }
                        .any()
                    };
                }

                view! {
                    Tabs(default_value = "songs", label = "Results") {
                        TabsList(variant = TabsListVariant::Line) {
                            TabsTrigger(value = "songs") {"Songs"}
                            TabsTrigger(value = "artists") {"Artists"}
                            TabsTrigger(value = "albums") {"Albums"}
                            TabsTrigger(value = "playlists") {"Playlists"}
                        }
                        TabsContent(value = "songs") {
                            box(class = "sect__top") {
                                Tracklist(
                                    tracks = tracks,
                                    context = Signal::stored_local(PlayContext::Adhoc)
                                )
                            }
                        }
                        TabsContent(value = "artists") {
                            ArtistList(results = results)
                        }
                        TabsContent(value = "albums") {
                            AlbumGrid(albums = albums)
                        }
                        TabsContent(value = "playlists") {
                            PlaylistList(results = results)
                        }
                    }
                }
                .any()
            }}
        }
    }
}

/// The artists that matched.
#[component]
fn ArtistList(
    /// What matched.
    results: RwSignal<SearchResults, zgui::reactive::LocalStorage>,
) -> impl IntoView {
    let router = ui().router;

    view! {
        scroll(class = "grid") {
            {move || {
                results
                    .with(|results| results.artists.clone())
                    .into_iter()
                    .map(|artist| {
                        let id = artist.id.clone();
                        let name = format::truncate(&artist.name, format::CARD).into_owned();
                        view! {
                            control(
                                class = "card",
                                tabindex = Focus::Sequential,
                                on:click = move |_| router.go(Route::Artist(id.clone()))
                            ) {
                                Art(
                                    images = Signal::stored_local(artist.images.clone()),
                                    class = "art art-card art-round",
                                    want = 300
                                )
                                text(class = "card__title") {{name}}
                                text(class = "card__note") {"Artist"}
                            }
                        }
                        .any()
                    })
                    .collect::<Vec<_>>()
            }}
        }
    }
}

/// The playlists that matched.
#[component]
fn PlaylistList(
    /// What matched.
    results: RwSignal<SearchResults, zgui::reactive::LocalStorage>,
) -> impl IntoView {
    let router = ui().router;

    view! {
        scroll(class = "grid") {
            {move || {
                results
                    .with(|results| results.playlists.clone())
                    .into_iter()
                    .filter_map(|playlist| {
                        let id = playlist.id.clone()?;
                        let name = format::truncate(&playlist.name, format::CARD).into_owned();
                        Some(
                            view! {
                                control(
                                    class = "card",
                                    tabindex = Focus::Sequential,
                                    on:click = move |_| router.go(Route::Playlist(id.clone()))
                                ) {
                                    Art(
                                        images = Signal::stored_local(playlist.images.clone()),
                                        class = "art art-card",
                                        want = 300
                                    )
                                    text(class = "card__title") {{name}}
                                    text(class = "card__note") {"Playlist"}
                                }
                            }
                            .any(),
                        )
                    })
                    .collect::<Vec<_>>()
            }}
        }
    }
}
