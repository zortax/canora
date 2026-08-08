//! How the window is laid out: the panel, the main area, and the bar that floats over both.
//!
//! The window carries no title bar. The strip across the top is both the header and the handle the
//! desktop moves the window by, so it looks like part of the page and behaves like a title bar.

use zgui::prelude::*;
use zgui::reactive::{LocalStorage, RenderEffect, RwSignal};
use zgui::{component, view};
use zgui_ui::prelude::*;
use zgui_ui_tokens::prelude::*;

use crate::ui::Erase;
use crate::ui::icons::{IconProps, IconSize, art};
use crate::ui::player::PlayerBarProps;
use crate::ui::router::Route;
use crate::ui::sidebar::SidebarProps;
use crate::ui::state::ui;
use crate::ui::theme::{ThemeMenuProps, Variant};
use crate::ui::views::album::AlbumViewProps;
use crate::ui::views::artist::ArtistViewProps;
use crate::ui::views::liked::LikedProps;
use crate::ui::views::playlist::PlaylistViewProps;
use crate::ui::views::radio::RadioViewProps;
use crate::ui::views::search::SearchViewProps;

/// Whether the desktop draws the frame around the window.
///
/// macOS keeps its frame for a window that asked for no title bar, so the window buttons, the
/// corners, the shadow and the resize edges all belong to the system. The other desktops hand the
/// whole frame over, and canora draws them itself.
pub const PLATFORM_FRAME: bool = cfg!(target_os = "macos");

/// The window.
#[component]
pub fn Shell(
    /// Which scheme the interface presents in.
    scheme: RwSignal<ColorScheme, LocalStorage>,
    /// Which theme it uses.
    variant: RwSignal<Variant, LocalStorage>,
) -> impl IntoView {
    let context = ui();
    let state = context.state;

    // The window draws on the cache at once, and reads Spotify when the session arrives.
    state.show_cached(&context.services);
    let services = context.services.clone();
    let linking = RenderEffect::new(move |_| {
        if state.link.get() == crate::ui::state::Link::Live {
            state.refresh(&services);
        }
    });
    on_cleanup_local(move || drop(linking));

    let router = context.router;

    view! {
        // The bar floats over the view: the list runs behind it and keeps its last rows
        // reachable through the padding the list carries.
        //
        // The two side buttons on a mouse go back and forward, the way they do in a browser. The
        // handler sits on the window rather than on any one view, so they work wherever the
        // pointer is.
        stack(
            class = "app",
            on:pointer_down = move |ev: &mut EventCx<'_, events::PointerDown>| {
                match ev.button {
                    Some(PointerButton::Back) => router.back(),
                    Some(PointerButton::Forward) => router.forward(),
                    _ => {}
                }
            }
        ) {
            row(class = "app__body") {
                Sidebar()
                column(class = "app__main") {
                    TopBar(scheme = scheme, variant = variant)
                    box(class = "app__view") {
                        View()
                    }
                }
            }
            PlayerBar()
            ResizeEdges()
        }
    }
}

/// Draws `view` where the desktop leaves it to the window.
///
/// The window buttons and the resize edges are the desktop's own wherever it keeps its frame, and
/// a second set drawn over them would be two of everything.
fn own_frame(view: impl IntoView + 'static) -> AnyView {
    if PLATFORM_FRAME {
        ().any()
    } else {
        view.any()
    }
}

