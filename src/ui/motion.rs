//! Movement the style engine will not make.
//!
//! zgui reads `transition` and `animation` off a sheet and acts on neither: a value arrives at its
//! destination on the frame it changes. Anything that has to travel is a signal a timer advances,
//! and the view reads that signal as a length, an angle or a strength.

use std::time::Duration;

use zgui::prelude::*;
use zgui::reactive::{LocalStorage, RwSignal};

/// How often a travelling value is moved on.
const STEP: Duration = Duration::from_millis(16);

/// A value on its way from 0 to 1 and back.
///
/// The result follows `open`: it climbs to 1 while `open` is true and falls to 0 while it is
/// false, taking `span` to cross. It starts already arrived, so a folder the sidebar draws open
/// does not swing open in front of the reader.
///
/// The path is eased at both ends. A straight ramp starts and stops abruptly enough to read as a
/// jump with a delay in the middle.
#[must_use]
pub fn travel(open: Signal<bool, LocalStorage>, span: Duration) -> Signal<f64, LocalStorage> {
    let at = RwSignal::new_local(if open.get_untracked() { 1.0_f64 } else { 0.0 });
    let step = STEP.as_secs_f64() / span.as_secs_f64().max(f64::EPSILON);

    // One timer, running while the view holds this value. It writes only while the value moves,
    // so a folder at rest costs a comparison per frame and wakes nothing.
    let timer = set_interval(STEP, move || {
        let target = if open.get() { 1.0 } else { 0.0 };
        let now = at.get_untracked();
        if (now - target).abs() < f64::EPSILON {
            return;
        }
        let next = if target > now {
            (now + step).min(1.0)
        } else {
            (now - step).max(0.0)
        };
        at.set(next);
    });
    on_cleanup_local(move || drop(timer));

    Signal::derive_local(move || ease(at.get()))
}

/// `at` slowed at both ends of its path.
fn ease(at: f64) -> f64 {
    if at < 0.5 {
        2.0 * at * at
    } else {
        1.0 - (-2.0 * at + 2.0).powi(2) / 2.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_path_keeps_its_ends() {
        assert!((ease(0.0) - 0.0).abs() < 1e-9);
        assert!((ease(1.0) - 1.0).abs() < 1e-9);
        assert!((ease(0.5) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn the_path_only_climbs() {
        let mut last = 0.0;
        for step in 0..=100 {
            let now = ease(f64::from(step) / 100.0);
            assert!(now >= last, "{now} came after {last}");
            last = now;
        }
    }

    #[test]
    fn the_ends_are_slower_than_the_middle() {
        // What makes the movement read as movement rather than as a jump with a pause in it.
        let start = ease(0.1) - ease(0.0);
        let middle = ease(0.55) - ease(0.45);
        assert!(middle > start);
    }
}
