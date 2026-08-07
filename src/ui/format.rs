//! Text the interface shows.
//!
//! The style engine has no way to cut a line that is too long, so a line is cut here. Each budget
//! below is a number of characters that fits its column at the interface's type size.

use std::borrow::Cow;
use std::time::Duration;

/// How many characters a track title gets in a list.
pub const TITLE: usize = 48;

/// How many an artist gets beside a title.
pub const ARTIST: usize = 40;

/// How many a name gets in the sidebar.
pub const SIDEBAR: usize = 22;

/// How many a name gets in the player bar.
pub const BAR: usize = 28;

/// How many a row of the queue gets.
pub const QUEUE: usize = 34;

/// How many a name gets under a cover in a grid.
pub const CARD: usize = 16;

/// `text`, cut to `budget` characters.
///
/// Add an ellipsis to a line that was cut. Count characters, because a byte cut breaks a letter.
#[must_use]
pub fn truncate(text: &str, budget: usize) -> Cow<'_, str> {
    if text.chars().count() <= budget {
        return Cow::Borrowed(text);
    }
    let kept: String = text.chars().take(budget.saturating_sub(1)).collect();
    Cow::Owned(format!("{}…", kept.trim_end()))
}

/// A length, as minutes and seconds.
#[must_use]
pub fn duration(value: Duration) -> String {
    let total = value.as_secs();
    let minutes = total / 60;
    let seconds = total % 60;
    if minutes >= 60 {
        format!("{}:{:02}:{seconds:02}", minutes / 60, minutes % 60)
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// How long a list of tracks runs, in words.
#[must_use]
pub fn total_time(value: Duration) -> String {
    let minutes = value.as_secs() / 60;
    if minutes >= 60 {
        let hours = minutes / 60;
        let rest = minutes % 60;
        if rest == 0 {
            format!("{hours} hr")
        } else {
            format!("{hours} hr {rest} min")
        }
    } else {
        format!("{minutes} min")
    }
}

/// How many tracks a list holds, and how long it runs.
#[must_use]
pub fn summary(count: u32, length: Duration) -> String {
    let songs = if count == 1 { "song" } else { "songs" };
    if length.is_zero() {
        format!("{count} {songs}")
    } else {
        format!("{count} {songs} · {}", total_time(length))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_a_short_line_whole() {
        assert_eq!(truncate("Paris", 10), "Paris");
        assert_eq!(truncate("exactly10!", 10), "exactly10!");
    }

    #[test]
    fn cuts_a_long_line_and_marks_it() {
        let cut = truncate("Invisible - Piano Version", 12);
        assert_eq!(cut, "Invisible -…");
        assert_eq!(cut.chars().count(), 12);
    }

    #[test]
    fn cuts_between_letters() {
        // A byte cut would split the accented letter.
        let cut = truncate("clé des champs für alle", 6);
        assert_eq!(cut, "clé d…");
    }

    #[test]
    fn writes_a_length() {
        assert_eq!(duration(Duration::from_secs(0)), "0:00");
        assert_eq!(duration(Duration::from_secs(61)), "1:01");
        assert_eq!(duration(Duration::from_secs(200)), "3:20");
        assert_eq!(duration(Duration::from_secs(3661)), "1:01:01");
    }

    #[test]
    fn writes_a_summary() {
        assert_eq!(summary(1, Duration::ZERO), "1 song");
        assert_eq!(
            summary(42, Duration::from_secs(9060)),
            "42 songs · 2 hr 31 min"
        );
        assert_eq!(summary(3, Duration::from_secs(600)), "3 songs · 10 min");
    }
}
