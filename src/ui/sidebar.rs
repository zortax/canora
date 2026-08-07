//! The panel on the left: the library and the playlists.
//!
//! Every row carries the cover of what it names. A cover says which playlist a row is faster than
//! its name does, and it costs the rasteriser nothing: a picture is a texture, not a drawing.

use zgui::prelude::*;
use zgui::reactive::RwSignal;
use zgui::{component, view};
use zgui_ui::prelude::*;

use crate::models::{ImageRef, LibraryNode, Playlist, PlaylistKind};
use crate::ui::Erase;
use crate::ui::art::ArtProps;
use crate::ui::format;
use crate::ui::icons::{IconProps, IconSize, art};
use crate::ui::router::{Router, Route};
use crate::ui::state::ui;

/// The panel on the left.
#[component]
pub fn Sidebar() -> impl IntoView {
    let context = ui();
    let router = context.router;
    let state = context.state;

    let liked_count = Signal::derive_local(move || {
        let total = state.liked_total.get();
        if total == 0 {
            String::new()
        } else {
            total.to_string()
        }
    });

    let is_liked_route = Signal::derive_local(move || router.current() == Route::Liked);

    let window = use_window();

    view! {
        column(class = "side") {
            // The panel's own header is part of the window's handle: the strip across the top of
            // the main area is the rest of it, and the two together are the whole width.
            row(class = "side__brand", on:pointer_down = window.move_drag_handler()) {
                box(class = "side__logo") {
                    Icon(svg = art::WAVE, size = IconSize::Sm)
                }
                text(class = "side__name") {"canora"}
            }

            column(class = "side__group") {
                text(class = "side__label") {"Library"}
                control(
                    class = "side__item",
                    class:side__item-on = move || is_liked_route.get(),
                    tabindex = Focus::Sequential,
                    on:click = move |_| router.go(Route::Liked)
                ) {
                    // Saved tracks have no cover of their own, so they carry a mark instead.
                    box(class = "side__art side__art-liked") {
                        Icon(svg = art::HEART, size = IconSize::Xs)
                    }
                    text(class = "side__text") {"Liked Songs"}
                    text(class = "side__count") {{move || liked_count.get()}}
                }
            }

            column(class = "side__group side__group-grow") {
                row(class = "side__head") {
                    text(class = "side__label") {"Playlists"}
                    box(class = "side__grow")
                    NewPlaylist()
                }
                scroll(class = "side__list") {
                    PlaylistLinks()
                }
            }
        }
    }
}

/// Makes a playlist.
#[component]
fn NewPlaylist() -> impl IntoView {
    let open = RwSignal::new_local(false);
    let name = RwSignal::new_local(String::new());

    let create = move || {
        let wanted = name.get_untracked().trim().to_owned();
        if wanted.is_empty() {
            return;
        }
        open.set(false);
        name.set(String::new());

        let app = ui();
        let services = app.services.clone();
        let state = app.state;
        let router = app.router;
        zgui::reactive::spawn_detached(async move {
            match services.api.create_playlist(&wanted).await {
                Ok(playlist) => {
                    let id = playlist.id.clone();
                    state
                        .playlists
                        .update(|playlists| playlists.insert(0, playlist));
                    services
                        .cache
                        .put_playlists(&state.playlists.get_untracked())
                        .await;
                    // Show what was just made.
                    if let Some(id) = id {
                        router.go(Route::Playlist(id));
                    }
                }
                Err(error) => tracing::warn!(%error, "cannot make the playlist"),
            }
        });
    };

    let field = Binding::controlled(
        Signal::derive_local(move || name.get()),
        move |text: String| name.set(text),
    );

    view! {
        control(
            class = "side__add",
            tabindex = Focus::Sequential,
            a11y:label = "New playlist",
            on:click = move |_| open.set(true)
        ) {
            Icon(svg = art::PLUS, size = IconSize::Xs)
        }

        Dialog(open = Binding::controlled(
            Signal::derive_local(move || open.get()),
            move |wanted: bool| open.set(wanted)
        )) {
            DialogContent {
                DialogHeader {
                    DialogTitle {"New playlist"}
                }
                Input(value = field, placeholder = "Playlist name")
                DialogFooter {
                    DialogClose(variant = ButtonVariant::Outline) {"Cancel"}
                    Button(size = ButtonSize::Sm, on:click = move |_| create()) {"Create"}
                }
            }
        }
    }
}

