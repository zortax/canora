//! The rows a list of tracks is made of.
//!
//! A row shows a number, a title, an artist and a length. It shows a heart while the pointer is
//! over it, and nothing else: the rasteriser plans one pass per icon, and a list that showed an
//! icon on every row would spend the whole budget on hearts.
//!
//! One click plays. The framework declares a double-click event and never sends one, and this
//! interface has nothing a first click could select, so a single click is what plays a track.
//! Clicks on the heart stop where they land.
//!
//! Work a menu item starts runs detached. Choosing an item closes the menu, and a task owned by
//! the menu would be cancelled where it stood — which for a station is halfway through reading it.

use std::rc::Rc;

use zgui::prelude::*;
use zgui::reactive::RwSignal;
use zgui::{component, view};
use zgui_ui::prelude::*;

use crate::models::{Track, TrackId};
use crate::player::PlayContext;
use crate::ui::format;
use crate::ui::icons::{IconProps, IconSize, art};
use crate::ui::router::Route;
use crate::ui::Erase;
use crate::ui::art::ArtProps;
use crate::ui::state::ui;

/// How tall one row is. The list measures itself by this, so it must match the sheet.
pub const ROW_HEIGHT: f32 = 34.0;

/// How many rows are drawn beyond the ones on the screen.
const OVERSCAN: usize = 6;

/// A list of tracks.
///
/// Only the rows on the screen are built. A playlist of ten thousand tracks costs the same as one
/// of thirty.
#[component]
pub fn Tracklist(
    /// The tracks to show.
    #[prop(into)]
    tracks: Signal<Rc<Vec<Track>>, zgui::reactive::LocalStorage>,
    /// Where the tracks came from. Playing one plays the rest.
    #[prop(into)]
    context: Signal<PlayContext, zgui::reactive::LocalStorage>,
    /// Whether to show the number of each track on its album.
    #[prop(default = false)]
    album_numbers: bool,
) -> impl IntoView {
    let count = Signal::derive_local(move || tracks.with(|tracks| tracks.len()));

    view! {
        VirtualList(
            count = count,
            row_size = ROW_HEIGHT,
            overscan = OVERSCAN,
            label = "Tracks",
            class = "list",
            row = move |index: usize| {
                // The list itself, and nothing lifted out of it. A row is mounted every row-height
                // of travel, and taking the track by value copied its name, its artists and its
                // album's covers on every one of them — for a track the list handed to the same row
                // already holds.
                let all = tracks.get();
                if index >= all.len() {
                    return ().any();
                }
                view! {
                    TrackRow(
                        index = index,
                        all = all,
                        context = context.get(),
                        album_numbers = album_numbers
                    )
                }
                .any()
            }
        )
    }
}

