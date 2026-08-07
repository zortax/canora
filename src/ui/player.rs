//! The bar under everything: what is playing, the transport, and how loud it is.

use std::time::Duration;

use zgui::prelude::*;
use zgui::reactive::RwSignal;
use zgui::{component, view};
use zgui_ui::prelude::*;

use crate::player::RepeatMode;
use crate::ui::art::ArtProps;
use crate::ui::queue::QueueButtonProps;
use crate::ui::format;
use crate::ui::icons::{IconProps, IconSize, art};
use crate::ui::router::Route;
use crate::ui::Erase;
use crate::ui::state::ui;

/// How often the position on the bar is redrawn while audio runs.
const TICK: Duration = Duration::from_millis(250);

/// The bar under everything.
#[component]
pub fn PlayerBar() -> impl IntoView {
    let app = ui();
    let playback = app.playback;
    let state = app.state;

    // Wake whatever draws the position, four times a second. The engine reports twice a second,
    // and the rest is read from the clock.
    spawn_local(async move {
        loop {
            tokio::time::sleep(TICK).await;
            if playback.status.get_untracked().is_playing() {
                playback.tick.update(|tick| *tick = tick.wrapping_add(1));
            }
        }
    });

    // While the pointer holds the bar, show where it holds it rather than where the track is.
    let scrub = RwSignal::new_local(Option::<f64>::None);

    let duration = Signal::derive_local(move || playback.duration().as_secs_f64().max(1.0));
    let shown = Signal::derive_local(move || {
        scrub
            .get()
            .unwrap_or_else(|| playback.position().as_secs_f64())
    });

    // The bar takes a fixed range, and a track has its own length, so the bar carries the
    // position as a part of one and the seek turns it back into a time.
    let fraction = Signal::derive_local(move || (shown.get() / duration.get()).clamp(0.0, 1.0));
    let seek = Binding::controlled(fraction, {
        let app = app.clone();
        move |part: f64| {
            let seconds = part.clamp(0.0, 1.0) * duration.get_untracked();
            scrub.set(Some(seconds));
            app.services.player.seek(Duration::from_secs_f64(seconds));
            // The engine reports the new position at once, so the hold can end here.
            scrub.set(None);
        }
    });

    let volume = Binding::controlled(
        Signal::derive_local(move || playback.volume.get()),
        {
            let app = app.clone();
            move |value: f64| app.services.player.set_volume(value)
        },
    );

    let playing = Signal::derive_local(move || playback.status.get().is_playing());
    let has_track = Signal::derive_local(move || playback.playing.with(Option::is_some));

    let liked = Binding::controlled(
        Signal::derive_local(move || {
            playback
                .current_id()
                .map(|id| state.is_liked(&id))
                .unwrap_or(false)
        }),
        {
            let app = app.clone();
            move |on: bool| {
                if let Some(id) = app.playback.current_id() {
                    app.state.set_liked(&app.services, id, on);
                }
            }
        },
    );

    let shuffle = Binding::controlled(
        Signal::derive_local(move || playback.shuffle.get()),
        {
            let app = app.clone();
            move |on: bool| app.services.player.set_shuffle(on)
        },
    );

    let volume_mark = Signal::derive_local(move || {
        let level = playback.volume.get();
        if level <= 0.001 {
            art::VOLUME_MUTE
        } else if level < 0.5 {
            art::VOLUME_LOW
        } else {
            art::VOLUME_HIGH
        }
    });

    let repeat_mark = Signal::derive_local(move || match playback.repeat.get() {
        RepeatMode::One => art::REPEAT_ONE,
        _ => art::REPEAT,
    });

    view! {
        row(class = "bar") {
            NowPlaying(liked = liked)

            column(class = "bar__mid") {
                row(class = "bar__keys") {
                    Toggle(pressed = shuffle, size = ToggleSize::Sm, a11y:label = "Shuffle") {
                        Icon(svg = art::SHUFFLE, size = IconSize::Sm)
                    }
                    Button(
                        variant = ButtonVariant::Ghost,
                        size = ButtonSize::IconSm,
                        disabled = Signal::derive_local(move || !has_track.get()),
                        on:click = {
                            let app = app.clone();
                            move |_| app.services.player.previous()
                        }
                    ) {
                        Icon(svg = art::SKIP_BACK, size = IconSize::Sm, label = "Previous")
                    }
                    Button(
                        variant = ButtonVariant::Outline,
                        size = ButtonSize::Icon,
                        class = "bar__play",
                        disabled = Signal::derive_local(move || !has_track.get()),
                        on:click = {
                            let app = app.clone();
                            move |_| app.services.player.toggle_play()
                        }
                    ) {
                        {move || {
                            let svg = if playing.get() { art::PAUSE } else { art::PLAY };
                            let label = if playing.get() { "Pause" } else { "Play" };
                            view! { Icon(svg = svg, size = IconSize::Md, label = label) }.any()
                        }}
                    }
                    Button(
                        variant = ButtonVariant::Ghost,
                        size = ButtonSize::IconSm,
                        disabled = Signal::derive_local(move || !has_track.get()),
                        on:click = {
                            let app = app.clone();
                            move |_| app.services.player.next()
                        }
                    ) {
                        Icon(svg = art::SKIP_FORWARD, size = IconSize::Sm, label = "Next")
                    }
                    Button(
                        variant = ButtonVariant::Ghost,
                        size = ButtonSize::IconSm,
                        class = "bar__rep",
                        class:bar__rep-on = move || playback.repeat.get() != RepeatMode::Off,
                        on:click = {
                            let app = app.clone();
                            move |_| {
                                let next = app.playback.repeat.get_untracked().next();
                                app.services.player.set_repeat(next);
                            }
                        }
                    ) {
                        Icon(svg = repeat_mark, size = IconSize::Sm, label = "Repeat")
                    }
                }
                row(class = "bar__seek") {
                    text(class = "bar__time") {{
                        move || format::duration(Duration::from_secs_f64(shown.get()))
                    }}
                    Slider(
                        value = seek,
                        min = 0.0,
                        max = 1.0,
                        step = 0.001,
                        class = "bar__bar",
                        a11y:label = "Seek"
                    )
                    text(class = "bar__time") {{move || format::duration(playback.duration())}}
                }
            }

            row(class = "bar__right") {
                QueueButton()
                Icon(svg = volume_mark, size = IconSize::Sm, label = "Volume")
                Slider(
                    value = volume,
                    min = 0.0,
                    max = 1.0,
                    step = 0.02,
                    class = "bar__vol",
                    a11y:label = "Volume"
                )
            }
        }
    }
}

