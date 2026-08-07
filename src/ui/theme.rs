//! The themes the interface offers.
//!
//! A theme is a block of custom-property declarations laid over the base token set. Each one is a
//! file under `assets/themes`, one for a light surface and one for a dark surface, compiled in.
//!
//! Adding a theme is two files and one line in the table below.

use serde::{Deserialize, Serialize};
use zgui::prelude::*;
use zgui::reactive::{LocalStorage, RwSignal};
use zgui::{component, view};
use zgui_ui::prelude::*;
use zgui_ui_tokens::prelude::*;

use crate::ui::Erase;
use crate::ui::icons::{IconProps, IconSize, art};

/// Declares the themes and reads their two files.
macro_rules! themes {
    ($($name:ident => ($file:literal, $label:literal),)*) => {
        /// A theme the interface offers.
        #[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
        #[serde(rename_all = "lowercase")]
        pub enum Variant {
            $(
                #[doc = concat!("The ", $label, " theme.")]
                $name,
            )*
        }

        impl Variant {
            /// Every theme, in the order they are offered.
            pub const ALL: &'static [Self] = &[$(Self::$name),*];

            /// How this is written in the settings file.
            #[must_use]
            pub const fn name(self) -> &'static str {
                match self {
                    $(Self::$name => $file,)*
                }
            }

            /// What it is called in the interface.
            #[must_use]
            pub const fn label(self) -> &'static str {
                match self {
                    $(Self::$name => $label,)*
                }
            }

            /// The declarations it lays over the light token set.
            const fn light_css(self) -> &'static str {
                match self {
                    $(
                        Self::$name => include_str!(
                            concat!("../../assets/themes/", $file, "-light.css")
                        ),
                    )*
                }
            }

            /// The declarations it lays over the dark token set.
            const fn dark_css(self) -> &'static str {
                match self {
                    $(
                        Self::$name => include_str!(
                            concat!("../../assets/themes/", $file, "-dark.css")
                        ),
                    )*
                }
            }
        }
    };
}

themes! {
    Slate => ("slate", "Slate"),
    Graphite => ("graphite", "Graphite"),
    Moss => ("moss", "Moss"),
    Violet => ("violet", "Violet"),
    Ocean => ("ocean", "Ocean"),
    Ember => ("ember", "Ember"),
    Rose => ("rose", "Rose"),
    Sand => ("sand", "Sand"),
    Mint => ("mint", "Mint"),
    Onyx => ("onyx", "Onyx"),
}

impl Default for Variant {
    /// Slate: near-monochrome, and the quietest of them.
    fn default() -> Self {
        Self::Slate
    }
}

impl Variant {
    /// The theme written as `name`, if there is one.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|it| it.name() == name)
    }

    /// This theme as the one for a light surface.
    #[must_use]
    pub fn light(self) -> Theme {
        Theme::light().with_css(self.light_css())
    }

    /// This theme as the one for a dark surface.
    #[must_use]
    pub fn dark(self) -> Theme {
        Theme::dark().with_css(self.dark_css())
    }
}

/// The menu that picks the scheme and the theme.
#[component]
pub fn ThemeMenu(
    /// Which scheme the interface presents in.
    scheme: RwSignal<ColorScheme, LocalStorage>,
    /// Which theme it uses.
    variant: RwSignal<Variant, LocalStorage>,
) -> impl IntoView {
    let mark = Signal::derive_local(move || match scheme.get() {
        ColorScheme::Light => art::SUN,
        ColorScheme::Dark => art::MOON,
        _ => art::MONITOR,
    });

    // Build the entries on every open. A view cannot be copied, so one list cannot be handed
    // out twice.
    let schemes = move || {
        [
            (ColorScheme::System, "System"),
            (ColorScheme::Light, "Light"),
            (ColorScheme::Dark, "Dark"),
        ]
        .into_iter()
        .map(|(value, label)| {
            let checked = Binding::controlled(
                Signal::derive_local(move || scheme.get() == value),
                move |on: bool| {
                    if on {
                        scheme.set(value);
                    }
                },
            );
            view! { MenuCheckboxItem(checked = checked) {{label}} }.any()
        })
        .collect::<Vec<_>>()
    };

    let variants = move || {
        Variant::ALL
            .iter()
            .copied()
            .map(|value| {
                let checked = Binding::controlled(
                    Signal::derive_local(move || variant.get() == value),
                    move |on: bool| {
                        if on {
                            variant.set(value);
                        }
                    },
                );
                view! { MenuCheckboxItem(checked = checked) {{value.label()}} }.any()
            })
            .collect::<Vec<_>>()
    };

    view! {
        DropdownMenu {
            DropdownMenuTrigger(variant = ButtonVariant::Ghost, size = ButtonSize::IconSm) {
                Icon(svg = mark, size = IconSize::Sm, label = "Appearance")
            }
            DropdownMenuContent {
                MenuLabel {"Appearance"}
                {schemes}
                MenuSeparator()
                MenuLabel {"Theme"}
                {variants}
            }
        }
    }
}
