//! Album art.
//!
//! The picture arrives after the row is drawn. Until it does, the box holds a plain square, so a
//! list does not move as pictures land.

use zgui::prelude::*;
use zgui::reactive::{RenderEffect, RwSignal};
use zgui::{component, view};

use crate::models::ImageRef;
use crate::ui::icons::{IconProps, IconSize};
use crate::ui::state::ui;

/// One picture.
///
/// `size` is the class that measures the box. `want` is how wide a picture to ask Spotify for:
/// asking for a large one to draw it small costs bandwidth and decode time.
#[component]
pub fn Art(
    /// Which pictures are offered.
    #[prop(into)]
    images: Signal<Vec<ImageRef>, zgui::reactive::LocalStorage>,
    /// The class that measures the box.
    #[prop(default = "art art-sm")]
    class: &'static str,
    /// How wide a picture to ask for.
    #[prop(default = 300)]
    want: u32,
) -> impl IntoView {
    let source = RwSignal::new_local(Option::<String>::None);
    let context = ui();

    // Fetch when the pictures change, and once at the start.
    let watching = RenderEffect::new(move |_| {
        let url = images.with(|images| {
            crate::models::pick_image(images, want).map(|image| image.url.clone())
        });
        let Some(url) = url else {
            source.set(None);
            return;
        };

        let cache = context.services.images.clone();
        // Detached, so the fetch outlives the effect that started it. A task owned by the effect
        // is cancelled the moment the effect runs again — and a list rebuilds every time the
        // library behind it changes, which is twice before it has settled. Nothing kept a picture
        // long enough to show it.
        zgui::reactive::spawn_detached(async move {
            match cache.src_for(&url).await {
                // The row may be gone by the time the picture lands. Writing to the signal that
                // went with it is nothing to report.
                Ok(src) => {
                    source.try_set(Some(src));
                }
                Err(error) => tracing::warn!(%error, %url, "cannot show the picture"),
            }
        });
    });
    on_cleanup_local(move || drop(watching));

    view! {
        box(class = class) {
            if move || source.get().is_some() {
                image(class = "art__image", src = move || source.get(), a11y:hidden = true)
            } else {
                box(class = "art__none") {
                    Icon(svg = crate::ui::icons::art::MUSIC, size = IconSize::Xs)
                }
            }
        }
    }
}
