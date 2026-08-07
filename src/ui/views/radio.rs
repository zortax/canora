//! A station built around one track.
//!
//! The station reads like a playlist and plays like one. The track it was built around leads it:
//! asking for a song's radio and hearing a different song first reads as the wrong station.

use std::rc::Rc;

use zgui::prelude::*;
use zgui::reactive::RwSignal;
use zgui::{component, view};

use crate::models::{Track, TrackId};
use crate::player::PlayContext;
use crate::ui::Erase;
use crate::ui::format;
use crate::ui::state::ui;
use crate::ui::track::TracklistProps;
use crate::ui::views::WaitingProps;
use crate::ui::views::header::HeaderProps;

/// A station built around one track.
#[component]
pub fn RadioView(
    /// The track it is built around.
    seed: TrackId,
) -> impl IntoView {
    let app = ui();
    let tracks = app.state.tracks;
    tracks.set(Rc::new(Vec::new()));
    let loading = RwSignal::new_local(true);
    let about = RwSignal::new_local(Option::<Track>::None);

    {
        let app = app.clone();
        let seed = seed.clone();
        spawn_local(async move {
            match app.services.radio.for_track(&app.services.api, &seed).await {
                Ok(found) => {
                    app.state.load_liked_for(&app.services, &found);
                    about.set(found.first().cloned());
                    tracks.set(Rc::new(found));
                }
                Err(error) => tracing::warn!(%error, %seed, "cannot build the station"),
            }
            loading.set(false);
        });
    }

    let context = Signal::stored_local(PlayContext::Radio { seed });

    // The station is named after the track it leads with.
    let title = Signal::derive_local(move || {
        about.with(|track| {
            track
                .as_ref()
                .map(|track| format!("{} Radio", track.name))
                .unwrap_or_else(|| "Radio".to_owned())
        })
    });

    let kind = Signal::derive_local(move || {
        about.with(|track| {
            track
                .as_ref()
                .map(|track| format!("Station · {}", track.artist_line()))
                .unwrap_or_else(|| "Station".to_owned())
        })
    });

    let images = Signal::derive_local(move || {
        about.with(|track| {
            track
                .as_ref()
                .and_then(|track| track.album.as_ref())
                .map(|album| album.images.clone())
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
                Tracklist(tracks = tracks, context = context)
            }
        }
    }
}
