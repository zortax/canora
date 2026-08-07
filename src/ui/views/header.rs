//! The block above a list of tracks: a cover, what it is, and what may be done to it.

use std::rc::Rc;

use zgui::prelude::*;
use zgui::{component, view};
use zgui_ui::prelude::*;

use crate::models::{ImageRef, Track};
use crate::player::PlayContext;
use crate::ui::art::ArtProps;
use crate::ui::icons::{IconProps, IconSize, art};
use crate::ui::Erase;
use crate::ui::state::ui;

/// The block above a list of tracks.
#[expect(clippy::too_many_arguments, reason = "each one is a prop the view! macro names")]
#[component]
pub fn Header(
    /// What kind of thing this is, above the name.
    #[prop(into)]
    kind: Signal<String, zgui::reactive::LocalStorage>,
    /// What it is called.
    #[prop(into)]
    title: Signal<String, zgui::reactive::LocalStorage>,
    /// How many tracks it holds and how long it runs.
    #[prop(into)]
    summary: Signal<String, zgui::reactive::LocalStorage>,
    /// Its cover.
    #[prop(into)]
    images: Signal<Vec<ImageRef>, zgui::reactive::LocalStorage>,
    /// The tracks, for the play button.
    #[prop(into)]
    tracks: Signal<Rc<Vec<Track>>, zgui::reactive::LocalStorage>,
    /// Where the tracks came from.
    #[prop(into)]
    context: Signal<PlayContext, zgui::reactive::LocalStorage>,
    /// Whether the cover is round, as an artist's picture is.
    #[prop(default = false)]
    round: bool,
    /// Whether to draw a heart where the cover would be, as the saved tracks have none.
    #[prop(default = false)]
    hearted: bool,
    /// What else this offers, beside the play button.
    #[prop(optional)]
    children: Option<Children>,
) -> impl IntoView {
    let app = ui();
    let playback = app.playback;

    let play = {
        let app = app.clone();
        move || {
            let tracks = tracks.get();
            if tracks.is_empty() {
                return;
            }
            app.services
                .player
                .play_tracks((*tracks).clone(), 0, context.get());
        }
    };

    let shuffle = {
        let app = app.clone();
        move || {
            let tracks = tracks.get();
            if tracks.is_empty() {
                return;
            }
            // Turn shuffle on first, so the queue is built shuffled.
            app.services.player.set_shuffle(true);
            app.services
                .player
                .play_tracks((*tracks).clone(), 0, context.get());
        }
    };

    let empty = Signal::derive_local(move || tracks.with(|tracks| tracks.is_empty()));

    view! {
        row(class = "hdr") {
            {if hearted {
                view! {
                    box(class = "art art-hero art-hearted") {
                        Icon(svg = art::HEART, size = IconSize::Md)
                    }
                }
                .any()
            } else {
                view! {
                    Art(
                        images = images,
                        class = if round { "art art-hero art-round" } else { "art art-hero" },
                        want = 640
                    )
                }
                .any()
            }}
            column(class = "hdr__meta") {
                text(class = "hdr__kind") {{move || kind.get()}}
                text(class = "hdr__title") {{move || title.get()}}
                text(class = "hdr__sub") {{move || summary.get()}}
                row(class = "hdr__acts") {
                    Button(
                        variant = ButtonVariant::Outline,
                        size = ButtonSize::Sm,
                        disabled = empty,
                        on:click = move |_| play()
                    ) {
                        Icon(svg = art::PLAY, size = IconSize::Sm)
                        "Play"
                    }
                    Button(
                        variant = ButtonVariant::Ghost,
                        size = ButtonSize::IconSm,
                        disabled = empty,
                        on:click = move |_| shuffle()
                    ) {
                        Icon(svg = art::SHUFFLE, size = IconSize::Sm, label = "Shuffle")
                    }
                    {children
                        .map(|children| children.into_view_once())
                        .unwrap_or_else(|| ().any())}
                    box(class = "hdr__spacer")
                    text(class = "hdr__now") {{
                        move || playback
                            .playing
                            .with(|p| p.as_ref().map(|it| {
                                format!("{} of {}", it.index + 1, it.queue_len)
                            }))
                            .unwrap_or_default()
                    }}
                }
            }
        }
    }
}
