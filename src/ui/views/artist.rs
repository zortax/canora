//! One artist: what they are best known for, and what they released.
//!
//! Spotify closed the top-tracks endpoint to applications registered after 2024. The list is empty
//! when it refuses, and the albums below carry the page.

use std::rc::Rc;

use zgui::prelude::*;
use zgui::reactive::RwSignal;
use zgui::{component, view};
use zgui_ui::prelude::*;

use crate::models::{Album, Artist, ArtistId, Track};
use crate::player::PlayContext;
use crate::ui::art::ArtProps;
use crate::ui::icons::{IconProps, IconSize, art};
use crate::ui::router::Route;
use crate::ui::Erase;
use crate::ui::state::ui;
use crate::ui::track::TracklistProps;
use crate::ui::views::header::HeaderProps;

/// How many releases the grid holds.
const ALBUMS: usize = 40;

/// One artist.
#[component]
pub fn ArtistView(
    /// Which artist.
    id: ArtistId,
) -> impl IntoView {
    let app = ui();
    let artist = RwSignal::new_local(Option::<Artist>::None);
    let top = RwSignal::new_local(Rc::new(Vec::<Track>::new()));
    let albums = RwSignal::new_local(Rc::new(Vec::<Album>::new()));

    {
        let app = app.clone();
        let id = id.clone();
        spawn_local(async move {
            match app.services.api.artist(&id).await {
                Ok(found) => artist.set(Some(found)),
                Err(error) => tracing::warn!(%error, %id, "cannot read the artist"),
            }
            match app.services.api.artist_top_tracks(&id).await {
                Ok(tracks) => {
                    app.state.load_liked_for(&app.services, &tracks);
                    top.set(Rc::new(tracks));
                }
                Err(error) => tracing::warn!(%error, %id, "cannot read the top tracks"),
            }
            // The catalogue endpoints hand out ten at a time. Take a few pages, so the grid
            // holds a useful part of what an artist released.
            let mut all = Vec::new();
            let mut offset = 0_u32;
            while all.len() < ALBUMS {
                match app.services.api.artist_albums(&id, offset).await {
                    Ok(page) => {
                        let more = page.has_more();
                        offset = page.next_offset();
                        all.extend(page.items);
                        albums.set(Rc::new(all.clone()));
                        if !more {
                            break;
                        }
                    }
                    Err(error) => {
                        tracing::warn!(%error, %id, "cannot read the albums");
                        break;
                    }
                }
            }
        });
    }

    let title = Signal::derive_local(move || {
        artist.with(|artist| artist.as_ref().map(|it| it.name.clone()).unwrap_or_default())
    });

    let images = Signal::derive_local(move || {
        artist.with(|artist| {
            artist
                .as_ref()
                .map(|it| it.images.clone())
                .unwrap_or_default()
        })
    });

    // Spotify stopped giving follower counts to applications like this one, so the line under
    // the name counts what it does give.
    let summary = Signal::derive_local(move || {
        let releases = albums.with(|albums| albums.len());
        match releases {
            0 => String::new(),
            1 => "1 release".to_owned(),
            many => format!("{many} releases"),
        }
    });

    let radio: std::rc::Rc<dyn Fn()> = {
        let app = app.clone();
        std::rc::Rc::new(move || {
            // A station needs a track to build on. Take the first one this artist is known for.
            if let Some(seed) = top.with(|tracks| tracks.first().map(|track| track.id.clone())) {
                crate::ui::track::start_radio(&app, seed);
            }
        })
    };

    let has_top = Signal::derive_local(move || top.with(|tracks| !tracks.is_empty()));

    view! {
        column(class = "view") {
            Header(
                kind = "Artist",
                title = title,
                summary = summary,
                images = images,
                tracks = top,
                context = Signal::stored_local(PlayContext::Adhoc),
                round = true
            ) {
                {move || {
                    let radio = radio.clone();
                    if has_top.get() {
                        view! {
                            Button(
                                variant = ButtonVariant::Ghost,
                                size = ButtonSize::IconSm,
                                on:click = move |_| radio()
                            ) {
                                Icon(svg = art::RADIO, size = IconSize::Sm, label = "Song radio")
                            }
                        }
                        .any()
                    } else {
                        ().any()
                    }
                }}
            }

            column(class = "sect") {
                {move || {
                    if has_top.get() {
                        view! {
                            text(class = "sect__label") {"Popular"}
                            box(class = "sect__top") {
                                Tracklist(
                                    tracks = top,
                                    context = Signal::stored_local(PlayContext::Adhoc)
                                )
                            }
                        }
                        .any()
                    } else {
                        ().any()
                    }
                }}

                text(class = "sect__label") {"Releases"}
                AlbumGrid(albums = albums)
            }
        }
    }
}

/// A grid of covers.
#[component]
pub fn AlbumGrid(
    /// Which albums.
    #[prop(into)]
    albums: Signal<Rc<Vec<Album>>, zgui::reactive::LocalStorage>,
) -> impl IntoView {
    let router = ui().router;

    view! {
        scroll(class = "grid") {
            {move || {
                albums
                    .get()
                    .iter()
                    .map(|album| {
                        let id = album.id.clone();
                        let name =
                            crate::ui::format::truncate(&album.name, crate::ui::format::CARD).into_owned();
                        let year = album.year.map(|year| year.to_string()).unwrap_or_default();
                        let images = album.images.clone();
                        view! {
                            control(
                                class = "card",
                                tabindex = Focus::Sequential,
                                on:click = move |_| router.go(Route::Album(id.clone()))
                            ) {
                                Art(
                                    images = Signal::stored_local(images),
                                    class = "art art-card",
                                    want = 300
                                )
                                text(class = "card__title") {{name}}
                                text(class = "card__note") {{year}}
                            }
                        }
                        .any()
                    })
                    .collect::<Vec<_>>()
            }}
        }
    }
}
