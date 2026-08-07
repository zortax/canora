//! The icons the interface draws.
//!
//! Each constant is one SVG document from `assets/icons`, compiled in. The set is Phosphor, in
//! the filled style. A document paints with `currentColor`, so an icon takes the colour of
//! whatever holds it.
//!
//! The heart comes in both styles. A saved track fills it; an unsaved one leaves it open.
//!
//! Keep the number of icons on the screen low. The vector rasteriser plans one pass per document,
//! and a frame that plans more than 64 passes draws nothing at all.

use zgui::prelude::*;
use zgui::reactive::LocalStorage;
use zgui::{component, view};

use crate::ui::Erase;

/// Names one icon after the file it is drawn from.
macro_rules! icons {
    ($($name:ident => $file:literal,)*) => {
        $(
            #[doc = concat!("The `", $file, "` icon.")]
            pub const $name: &str = include_str!(concat!("../../assets/icons/", $file, ".svg"));
        )*
    };
}

/// Icon source text, from `assets/icons`.
pub mod art {
    icons! {
        PLAY => "play",
        PAUSE => "pause",
        SKIP_BACK => "skip-back",
        SKIP_FORWARD => "skip-forward",
        SHUFFLE => "shuffle",
        REPEAT => "repeat",
        REPEAT_ONE => "repeat-one",
        VOLUME_MUTE => "volume-mute",
        VOLUME_LOW => "volume-low",
        VOLUME_HIGH => "volume-high",
        QUEUE => "queue",
        HEART => "heart",
        HEART_LINE => "heart-line",
        CHEVRON_LEFT => "chevron-left",
        CHEVRON_RIGHT => "chevron-right",
        SEARCH => "search",
        PLUS => "plus",
        ELLIPSIS => "ellipsis",
        RADIO => "radio",
        ADD_TO_PLAYLIST => "add-to-playlist",
        ARTIST => "artist",
        ALBUM => "album",
        RENAME => "rename",
        REMOVE => "remove",
        SIGN_OUT => "sign-out",
        SYNC => "sync",
        OFFLINE => "offline",
        SUN => "sun",
        MOON => "moon",
        MONITOR => "monitor",
        WINDOW_MINIMISE => "window-minimise",
        WINDOW_MAXIMISE => "window-maximise",
        WINDOW_RESTORE => "window-restore",
        WINDOW_CLOSE => "window-close",
        MUSIC => "music",
        WAVE => "wave",
        USER => "user",
    }
}

/// How large an icon is drawn.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum IconSize {
    /// 12px. For a dense row.
    Xs,
    /// 14px. The size of an icon beside text.
    #[default]
    Sm,
    /// 16px. For a control that stands alone.
    Md,
}

impl IconSize {
    /// The class this size is drawn by.
    const fn class(self) -> &'static str {
        match self {
            Self::Xs => "icon icon-xs",
            Self::Sm => "icon icon-sm",
            Self::Md => "icon icon-md",
        }
    }
}

/// One icon.
///
/// Give a label to an icon that carries meaning of its own. Leave it out for an icon beside text
/// that says the same thing, and the icon stays out of the accessibility tree.
#[component]
pub fn Icon(
    /// The document to draw.
    #[prop(into)]
    svg: Signal<&'static str, LocalStorage>,
    /// How large to draw it.
    #[prop(default = IconSize::Sm)]
    size: IconSize,
    /// What it is, for a reader.
    #[prop(into, optional)]
    label: Option<String>,
) -> impl IntoView {
    match label {
        Some(label) => view! {
            vector(
                class = size.class(),
                prop:svg = move || zgui::vocab::PropValue::from(svg.get()),
                a11y:label = label
            )
        }
        .any(),
        None => view! {
            vector(
                class = size.class(),
                prop:svg = move || zgui::vocab::PropValue::from(svg.get()),
                a11y:hidden = true
            )
        }
        .any(),
    }
}
