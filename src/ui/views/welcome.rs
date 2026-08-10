//! What the window shows before there is an account.
//!
//! The first run has nothing on disk, so the window opens here rather than sending a browser out
//! on its own. One button starts the login. Every phase after that is reported in place, and a
//! failure leaves the button to press again.

use zgui::prelude::*;
use zgui::reactive::{LocalStorage, RwSignal};
use zgui::{component, view};
use zgui_ui::prelude::*;

use crate::auth::AuthPhase;
use crate::ui::Erase;
use crate::ui::icons::{IconProps, IconSize, art};
use crate::ui::shell::{ResizeEdgesProps, WindowControlsProps};

/// How far the window has got towards an account.
#[derive(Clone)]
pub enum Stage {
    /// Nothing on disk. Waiting for the button.
    Waiting,
    /// A login is running.
    Working(AuthPhase),
    /// It stopped, with the reason.
    Failed(String),
    /// Connected, with everything the interface talks to.
    ///
    /// The flag says whether the session is already connected. A first run signs in here and
    /// arrives connected; a later run arrives with the cache and connects behind the window.
    Ready(crate::services::Services, crate::ui::Events, bool),
}

impl Stage {
    /// Whether a login is running.
    #[must_use]
    fn busy(&self) -> bool {
        matches!(self, Self::Working(_))
    }
}

/// Starts a login and reports it into `stage`.
///
/// The whole flow runs on the interface thread. zgui enters the runtime around every poll of that
/// thread's tasks, so the blocking browser wait and every socket below it reach the runtime from
/// here.
pub fn begin(stage: RwSignal<Stage, LocalStorage>, runtime: tokio::runtime::Handle) {
    if stage.with_untracked(Stage::busy) {
        return;
    }
    stage.set(Stage::Working(AuthPhase::CheckingCache));

    let (events, receiver) = tokio::sync::mpsc::unbounded_channel();

    // Detached rather than owned by this view. Writing the stage above rebuilds the screen that
    // holds the button, and a task owned by that screen would be thrown away before its first
    // poll — the login would report one phase and then stop.
    zgui::reactive::spawn_detached(async move {
        let services = match crate::services::start(runtime, events).await {
            Ok(services) => services,
            Err(error) => {
                tracing::warn!(%error, "cannot start");
                stage.set(Stage::Failed(plainly(&error)));
                return;
            }
        };

        match services
            .link(move |phase| {
                tracing::info!(?phase, "login phase");
                stage.set(Stage::Working(phase));
            })
            .await
        {
            Ok(_) => {
                let events = std::rc::Rc::new(std::cell::RefCell::new(Some(receiver)));
                stage.set(Stage::Ready(services, events, true));
            }
            Err(error) => {
                tracing::warn!(%error, "cannot sign in");
                stage.set(Stage::Failed(plainly(&error)));
            }
        }
    });
}

/// Opens the window on the cached library, with the connection running behind it.
///
/// For a run that has a login on disk. Nothing here reaches the network: the session is built and
/// left unconnected, the library is read from the cache, and the window is shown. The connection
/// is the interface's own job from there, and a failure leaves a mark in the header rather than a
/// waiting screen.
pub fn resume(stage: RwSignal<Stage, LocalStorage>, runtime: tokio::runtime::Handle) {
    let (events, receiver) = tokio::sync::mpsc::unbounded_channel();

    zgui::reactive::spawn_detached(async move {
        match crate::services::start(runtime, events).await {
            Ok(services) => {
                let events = std::rc::Rc::new(std::cell::RefCell::new(Some(receiver)));
                stage.set(Stage::Ready(services, events, false));
            }
            Err(error) => {
                // Nothing can be shown without the pieces below the interface. Ask for the login
                // again rather than opening an empty window.
                tracing::warn!(%error, "cannot start");
                stage.set(Stage::Failed(plainly(&error)));
            }
        }
    });
}

/// The screen shown until there is an account.
#[component]
pub fn Welcome(
    /// How far the login has got.
    stage: RwSignal<Stage, LocalStorage>,
    /// What background work runs on.
    runtime: tokio::runtime::Handle,
) -> impl IntoView {
    let busy = Signal::derive_local(move || stage.with(Stage::busy));
    let note = Signal::derive_local(move || match stage.get() {
        Stage::Working(phase) => describe(&phase).to_owned(),
        Stage::Failed(reason) => reason,
        _ => String::new(),
    });
    let failed = Signal::derive_local(move || matches!(stage.get(), Stage::Failed(_)));

    let window = use_window();

    view! {
        // The window draws its own frame, so the welcome screen carries one too. Without it there
        // is no way to move or close the window before the account arrives.
        stack(class = "hello") {
            row(class = "hello__bar", on:pointer_down = window.move_drag_handler()) {
                box(class = "hello__gap")
                row(on:pointer_down = window.no_drag_handler()) {
                    WindowControls()
                }
            }

            column(class = "hello__card") {
                // The name sits in the middle of what is left. The button holds the bottom.
                column(class = "hello__say") {
                    box(class = "hello__mark") {
                        Icon(svg = art::MUSIC, size = IconSize::Md)
                    }
                    text(class = "hello__name") {"canora"}
                    text(class = "hello__line") {"A cozy player for Spotify."}
                }

                column(class = "hello__foot") {
                    {move || {
                        let note = note.get();
                        if note.is_empty() {
                            return ().any();
                        }
                        view! {
                            text(
                                class = "hello__note",
                                class:hello__note-bad = move || failed.get()
                            ) {{note}}
                        }
                        .any()
                    }}

                    Button(
                        size = ButtonSize::Sm,
                        class = "hello__go",
                        disabled = busy,
                        on:click = {
                            let runtime = runtime.clone();
                            move |_| begin(stage, runtime.clone())
                        }
                    ) {
                        {move || if busy.get() { "Signing in…" } else { "Sign in with Spotify" }}
                    }
                }
            }

            ResizeEdges()
        }
    }
}

/// What went wrong, in a sentence.
///
/// The error carries the address the browser came back to, which says nothing to the person
/// reading it. The log keeps the whole thing.
fn plainly(error: &crate::services::StartError) -> String {
    let full = error.to_string();
    if full.contains("Auth code param not found") {
        return "Spotify gave no code back. The login was refused or closed early.".to_owned();
    }
    match error {
        crate::services::StartError::Auth(_) => format!("Cannot sign in. {full}"),
        other => other.to_string(),
    }
}

/// What one phase says.
fn describe(phase: &AuthPhase) -> &'static str {
    match phase {
        AuthPhase::CheckingCache => "Looking for a login on this machine…",
        AuthPhase::WaitingForBrowser => "Finish in the browser, then come back.",
        AuthPhase::WaitingForPlayback => "Once more in the browser, to allow playback.",
        AuthPhase::Connecting => "Connecting to Spotify…",
        AuthPhase::Ready => "Connected.",
        AuthPhase::Failed(_) => "That did not work.",
    }
}
