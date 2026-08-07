//! What plays next.
//!
//! The list opens over the bar rather than beside it, so the window keeps its shape. Rows here
//! carry no icons: the list can be long, and every icon costs a rasteriser pass.

use zgui::prelude::*;
use zgui::{component, view};
use zgui_ui::prelude::*;
use zgui_ui::OverlayState;
use zgui::vocab::HasPopup;

use crate::ui::Erase;
use crate::ui::format;
use crate::ui::icons::{IconProps, IconSize, art};
use crate::ui::state::ui;

/// The button that opens the queue.
#[component]
pub fn QueueButton() -> impl IntoView {
    view! {
        Popover {
            QueueTrigger()
            PopoverContent(class = "queue") {
                QueuePanel()
            }
        }
    }
}

/// What opens the queue.
///
/// It opens on the press rather than on the release, which is what a menu does and what a pointer
/// expects of one. The library's own trigger opens on the click, and a press that opened it first
/// would meet that click on the way up and close it again — so this is the same trigger with the
/// one line moved.
#[component]
fn QueueTrigger() -> impl IntoView {
    let state = OverlayState::current();
    let own = state.map_or_else(Attrs::new, |state| state.trigger_attrs(HasPopup::Dialog));
    let node = state.map_or_else(NodeRef::new, |state| state.trigger());

    view! {
        Button(
            node_ref = node,
            variant = ButtonVariant::Ghost,
            size = ButtonSize::IconSm,
            on:pointer_down = move |_| {
                if let Some(state) = state {
                    state.toggle();
                }
            },
            {..own}
        ) {
            Icon(svg = art::QUEUE, size = IconSize::Sm, label = "Queue")
        }
    }
}

/// What plays next.
#[component]
fn QueuePanel() -> impl IntoView {
    let playback = ui().playback;

    let now = Signal::derive_local(move || {
        playback.playing.with(|playing| {
            playing.as_ref().map(|it| {
                (
                    format::truncate(&it.track.name, format::QUEUE).into_owned(),
                    format::truncate(&it.track.artist_line(), format::QUEUE).into_owned(),
                )
            })
        })
    });

    view! {
        column(class = "queue__body") {
            {move || {
                now.get()
                    .map(|(title, artist)| {
                        view! {
                            column(class = "queue__group") {
                                text(class = "queue__label") {"Now playing"}
                                column(class = "queue__row queue__row-on") {
                                    text(class = "queue__title") {{title}}
                                    text(class = "queue__artist") {{artist}}
                                }
                            }
                        }
                        .any()
                    })
                    .unwrap_or_else(|| ().any())
            }}

            column(class = "queue__group") {
                text(class = "queue__label") {"Next"}
                scroll(class = "queue__list") {
                    {move || {
                        let upcoming = playback.upcoming.get();
                        if upcoming.is_empty() {
                            return view! {
                                text(class = "queue__empty") {"Nothing is queued."}
                            }
                            .any();
                        }
                        upcoming
                            .iter()
                            .take(50)
                            .map(|track| {
                                let title =
                                    format::truncate(&track.name, format::QUEUE).into_owned();
                                let artist = format::truncate(
                                    &track.artist_line(),
                                    format::QUEUE,
                                )
                                .into_owned();
                                view! {
                                    column(class = "queue__row") {
                                        text(class = "queue__title") {{title}}
                                        text(class = "queue__artist") {{artist}}
                                    }
                                }
                                .any()
                            })
                            .collect::<Vec<_>>()
                            .any()
                    }}
                }
            }
        }
    }
}
