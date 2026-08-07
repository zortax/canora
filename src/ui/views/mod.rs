//! What the main area shows.

pub mod album;
pub mod artist;
pub mod header;
pub mod liked;
pub mod playlist;
pub mod radio;
pub mod search;
pub mod welcome;

use zgui::prelude::*;
use zgui::{component, view};
use zgui_ui::prelude::*;

use crate::ui::Erase;

/// Rows that stand in for a list that has not arrived.
#[component]
pub fn Waiting() -> impl IntoView {
    view! {
        column(class = "wait") {
            {(0..8)
                .map(|_| view! { Skeleton(class = "wait__row") }.any())
                .collect::<Vec<_>>()}
        }
    }
}
