//! The saved tracks.
//!
//! Spotify builds this list and reaches it through its own endpoints, so it has no identifier and
//! no cover. The pages are read one after another until the list is whole.

use std::rc::Rc;

use zgui::prelude::*;
use zgui::reactive::RwSignal;
use zgui::{component, view};

use crate::player::PlayContext;
use crate::ui::format;
use crate::ui::Erase;
use crate::ui::state::ui;
use crate::ui::track::TracklistProps;
use crate::ui::views::WaitingProps;
use crate::ui::views::header::HeaderProps;

/// The saved tracks.
#[component]
pub fn Liked() -> impl IntoView {
    let app = ui();
    let tracks = app.state.tracks;
    tracks.set(Rc::new(Vec::new()));
    let loading = RwSignal::new_local(true);

    {
        let app = app.clone();
        spawn_local(async move {
            // Show what the cache holds while the pages arrive.
            let key = crate::cache::ListKey::Liked;
            let cached = app.services.cache.tracks(&key).await;
            let fresh =
                !cached.is_empty() && app.services.cache.is_fresh(&key, crate::cache::FRESH).await;
            if !cached.is_empty() {
                tracks.set(Rc::new(cached.clone()));
                loading.set(false);
            }
            // Five hundred saved tracks are ten requests. A recent copy answers for nothing.
            if fresh {
                app.state.liked_total.set(
                    u32::try_from(cached.len()).unwrap_or(u32::MAX),
                );
                app.state.liked.update(|liked| {
                    liked.extend(cached.iter().map(|track| track.id.clone()));
                });
                app.state.liked_known.update(|known| {
                    known.extend(cached.iter().map(|track| track.id.clone()));
                });
                return;
            }

            let mut all = Vec::new();
            let mut offset = 0_u32;
            let mut whole = true;
            loop {
                match app.services.api.saved_tracks(offset).await {
                    Ok(page) => {
                        let more = page.has_more();
                        offset = page.next_offset();
                        all.extend(page.items);
                        // Show what has arrived while the rest is on its way.
                        tracks.set(Rc::new(all.clone()));
                        app.state.liked_total.set(page.total);
                        if !more {
                            break;
                        }
                    }
                    Err(error) => {
                        tracing::warn!(%error, "cannot read the saved tracks");
                        whole = false;
                        break;
                    }
                }
            }
            loading.set(false);
            // A reading that stopped part way is not what the account holds. Keep what was there.
            if whole {
                app.services.cache.put_saved(&all).await;
            } else if !cached.is_empty() {
                tracks.set(Rc::new(cached.clone()));
            }

            // Every track here is saved by definition.
            app.state.liked.update(|liked| {
                liked.extend(all.iter().map(|track| track.id.clone()));
            });
            app.state.liked_known.update(|known| {
                known.extend(all.iter().map(|track| track.id.clone()));
            });
        });
    }

    let summary = Signal::derive_local(move || {
        let tracks = tracks.get();
        let length = tracks.iter().map(|track| track.duration).sum();
        format::summary(tracks.len() as u32, length)
    });

    view! {
        column(class = "view") {
            Header(
                kind = "Playlist",
                title = "Liked Songs",
                summary = summary,
                images = Signal::stored_local(Vec::new()),
                tracks = tracks,
                context = Signal::stored_local(PlayContext::Liked),
                hearted = true
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
                Tracklist(
                    tracks = tracks,
                    context = Signal::stored_local(PlayContext::Liked)
                )
            }
        }
    }
}