/// One row per playlist, and one per folder with its playlists under it.
#[component]
fn PlaylistLinks() -> impl IntoView {
    let context = ui();
    let router = context.router;
    let state = context.state;

    view! {
        column(class = "side__items") {
            {move || {
                state
                    .library()
                    .into_iter()
                    .filter_map(|node| match node {
                        LibraryNode::Playlist(playlist) => playlist_link(router, &playlist, false),
                        LibraryNode::Folder { id, name, playlists } => {
                            Some(folder_rows(state, router, &id, &name, &playlists))
                        }
                    })
                    .collect::<Vec<_>>()
            }}
        }
    }
}

/// How long a folder takes to open. Long enough to see, short enough to stay out of the way.
const SWING: std::time::Duration = std::time::Duration::from_millis(140);

/// How tall one row inside a folder is, with the gap below it. This matches the sheet.
const ROW: f64 = 27.0;

/// What separates two rows. This matches the sheet.
const GAP: f64 = 1.0;

/// A folder and, when it is open, what it holds.
fn folder_rows(
    state: crate::ui::state::AppState,
    router: Router,
    id: &str,
    name: &str,
    playlists: &[Playlist],
) -> zgui::view::AnyView {
    let key = id.to_owned();
    let open = Signal::derive_local({
        let key = key.clone();
        move || state.open_folders.with(|open| open.contains(&key))
    });
    let label = format::truncate(name, format::SIDEBAR - 2).into_owned();
    let count = playlists.len().to_string();

    // How far the folder is through opening. The rows stay in place while it moves, and the strip
    // that holds them grows past them, so the list slides out from under the folder's own row.
    let swing = crate::ui::motion::travel(open, SWING);
    // Every row but the last carries the gap below it.
    let rows = f64::from(u32::try_from(playlists.len()).unwrap_or(u32::MAX));
    let tall = (rows * ROW - GAP).max(0.0);
    let inside = Signal::derive_local(move || format!("{:.1}px", swing.get() * tall));

    // The rows are built each time the folder opens: a view cannot be copied, so one list
    // cannot be handed out twice.
    let held: Vec<Playlist> = playlists.to_vec();

    view! {
        control(
            class = "side__item side__folder",
            tabindex = Focus::Sequential,
            on:click = {
                let key = key.clone();
                move |_| state.toggle_folder(&key)
            }
        ) {
            box(
                class = "side__twist",
                style:transform = move || Some(format!("rotate({:.1}deg)", swing.get() * 90.0))
            ) {
                Icon(svg = art::CHEVRON_RIGHT, size = IconSize::Xs)
            }
            text(class = "side__text") {{label.clone()}}
            text(class = "side__count") {{count.clone()}}
        }
        {move || {
            // Nothing is built while the folder is shut, so a closed sidebar draws only its own
            // rows however many playlists the folders hold.
            if swing.get() <= 0.0 {
                return ().any();
            }
            let rows: Vec<_> = held
                .iter()
                .filter_map(|playlist| playlist_link(router, playlist, true))
                .collect();
            view! {
                box(
                    class = "side__inside",
                    style:height = move || Some(inside.get()),
                    style:opacity = move || Some(format!("{:.2}", swing.get()))
                ) {
                    column(class = "side__rows") {{rows}}
                }
            }
            .any()
        }}
    }
    .any()
}

/// One link, or nothing when the playlist carries no identifier.
fn playlist_link(router: Router, playlist: &Playlist, nested: bool) -> Option<zgui::view::AnyView> {
    let id = playlist.id.clone()?;
    let route = Route::Playlist(id);
    let target = route.clone();
    let budget = if nested {
        format::SIDEBAR - 3
    } else {
        format::SIDEBAR
    };
    let name = format::truncate(&playlist.name, budget).into_owned();
    let owned = matches!(playlist.kind, PlaylistKind::Owned);
    let images: Vec<ImageRef> = playlist.images.clone();

    Some(
        view! {
            control(
                class = "side__item",
                class:side__item-in = move || nested,
                class:side__item-on = move || router.current() == route,
                class:side__item-own = move || owned,
                tabindex = Focus::Sequential,
                on:click = move |_| router.go(target.clone())
            ) {
                Art(
                    images = Signal::stored_local(images.clone()),
                    class = "art side__art",
                    want = 64
                )
                text(class = "side__text") {{name.clone()}}
            }
        }
        .any(),
    )
}
