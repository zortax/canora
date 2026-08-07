//! One playlist.
//!
//! A playlist somebody else owns, such as Discover Weekly, is read here and edited nowhere: the
//! menu that renames and deletes is shown only for a playlist this person owns.

use std::rc::Rc;

use zgui::prelude::*;
use zgui::reactive::RwSignal;
use zgui::{component, view};
use zgui_ui::prelude::*;

use crate::models::{Playlist, PlaylistId, PlaylistKind};
use crate::player::PlayContext;
use crate::ui::format;
use crate::ui::icons::{IconProps, IconSize, art};
use crate::ui::Erase;
use crate::ui::router::Route;
use crate::ui::state::ui;
use crate::ui::track::TracklistProps;
use crate::ui::views::WaitingProps;
use crate::ui::views::header::HeaderProps;

/// One playlist.
#[component]
pub fn PlaylistView(
    /// Which playlist.
    id: PlaylistId,
) -> impl IntoView {
    let app = ui();
    let tracks = app.state.tracks;
    tracks.set(Rc::new(Vec::new()));
    let playlist = RwSignal::new_local(Option::<Playlist>::None);
    let loading = RwSignal::new_local(true);

    {
        let app = app.clone();
        let id = id.clone();
        // Owned by this view, so leaving the playlist stops the reading. A detached read carried
        // on against a page nobody was looking at any more.
        spawn_local(async move {
            // The name and the cover come first, so the header fills before the tracks arrive.
            // A playlist the sidebar already knows fills it with no request at all.
            match app.state.known_playlist(&id) {
                Some(found) => playlist.set(Some(found)),
                None => match app.services.api.playlist(&id).await {
                    Ok(found) => playlist.set(Some(found)),
                    Err(error) => tracing::warn!(%error, %id, "cannot read the playlist"),
                },
            }

            // Show what the cache holds while the pages arrive.
            let key = crate::cache::ListKey::Playlist(id.0.clone());
            let cached = app.services.cache.tracks(&key).await;
            let fresh = !cached.is_empty()
                && app.services.cache.is_fresh(&key, crate::cache::FRESH).await;
            if !cached.is_empty() {
                tracks.set(Rc::new(cached.clone()));
                loading.set(false);
            }
            // A recent copy is the whole answer. Reading it again costs one request per track for
            // a playlist Spotify makes.
            if fresh {
                app.state.load_liked_for(&app.services, &cached);
                return;
            }

            let mut all = Vec::new();
            let mut offset = 0_u32;
            let mut whole = true;
            loop {
                match app.services.api.playlist_tracks(&id, offset).await {
                    Ok(page) => {
                        let more = page.has_more();
                        whole &= page.whole;
                        offset = page.next_offset();
                        all.extend(page.items);
                        tracks.set(Rc::new(all.clone()));
                        if !more {
                            break;
                        }
                    }
                    Err(error) => {
                        tracing::warn!(%error, %id, "cannot read the tracks");
                        whole = false;
                        break;
                    }
                }
            }
            loading.set(false);
            // Only a whole reading replaces what is kept. A read that stopped part way — a rate
            // limit, a dropped connection — would otherwise put a short list in the cache and
            // mark it fresh, and the view would show that short list for as long as it stands.
            if whole {
                app.services.cache.put_tracks(&key, &all).await;
            } else if !cached.is_empty() {
                tracks.set(Rc::new(cached.clone()));
            }
            app.state.load_liked_for(&app.services, &all);
        });
    }

    let context = Signal::stored_local(PlayContext::Playlist(id.clone()));

    let title = Signal::derive_local(move || {
        playlist.with(|playlist| {
            playlist
                .as_ref()
                .map(|it| it.name.clone())
                .unwrap_or_default()
        })
    });

    let kind = Signal::derive_local(move || {
        playlist.with(|playlist| match playlist.as_ref().map(|it| &it.kind) {
            Some(PlaylistKind::Followed { owner_name }) => format!("Playlist · {owner_name}"),
            _ => "Playlist".to_owned(),
        })
    });

    let images = Signal::derive_local(move || {
        playlist.with(|playlist| {
            playlist
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

    let editable =
        Signal::derive_local(move || playlist.with(|it| {
            it.as_ref().map(|it| it.kind.is_editable()).unwrap_or(false)
        }));

    view! {
        column(class = "view") {
            Header(
                kind = kind,
                title = title,
                summary = summary,
                images = images,
                tracks = tracks,
                context = context
            ) {
                {move || {
                    if editable.get() {
                        view! { PlaylistMenu(id = id.clone(), playlist = playlist) }.any()
                    } else {
                        ().any()
                    }
                }}
            }
            box(class = "view__wait") {
                {move || {
                    if !tracks.with(|tracks| tracks.is_empty()) {
                        ().any()
                    } else if loading.get() {
                        view! { Waiting() }.any()
                    } else {
                        view! {
                            Empty {
                                EmptyTitle {"Nothing here yet"}
                                EmptyDescription {"This playlist holds no tracks."}
                            }
                        }
                        .any()
                    }
                }}
            }
            box(class = "view__list") {
                Tracklist(tracks = tracks, context = context)
            }
        }
    }
}

/// What may be done to a playlist this person owns.
///
/// Every callback here reads its playlist from a signal and the services from context, so each one
/// captures nothing but copyable handles. A menu is rebuilt on every open, and a callback that
/// held anything else could not be built twice.
#[component]
fn PlaylistMenu(
    /// Which playlist.
    id: PlaylistId,
    /// What is known about it.
    playlist: RwSignal<Option<Playlist>, zgui::reactive::LocalStorage>,
) -> impl IntoView {
    let which = RwSignal::new_local(id);
    let renaming = RwSignal::new_local(false);
    let name = RwSignal::new_local(String::new());

    let start_rename = move |()| {
        name.set(playlist.with(|it| it.as_ref().map(|it| it.name.clone()).unwrap_or_default()));
        renaming.set(true);
    };

    let commit_rename = move || {
        let wanted = name.get_untracked().trim().to_owned();
        let id = which.get_untracked();
        if wanted.is_empty() {
            return;
        }
        renaming.set(false);

        let app = ui();
        // Show the new name at once, and put the old one back if Spotify refuses.
        let previous = playlist.with_untracked(|it| it.as_ref().map(|it| it.name.clone()));
        set_name(playlist, app.state, &id, &wanted);

        let api = app.services.api.clone();
        let state = app.state;
        zgui::reactive::spawn_detached(async move {
            if let Err(error) = api.rename_playlist(&id, &wanted).await {
                tracing::warn!(%error, %id, "cannot rename the playlist");
                if let Some(previous) = previous {
                    set_name(playlist, state, &id, &previous);
                }
            }
        });
    };

    let delete = move |()| {
        let id = which.get_untracked();
        let app = ui();
        let api = app.services.api.clone();
        let state = app.state;
        let router = app.router;
        zgui::reactive::spawn_detached(async move {
            match api.unfollow_playlist(&id).await {
                Ok(()) => {
                    state
                        .playlists
                        .update(|playlists| playlists.retain(|it| it.id.as_ref() != Some(&id)));
                    router.go(Route::Liked);
                }
                Err(error) => tracing::warn!(%error, %id, "cannot remove the playlist"),
            }
        });
    };

    let field = Binding::controlled(
        Signal::derive_local(move || name.get()),
        move |text: String| name.set(text),
    );

    view! {
        DropdownMenu {
            DropdownMenuTrigger(variant = ButtonVariant::Ghost, size = ButtonSize::IconSm) {
                Icon(svg = art::ELLIPSIS, size = IconSize::Sm, label = "More")
            }
            DropdownMenuContent {
                MenuItem(on_select = UnsyncCallback::new(start_rename)) {
                    Icon(svg = art::RENAME, size = IconSize::Xs)
                    "Rename"
                }
                MenuSeparator()
                MenuItem(
                    destructive = true,
                    on_select = UnsyncCallback::new(delete)
                ) {
                    Icon(svg = art::REMOVE, size = IconSize::Xs)
                    "Delete"
                }
            }
        }

        Dialog(open = Binding::controlled(
            Signal::derive_local(move || renaming.get()),
            move |open: bool| renaming.set(open)
        )) {
            DialogContent {
                DialogHeader {
                    DialogTitle {"Rename playlist"}
                }
                Input(value = field, placeholder = "Playlist name")
                DialogFooter {
                    DialogClose(variant = ButtonVariant::Outline) {"Cancel"}
                    Button(size = ButtonSize::Sm, on:click = move |_| commit_rename()) {"Save"}
                }
            }
        }
    }
}

/// Writes a playlist name in both places the interface shows it.
fn set_name(
    playlist: RwSignal<Option<Playlist>, zgui::reactive::LocalStorage>,
    state: crate::ui::state::AppState,
    id: &PlaylistId,
    name: &str,
) {
    playlist.update(|it| {
        if let Some(it) = it {
            it.name = name.to_owned();
        }
    });
    state.playlists.update(|playlists| {
        if let Some(found) = playlists.iter_mut().find(|it| it.id.as_ref() == Some(id)) {
            found.name = name.to_owned();
        }
    });
}