/// One track.
#[component]
pub fn TrackRow(
    /// Where it sits in the list.
    index: usize,
    /// The whole list. This row is `all[index]`, and playing it plays the rest.
    all: Rc<Vec<Track>>,
    /// Where the list came from.
    context: PlayContext,
    /// Whether to show the number of the track on its album.
    #[prop(default = false)]
    album_numbers: bool,
) -> impl IntoView {
    let app = ui();
    let state = app.state;
    let playback = app.playback;

    // Borrowed for the length of this build. Everything below takes the one field it needs, so a
    // mount copies a title and a line of artists rather than the whole record twice over.
    let Some(track) = all.get(index) else {
        return ().any();
    };
    let playable = track.playable;

    let id = track.id.clone();
    let playing = Signal::derive_local({
        let id = id.clone();
        move || playback.current_id().as_ref() == Some(&id)
    });

    let liked = Binding::controlled(
        Signal::derive_local({
            let id = id.clone();
            move || state.is_liked(&id)
        }),
        {
            let app = app.clone();
            let id = id.clone();
            move |on: bool| app.state.set_liked(&app.services, id.clone(), on)
        },
    );

    // A saved track keeps its heart on show. An unsaved one draws it open, and only under the
    // pointer.
    let is_liked = Signal::derive_local({
        let id = id.clone();
        move || state.is_liked(&id)
    });
    let heart = Signal::derive_local(move || {
        if is_liked.get() {
            art::HEART
        } else {
            art::HEART_LINE
        }
    });

    let number = if album_numbers {
        track.track_number.to_string()
    } else {
        (index + 1).to_string()
    };
    let title = format::truncate(&track.name, format::TITLE).into_owned();
    let artist_line = format::truncate(&track.artist_line(), format::ARTIST).into_owned();
    let length = format::duration(track.duration);
    // The one identifier the link needs, rather than every artist the track has: a row shows the
    // names as one line and offers a link only when there is no question which artist it means.
    let lone_artist = (track.artists.len() == 1).then(|| track.artists[0].id.clone());

    let play: Rc<dyn Fn()> = {
        let app = app.clone();
        let context = context.clone();
        let all = Rc::clone(&all);
        Rc::new(move || {
            app.services
                .player
                .play_tracks((*all).clone(), index, context.clone());
        })
    };

    // One artist is a link. Several are a line of text: which one a click meant would be a guess.
    let artist_view = if let Some(id) = lone_artist {
        let router = app.router;
        let name = artist_line.clone();
        view! {
            control(
                class = "trk__artist trk__link",
                tabindex = Focus::Sequential,
                on:click:stop = move |_| router.go(Route::Artist(id.clone()))
            ) {{name.clone()}}
        }
        .any()
    } else {
        view! { text(class = "trk__artist") {{artist_line.clone()}} }.any()
    };

    // A playlist shows the cover of each track. An album shows numbers: every track there
    // carries the same cover, and twelve copies of it say less than 1 to 12.
    let lead = if album_numbers {
        view! {
            box(class = "trk__mark") {
                text(class = "trk__num") {{number.clone()}}
                box(class = "trk__sign") {
                    Icon(svg = art::PLAY, size = IconSize::Xs)
                }
            }
        }
        .any()
    } else {
        // Read here rather than above: an album row shows a number and never touches the covers,
        // so lifting them out unconditionally copied a vector per mount for half the lists that
        // exist.
        let images = track
            .album
            .as_ref()
            .map(|album| album.images.clone())
            .unwrap_or_default();
        view! {
            Art(images = Signal::stored_local(images), class = "art art-row", want = 64)
            box(class = "trk__mark trk__mark-over") {
                box(class = "trk__sign") {
                    Icon(svg = art::PLAY, size = IconSize::Xs)
                }
            }
        }
        .any()
    };

    let on_row = play.clone();
    // The menu's copy is taken when the menu opens, not when the row mounts: the content is built
    // behind a presence gate, and a right-click is rare next to a row that arrives every
    // row-height of a scroll.
    let menu_list = Rc::clone(&all);
    view! {
        ContextMenu {
            ContextMenuTrigger {
                row(
                    class = "trk",
                    class:trk-on = move || playing.get(),
                    class:trk-off = move || !playable,
                    class:trk-liked = move || is_liked.get(),
                    tabindex = Focus::Sequential,
                    // Only the primary button plays. A right press opens the menu, and a track
                    // already playing carries on.
                    on:click = move |ev: &mut EventCx<'_, events::Click>| {
                        if ev.button != Some(PointerButton::Secondary) {
                            on_row();
                        }
                    }
                ) {
                    box(class = "trk__lead") {
                        {lead}
                    }
                    column(class = "trk__main") {
                        text(class = "trk__title") {{title.clone()}}
                        {artist_view}
                    }
                    row(class = "trk__acts", on:click:stop = move |_| {}) {
                        Toggle(
                            pressed = liked,
                            size = ToggleSize::Sm,
                            class = "trk__like",
                            a11y:label = "Save to your library"
                        ) {
                            Icon(svg = heart, size = IconSize::Xs)
                        }
                    }
                    text(class = "trk__time") {{length.clone()}}
                }
            }
            ContextMenuContent {
                TrackMenu(
                    track = menu_list[index].clone(),
                    context = context.clone(),
                    play = play.clone()
                )
            }
        }
    }
    .any()
}

