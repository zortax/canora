//! The interface.

pub mod art;
pub mod format;
pub mod icons;
pub mod motion;
pub mod player;
pub mod queue;
pub mod router;
pub mod shell;
pub mod sidebar;
pub mod state;
pub mod style;
pub mod theme;
pub mod track;
pub mod views;

use std::rc::Rc;

use zgui::prelude::*;
use zgui::reactive::{RenderEffect, RwSignal};
use zgui::{component, view};
use zgui_ui::prelude::*;
use zgui_ui_tokens::prelude::*;

use crate::config::Settings;
use crate::player::state::PlaybackStore;
use crate::ui::router::Router;
use crate::ui::shell::ShellProps;
use crate::ui::state::{AppState, Ui};
use crate::ui::theme::Variant;

/// The playback events the interface has not read yet.
///
/// The engine fills this from its own thread. The root takes it once and drains it.
pub type Events =
    Rc<std::cell::RefCell<Option<tokio::sync::mpsc::UnboundedReceiver<crate::player::PlaybackEvent>>>>;

/// Erases a view.
///
/// Two branches of a choice build different views. This makes them one type, which is what a
/// reactive hole needs.
pub trait Erase {
    /// This view, with its type forgotten.
    fn any(self) -> zgui::view::AnyView;
}

impl<V: IntoView> Erase for V {
    fn any(self) -> zgui::view::AnyView {
        zgui::view::AnyView::new(self)
    }
}

/// The window root.
///
/// The window opens before there is an account, so a first run has something to look at while it
/// signs in. The theme is set up here rather than below it, because the welcome screen wears it
/// too.
#[component]
pub fn Root(
    /// What background work runs on.
    runtime: tokio::runtime::Handle,
) -> impl IntoView {
    use crate::ui::views::welcome::{Stage, WelcomeProps, resume};

    let settings = Settings::load().unwrap_or_default();
    let scheme = RwSignal::new_local(match settings.scheme.as_str() {
        "light" => ColorScheme::Light,
        "dark" => ColorScheme::Dark,
        _ => ColorScheme::System,
    });
    let variant = RwSignal::new_local(
        Variant::from_name(&settings.variant).unwrap_or_default(),
    );

    // Keep the choice for the next run.
    let watching = RenderEffect::new(move |_| {
        let settings = Settings {
            scheme: match scheme.get() {
                ColorScheme::Light => "light",
                ColorScheme::Dark => "dark",
                _ => "system",
            }
            .to_owned(),
            variant: variant.get().name().to_owned(),
        };
        if let Err(error) = settings.save() {
            tracing::warn!(%error, "cannot keep the settings");
        }
    });
    on_cleanup_local(move || drop(watching));

    let stage = RwSignal::new_local(Stage::Waiting);
    // Signing out from the account menu puts the window back on the welcome screen.
    provide_local_context(stage);

    // A login already on disk shows no welcome screen at all. Everything is built from the cache
    // and the window opens on the library, with the connection running behind it.
    if crate::auth::is_signed_in() {
        resume(stage, runtime.clone());
    }

    view! {
        ThemeProvider(
            scheme = scheme,
            light = Signal::derive_local(move || variant.get().light()),
            dark = Signal::derive_local(move || variant.get().dark())
        ) {
            Toaster {
                {move || match stage.get() {
                    Stage::Ready(services, events, linked) => {
                        view! {
                            App(
                                services = services,
                                events = events,
                                linked = linked,
                                scheme = scheme,
                                variant = variant
                            )
                        }
                        .any()
                    }
                    _ => view! { Welcome(stage = stage, runtime = runtime.clone()) }.any(),
                }}
            }
        }
    }
}

/// The interface, with an account in hand.
#[component]
pub fn App(
    /// What the interface talks to.
    services: crate::services::Services,
    /// Playback events, as the engine reports them.
    events: Events,
    /// Whether the session is already connected.
    linked: bool,
    /// Light, dark or whatever the desktop says.
    scheme: RwSignal<ColorScheme, zgui::reactive::LocalStorage>,
    /// Which set of colours.
    variant: RwSignal<Variant, zgui::reactive::LocalStorage>,
) -> impl IntoView {
    let playback = PlaybackStore::new();
    let state = AppState::new();
    provide_local_context(Rc::new(Ui {
        services: services.clone(),
        state,
        router: Router::new(),
        playback,
    }));

    // A first run signs in on the welcome screen and arrives here connected. Every later run
    // arrives with the cache on the screen and connects behind it.
    if linked {
        state.link.set(crate::ui::state::Link::Live);
    } else {
        state.link_up(&services);
    }

    // Every playback event is written here, on the interface thread.
    if let Some(mut events) = events.borrow_mut().take() {
        spawn_local(async move {
            while let Some(event) = events.recv().await {
                // The header answers an outage. It shows the mark that a failed login also
                // shows, and a press on that mark asks for the connection at once.
                if let crate::player::PlaybackEvent::Connection(connection) = event {
                    state.link.set(match connection {
                        crate::player::events::Connection::Lost => {
                            crate::ui::state::Link::Broken("The connection to Spotify went".into())
                        }
                        crate::player::events::Connection::Restored => {
                            crate::ui::state::Link::Live
                        }
                    });
                }
                playback.apply(event);
            }
        });
    }

    view! {
        Shell(scheme = scheme, variant = variant)
    }
}