/// What is playing, on the left of the bar.
#[component]
fn NowPlaying(
    /// Whether the track that is playing is saved.
    liked: Binding<bool>,
) -> impl IntoView {
    let app = ui();
    let playback = app.playback;
    let router = app.router;

    let images = Signal::derive_local(move || {
        playback.playing.with(|playing| {
            playing
                .as_ref()
                .and_then(|it| it.track.album.as_ref())
                .map(|album| album.images.clone())
                .unwrap_or_default()
        })
    });

    let title = Signal::derive_local(move || {
        playback.playing.with(|playing| {
            playing
                .as_ref()
                .map(|it| format::truncate(&it.track.name, format::BAR).into_owned())
                .unwrap_or_default()
        })
    });

    let artist = Signal::derive_local(move || {
        playback.playing.with(|playing| {
            playing
                .as_ref()
                .map(|it| format::truncate(&it.track.artist_line(), format::BAR).into_owned())
                .unwrap_or_default()
        })
    });

    let has_track = Signal::derive_local(move || playback.playing.with(Option::is_some));

    view! {
        row(class = "bar__now") {
            Art(images = images, class = "art art-bar", want = 64)
            column(class = "bar__meta") {
                control(
                    class = "bar__title",
                    tabindex = Focus::Sequential,
                    on:click = move |_| {
                        if let Some(album) = playback
                            .playing
                            .with(|p| p.as_ref().and_then(|it| it.track.album.clone()))
                        {
                            router.go(Route::Album(album.id));
                        }
                    }
                ) {{move || title.get()}}
                control(
                    class = "bar__artist",
                    tabindex = Focus::Sequential,
                    on:click = move |_| {
                        if let Some(artist) = playback
                            .playing
                            .with(|p| p.as_ref().and_then(|it| it.track.artists.first().cloned()))
                        {
                            router.go(Route::Artist(artist.id));
                        }
                    }
                ) {{move || artist.get()}}
            }
            {move || {
                if has_track.get() {
                    view! {
                        Toggle(
                            pressed = liked,
                            size = ToggleSize::Sm,
                            class = "bar__like",
                            a11y:label = "Save to your library"
                        ) {
                            Icon(svg = art::HEART, size = IconSize::Xs)
                        }
                    }
                    .any()
                } else {
                    ().any()
                }
            }}
        }
    }
}