/// What a track offers.
#[component]
fn TrackMenu(
    /// Which track.
    track: Track,
    /// Where the list came from. A playlist this person owns may lose the track.
    context: PlayContext,
    /// Plays it where it sits.
    play: Rc<dyn Fn()>,
) -> impl IntoView {
    let app = ui();
    let router = app.router;

    let album = track.album.as_ref().map(|album| album.id.clone());
    let artist = track.artists.first().map(|artist| artist.id.clone());
    let id = track.id.clone();

    let radio: Rc<dyn Fn()> = {
        let app = app.clone();
        let id = id.clone();
        Rc::new(move || start_radio(&app, id.clone()))
    };
    let queue: Rc<dyn Fn()> = {
        let app = app.clone();
        let track = track.clone();
        Rc::new(move || app.services.player.play_next(track.clone()))
    };

    // The playlists this person may add to. The list is rebuilt on every open, so a playlist
    // made a moment ago is already there.
    // Hold the track in a signal, so this closure carries nothing but copyable handles and can
    // be built again on every open.
    let which = RwSignal::new_local(id.clone());
    let add_to = move || {
        let app = ui();
        {
            let app = app.clone();
            app.state
                .playlists
                .get()
                .into_iter()
                .filter(|playlist| playlist.kind.is_editable())
                .filter_map(|playlist| {
                    let target = playlist.id.clone()?;
                    let name = crate::ui::format::truncate(&playlist.name, 28).into_owned();
                    let app = app.clone();
                    Some(
                        view! {
                            MenuItem(on_select = UnsyncCallback::new(move |()| {
                                add_track(&app, target.clone(), which.get_untracked());
                            })) {{name.clone()}}
                        }
                        .any(),
                    )
                })
                .collect::<Vec<_>>()
        }
    };

    // Build the navigation entries first. A block that follows a childless element is read as
    // that element's children, so the separator travels with them.
    let mut nav: Vec<zgui::view::AnyView> = Vec::new();

    // A track can leave only a playlist this person owns.
    if let PlayContext::Playlist(from) = &context {
        let editable = app.state.playlists.with(|playlists| {
            playlists
                .iter()
                .any(|it| it.id.as_ref() == Some(from) && it.kind.is_editable())
        });
        if editable {
            let from = from.clone();
            let app = app.clone();
            let track = id.clone();
            nav.push(view! { MenuSeparator() }.any());
            nav.push(
                view! {
                    MenuItem(
                        destructive = true,
                        on_select = UnsyncCallback::new(move |()| {
                            remove_track(&app, from.clone(), track.clone());
                        })
                    ) {
                        Icon(svg = art::REMOVE, size = IconSize::Xs)
                        "Remove from this playlist"
                    }
                }
                .any(),
            );
        }
    }
    if artist.is_some() || album.is_some() {
        nav.push(view! { MenuSeparator() }.any());
    }
    if let Some(id) = artist {
        nav.push(
            view! {
                MenuItem(on_select = UnsyncCallback::new(move |()| {
                    router.go(Route::Artist(id.clone()));
                })) {
                    Icon(svg = art::ARTIST, size = IconSize::Xs)
                    "Go to artist"
                }
            }
            .any(),
        );
    }
    if let Some(id) = album {
        nav.push(
            view! {
                MenuItem(on_select = UnsyncCallback::new(move |()| {
                    router.go(Route::Album(id.clone()));
                })) {
                    Icon(svg = art::ALBUM, size = IconSize::Xs)
                    "Go to album"
                }
            }
            .any(),
        );
    }

    view! {
        MenuItem(on_select = UnsyncCallback::new(move |()| play())) {
            Icon(svg = art::PLAY, size = IconSize::Xs)
            "Play"
        }
        MenuItem(on_select = UnsyncCallback::new(move |()| queue())) {
            Icon(svg = art::QUEUE, size = IconSize::Xs)
            "Play next"
        }
        MenuItem(on_select = UnsyncCallback::new(move |()| radio())) {
            Icon(svg = art::RADIO, size = IconSize::Xs)
            "Go to song radio"
        }
        MenuSub {
            MenuSubTrigger {
                Icon(svg = art::ADD_TO_PLAYLIST, size = IconSize::Xs)
                "Add to playlist"
            }
            MenuSubContent {
                {add_to}
            }
        }
        {nav}
    }
}

/// Adds a track to a playlist.
///
/// Say what happened: a playlist this person does not own answers with a refusal, and a silent
/// menu would leave them guessing.
fn add_track(app: &Rc<crate::ui::state::Ui>, playlist: crate::models::PlaylistId, track: TrackId) {
    let api = app.services.api.clone();
    let state = app.state;
    zgui::reactive::spawn_detached(async move {
        match api.add_tracks(&playlist, &[track], None).await {
            Ok(_) => {
                // The playlist holds one more track than the sidebar last heard.
                state.playlists.update(|playlists| {
                    if let Some(found) = playlists
                        .iter_mut()
                        .find(|it| it.id.as_ref() == Some(&playlist))
                    {
                        found.total_tracks += 1;
                    }
                });
            }
            Err(error) => tracing::warn!(%error, %playlist, "cannot add the track"),
        }
    });
}

/// Takes a track out of a playlist.
fn remove_track(
    app: &Rc<crate::ui::state::Ui>,
    playlist: crate::models::PlaylistId,
    track: TrackId,
) {
    let api = app.services.api.clone();
    let state = app.state;
    zgui::reactive::spawn_detached(async move {
        match api.remove_tracks(&playlist, std::slice::from_ref(&track), None).await {
            Ok(_) => {
                state.playlists.update(|playlists| {
                    if let Some(found) = playlists
                        .iter_mut()
                        .find(|it| it.id.as_ref() == Some(&playlist))
                    {
                        found.total_tracks = found.total_tracks.saturating_sub(1);
                    }
                });
                // Take the row out here. Spotify agrees with itself a moment later, and a
                // reader that refetched now would still see the track.
                state.tracks.update(|tracks| {
                    let kept: Vec<_> = tracks
                        .iter()
                        .filter(|it| it.id != track)
                        .cloned()
                        .collect();
                    *tracks = Rc::new(kept);
                });
            }
            Err(error) => tracing::warn!(%error, %playlist, "cannot remove the track"),
        }
    });
}

/// Opens the station built around `seed`.
///
/// The view reads the station itself, so this only says where to go.
pub fn start_radio(app: &Rc<crate::ui::state::Ui>, seed: TrackId) {
    app.router.go(Route::Radio(seed));
}