/// The strip across the top.
///
/// It carries where the interface has been, the search field, and who is signed in. A press on
/// anything that is not a control moves the window.
#[component]
fn TopBar(
    /// Which scheme the interface presents in.
    scheme: RwSignal<ColorScheme, LocalStorage>,
    /// Which theme it uses.
    variant: RwSignal<Variant, LocalStorage>,
) -> impl IntoView {
    let context = ui();
    let router = context.router;
    let state = context.state;
    let window = use_window();

    let query = Binding::controlled(
        Signal::derive_local(move || state.query.get()),
        move |text: String| {
            let searching = !text.trim().is_empty();
            state.query.set(text);
            // Typing goes to the results. Clearing the field goes back.
            if searching && router.current() != Route::Search {
                router.go(Route::Search);
            }
        },
    );

    view! {
        row(class = "top", on:pointer_down = window.move_drag_handler()) {
            // A control inside the handle stops the press, or the click it wants never forms.
            row(class = "top__nav", on:pointer_down = window.no_drag_handler()) {
                Button(
                    variant = ButtonVariant::Ghost,
                    size = ButtonSize::IconSm,
                    disabled = Signal::derive_local(move || !router.can_go_back()),
                    on:click = move |_| router.back()
                ) {
                    Icon(svg = art::CHEVRON_LEFT, size = IconSize::Sm, label = "Back")
                }
                Button(
                    variant = ButtonVariant::Ghost,
                    size = ButtonSize::IconSm,
                    disabled = Signal::derive_local(move || !router.can_go_forward()),
                    on:click = move |_| router.forward()
                ) {
                    Icon(svg = art::CHEVRON_RIGHT, size = IconSize::Sm, label = "Forward")
                }
                box(class = "top__search") {
                    InputGroup {
                        InputGroupAddon {
                            Icon(svg = art::SEARCH, size = IconSize::Xs)
                        }
                        InputGroupInput(value = query, placeholder = "Search")
                    }
                }
            }

            box(class = "top__gap")

            row(class = "top__end", on:pointer_down = window.no_drag_handler()) {
                LinkMark()
                SyncButton()
                ThemeMenu(scheme = scheme, variant = variant)
                AccountMenu()
                WindowControls()
            }
        }
    }
}

/// Says when the account is out of reach, and tries again.
///
/// The window is drawn from the cache whatever Spotify answers, so a failure has to be visible
/// somewhere. It shows here, beside the button that reads the library, and a press tries the
/// connection again. Signing in from the start is in the account menu beside it.
#[component]
fn LinkMark() -> impl IntoView {
    let app = ui();
    let state = app.state;

    let broken = Signal::derive_local(move || {
        state.link.with(|link| matches!(link, crate::ui::state::Link::Broken(_)))
    });
    let why = Signal::derive_local(move || {
        state.link.with(|link| match link {
            crate::ui::state::Link::Broken(why) => format!("{why}. Press to try again."),
            _ => String::new(),
        })
    });

    view! {
        {move || {
            if !broken.get() {
                return ().any();
            }
            let app = app.clone();
            view! {
                Button(
                    variant = ButtonVariant::Ghost,
                    size = ButtonSize::IconSm,
                    class = "top__off",
                    a11y:label = why.get(),
                    on:click = move |_| app.state.link_up(&app.services)
                ) {
                    Icon(svg = art::OFFLINE, size = IconSize::Sm)
                }
            }
            .any()
        }}
    }
}

/// Forgets the login and goes back to the welcome screen.
fn forget(app: &std::rc::Rc<crate::ui::state::Ui>) {
    if let Err(error) = crate::auth::forget() {
        tracing::warn!(%error, "cannot forget the login");
    }
    app.services
        .player
        .send(crate::player::PlayerCommand::Shutdown);

    let stage = use_local_context::<
        zgui::reactive::RwSignal<crate::ui::views::welcome::Stage, zgui::reactive::LocalStorage>,
    >();
    match stage {
        // Back to the welcome screen. Everything below it goes with the account.
        Some(stage) => stage.set(crate::ui::views::welcome::Stage::Waiting),
        None => use_window().close(),
    }
}

/// How far the sync mark turns between frames, in degrees.
const SPIN_STEP: f64 = 12.0;

/// How often it turns.
const SPIN_EVERY: std::time::Duration = std::time::Duration::from_millis(40);

/// Reads the library again.
///
/// The mark turns while the reading runs. The style engine animates nothing on its own, so the
/// angle is a signal a timer advances and the transform reads.
#[component]
fn SyncButton() -> impl IntoView {
    let app = ui();
    let state = app.state;

    let syncing = Signal::derive_local(move || state.syncing.get());
    let angle = RwSignal::new_local(0.0_f64);

    let spinning = set_interval(SPIN_EVERY, move || {
        if state.syncing.get_untracked() {
            angle.update(|at| *at = (*at + SPIN_STEP) % 360.0);
        } else if angle.get_untracked() != 0.0 {
            // Come to rest upright rather than wherever the last frame left it.
            angle.set(0.0);
        }
    });
    on_cleanup_local(move || drop(spinning));

    view! {
        Button(
            variant = ButtonVariant::Ghost,
            size = ButtonSize::IconSm,
            disabled = syncing,
            class = "top__sync",
            on:click = {
                let app = app.clone();
                move |_| app.state.sync(&app.services)
            }
        ) {
            box(
                class = "top__spin",
                style:transform = move || Some(format!("rotate({}deg)", angle.get()))
            ) {
                Icon(svg = art::SYNC, size = IconSize::Sm, label = "Sync with Spotify")
            }
        }
    }
}

