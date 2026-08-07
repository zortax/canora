//! One album.

use std::rc::Rc;

use zgui::prelude::*;
use zgui::reactive::RwSignal;
use zgui::{component, view};

use crate::models::{Album, AlbumId};
use crate::player::PlayContext;
use crate::ui::format;
use crate::ui::Erase;
use crate::ui::state::ui;
use crate::ui::track::TracklistProps;
use crate::ui::views::WaitingProps;
use crate::ui::views::header::HeaderProps;

/// One album.
#[component]
pub fn AlbumView(
    /// Which album.
    id: AlbumId,
) -> impl IntoView {
    let app = ui();
    let album = RwSignal::new_local(Option::<Album>::None);
    let tracks = app.state.tracks;
    tracks.set(Rc::new(Vec::new()));
    let loading = RwSignal::new_local(true);

    {
        let app = app.clone();
        let id = id.clone();
        spawn_local(async move {
            match app.services.api.album(&id).await {
                Ok((found, found_tracks)) => {
                    app.services.cache.put_album(&found, &found_tracks).await;
                    app.state.load_liked_for(&app.services, &found_tracks);
                    album.set(Some(found));
                    tracks.set(Rc::new(found_tracks));
                }
                Err(error) => tracing::warn!(%error, %id, "cannot read the album"),
            }
            loading.set(false);
        });
    }

    let context = Signal::stored_local(PlayContext::Album(id));

    let title = Signal::derive_local(move || {
        album.with(|album| album.as_ref().map(|it| it.name.clone()).unwrap_or_default())
    });

    let kind = Signal::derive_local(move || {
        album.with(|album| {
            album
                .as_ref()
                .map(|it| {
                    let artists = it
                        .artists
                        .iter()
                        .map(|artist| artist.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                    match it.year {
                        Some(year) => format!("Album · {artists} · {year}"),
                        None => format!("Album · {artists}"),
                    }
                })
                .unwrap_or_default()
        })
    });

    let images = Signal::derive_local(move || {
        album.with(|album| {
            album
                .as_ref()
                .map(|it| it.images.clone())
                .unwrap_or_default()
        })
    });

    let summary = Signal::derive_local(move || {
        let tracks = tracks.get();
        let length = tracks.iter().map(|track| track.duration).sum();
        format::summary(tracks.len() as u32, length)
    });

    view! {
        column(class = "view") {
            Header(
                kind = kind,
                title = title,
                summary = summary,
                images = images,
                tracks = tracks,
                context = context
            )
            box(class = "view__wait") {
                {move || {
                    if tracks.with(|tracks| tracks.is_empty()) && loading.get() {
                        view! { Waiting() }.any()
                    } else {
                        ().any()
                    }
                }}
            }
            box(class = "view__list") {
                Tracklist(tracks = tracks, context = context, album_numbers = true)
            }
        }
    }
}