/// Who is signed in, and what they may do about it.
#[component]
fn AccountMenu() -> impl IntoView {
    let app = ui();
    let state = app.state;
    let name = Signal::derive_local(move || {
        let who = state.who.get();
        if who.is_empty() {
            "Account".to_owned()
        } else {
            crate::ui::format::truncate(&who, 18).into_owned()
        }
    });

    let sign_out = std::rc::Rc::new({
        let app = app.clone();
        move |()| forget(&app)
    });

    view! {
        DropdownMenu {
            DropdownMenuTrigger(variant = ButtonVariant::Ghost, size = ButtonSize::Sm) {
                Icon(svg = art::USER, size = IconSize::Xs)
                text(class = "top__who") {{move || name.get()}}
            }
            DropdownMenuContent {
                MenuItem(
                    destructive = true,
                    on_select = UnsyncCallback::new({
                        let sign_out = sign_out.clone();
                        move |()| sign_out(())
                    })
                ) {
                    Icon(svg = art::SIGN_OUT, size = IconSize::Xs)
                    "Log out"
                }
            }
        }
    }
}

/// Minimise, maximise and close.
///
/// Absent where the desktop draws its own, which on macOS is the three buttons at the top left.
#[component]
pub fn WindowControls() -> impl IntoView {
    let window = use_window();
    let maximised = window.maximized();

    own_frame(view! {
        row(class = "wctl") {
            control(
                class = "wctl__key",
                tabindex = Focus::Sequential,
                a11y:label = "Minimise",
                on:click = {
                    let window = window.clone();
                    move |_| window.minimize()
                }
            ) {
                Icon(svg = art::WINDOW_MINIMISE, size = IconSize::Xs)
            }
            control(
                class = "wctl__key",
                tabindex = Focus::Sequential,
                a11y:label = "Maximise",
                on:click = {
                    let window = window.clone();
                    move |_| window.toggle_maximized()
                }
            ) {
                Icon(
                    svg = Signal::derive_local(move || {
                        if maximised.get() {
                            art::WINDOW_RESTORE
                        } else {
                            art::WINDOW_MAXIMISE
                        }
                    }),
                    size = IconSize::Xs
                )
            }
            control(
                class = "wctl__key wctl__key-close",
                tabindex = Focus::Sequential,
                a11y:label = "Close",
                on:click = {
                    let window = window.clone();
                    move |_| window.close()
                }
            ) {
                Icon(svg = art::WINDOW_CLOSE, size = IconSize::Xs)
            }
        }
    })
}

/// The strips along the window's edges that resize it.
///
/// Absent where the desktop keeps its frame, which carries edges of its own.
#[component]
pub fn ResizeEdges() -> impl IntoView {
    let window = use_window();

    let edges = [
        (ResizeEdge::North, "edge edge-n"),
        (ResizeEdge::South, "edge edge-s"),
        (ResizeEdge::West, "edge edge-w"),
        (ResizeEdge::East, "edge edge-e"),
        (ResizeEdge::NorthWest, "edge edge-nw"),
        (ResizeEdge::NorthEast, "edge edge-ne"),
        (ResizeEdge::SouthWest, "edge edge-sw"),
        (ResizeEdge::SouthEast, "edge edge-se"),
    ]
    .into_iter()
    .map(|(edge, class)| {
        let window = window.clone();
        view! { box(class = class, on:pointer_down = window.resize_drag_handler(edge)) }.any()
    })
    .collect::<Vec<_>>();

    own_frame(view! { box(class = "edges") {{edges}} })
}

/// What the main area shows.
#[component]
fn View() -> impl IntoView {
    let router = ui().router;

    view! {
        {move || match router.current() {
            Route::Liked => view! { Liked() }.any(),
            Route::Playlist(id) => view! { PlaylistView(id = id) }.any(),
            Route::Album(id) => view! { AlbumView(id = id) }.any(),
            Route::Artist(id) => view! { ArtistView(id = id) }.any(),
            Route::Radio(seed) => view! { RadioView(seed = seed) }.any(),
            Route::Search => view! { SearchView() }.any(),
        }}
    }
}
