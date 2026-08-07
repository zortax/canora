//! What plays, and what plays next.
//!
//! The queue holds the tracks of one context, the order they play in, and the tracks a person put
//! next by hand. Shuffle replaces the order and keeps the current track where it is. Nothing here
//! talks to Spotify, so every rule below is decided by a test.

use std::collections::VecDeque;

use rand::seq::SliceRandom as _;

use crate::models::{AlbumId, PlaylistId, Track, TrackId};

/// Where the tracks in the queue came from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PlayContext {
    /// One playlist.
    Playlist(PlaylistId),
    /// One album.
    Album(AlbumId),
    /// The saved tracks.
    Liked,
    /// A station built around one track.
    Radio {
        /// The track it was built around.
        seed: TrackId,
    },
    /// Tracks with no list behind them.
    #[default]
    Adhoc,
}

/// What happens at the end of the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RepeatMode {
    /// Stop.
    #[default]
    Off,
    /// Start again from the top.
    All,
    /// Play the same track again.
    One,
}

impl RepeatMode {
    /// The mode after this one, for a control that steps through them.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::All,
            Self::All => Self::One,
            Self::One => Self::Off,
        }
    }
}

/// The tracks in play, and the order they play in.
#[derive(Debug, Default)]
pub struct Queue {
    /// Every track of the context, in the order the context lists them.
    tracks: Vec<Track>,
    /// Where the tracks came from.
    context: PlayContext,
    /// Which entry of `order` is playing.
    current: Option<usize>,
    /// Positions into `tracks`. This is the play order.
    order: Vec<usize>,
    /// Tracks to play before the order continues.
    manual: VecDeque<Track>,
    /// Whether the order is shuffled.
    shuffle: bool,
    /// What happens at the end.
    repeat: RepeatMode,
}

impl Queue {
    /// Replaces everything with `tracks`, playing the one at `start`.
    pub fn replace(&mut self, tracks: Vec<Track>, start: usize, context: PlayContext) {
        self.order = (0..tracks.len()).collect();
        self.tracks = tracks;
        self.context = context;
        self.manual.clear();
        self.current = (start < self.tracks.len()).then_some(start);

        if self.shuffle {
            self.reshuffle();
        }
    }

    /// The track playing now.
    #[must_use]
    pub fn current(&self) -> Option<&Track> {
        let position = *self.order.get(self.current?)?;
        self.tracks.get(position)
    }

    /// Where the current track sits in the play order, and how long the order is.
    #[must_use]
    pub fn position(&self) -> (usize, usize) {
        (self.current.unwrap_or(0), self.order.len())
    }

    /// Where the tracks came from.
    #[must_use]
    #[allow(dead_code, reason = "the queue panel will name where the tracks came from")]
    pub fn context(&self) -> &PlayContext {
        &self.context
    }

    /// Whether the order is shuffled.
    #[must_use]
    #[allow(dead_code, reason = "the engine reports the state; this reads it back")]
    pub fn shuffle(&self) -> bool {
        self.shuffle
    }

    /// What happens at the end.
    #[must_use]
    pub fn repeat(&self) -> RepeatMode {
        self.repeat
    }

    /// The next few tracks, for the queue panel.
    #[must_use]
    pub fn upcoming(&self, count: usize) -> Vec<Track> {
        let mut out: Vec<Track> = self.manual.iter().take(count).cloned().collect();
        let start = self.current.map_or(0, |index| index + 1);
        for entry in self.order.iter().skip(start) {
            if out.len() >= count {
                break;
            }
            if let Some(track) = self.tracks.get(*entry) {
                out.push(track.clone());
            }
        }
        out
    }

    /// Puts a track next, before the order continues.
    pub fn play_next(&mut self, track: Track) {
        self.manual.push_back(track);
    }

    /// Moves to the next track and returns it.
    ///
    /// `automatic` marks the end of a track, rather than a person asking. Repeat-one repeats only
    /// for the first kind: a person who presses next wants a different track.
    pub fn next(&mut self, automatic: bool) -> Option<Track> {
        if automatic
            && self.repeat == RepeatMode::One
            && let Some(track) = self.current()
        {
            return Some(track.clone());
        }

        if let Some(track) = self.manual.pop_front() {
            // A track played by hand leaves the order where it was, so the list carries on after.
            return Some(track);
        }

        let next = self.current.map_or(0, |index| index + 1);
        if next < self.order.len() {
            self.current = Some(next);
            return self.current().cloned();
        }

        if self.repeat == RepeatMode::All && !self.order.is_empty() {
            self.current = Some(0);
            return self.current().cloned();
        }

        None
    }

    /// Moves to the previous track and returns it.
    pub fn previous(&mut self) -> Option<Track> {
        let current = self.current?;
        if current == 0 {
            if self.repeat == RepeatMode::All && !self.order.is_empty() {
                self.current = Some(self.order.len() - 1);
            }
            return self.current().cloned();
        }
        self.current = Some(current - 1);
        self.current().cloned()
    }

    /// Plays the entry at `index` of the play order.
    #[allow(dead_code, reason = "playing a row replaces the queue instead, for now")]
    pub fn jump_to(&mut self, index: usize) -> Option<Track> {
        if index >= self.order.len() {
            return None;
        }
        self.current = Some(index);
        self.current().cloned()
    }

    /// Turns shuffle on or off.
    ///
    /// The track playing keeps playing. Turning shuffle on moves it to the front of a new random
    /// order. Turning it off puts the list back and finds the track again.
    pub fn set_shuffle(&mut self, on: bool) {
        if self.shuffle == on {
            return;
        }
        self.shuffle = on;

        let playing = self.current.and_then(|index| self.order.get(index)).copied();
        if on {
            self.reshuffle();
        } else {
            self.order = (0..self.tracks.len()).collect();
        }

        self.current = playing.and_then(|position| {
            self.order.iter().position(|entry| *entry == position)
        });
    }

    /// Sets what happens at the end.
    pub fn set_repeat(&mut self, repeat: RepeatMode) {
        self.repeat = repeat;
    }

    /// Builds a random order with the current track first.
    fn reshuffle(&mut self) {
        let playing = self.current.and_then(|index| self.order.get(index)).copied();
        let mut rest: Vec<usize> = (0..self.tracks.len())
            .filter(|position| Some(*position) != playing)
            .collect();
        rest.shuffle(&mut rand::rng());

        self.order = match playing {
            Some(position) => {
                let mut order = Vec::with_capacity(self.tracks.len());
                order.push(position);
                order.extend(rest);
                order
            }
            None => rest,
        };
        self.current = playing.map(|_| 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A track called `name`, for a test.
    fn track(name: &str) -> Track {
        Track {
            id: TrackId(name.to_owned()),
            name: name.to_owned(),
            artists: Vec::new(),
            album: None,
            duration: Duration::from_secs(180),
            explicit: false,
            playable: true,
            track_number: 1,
        }
    }

    /// A queue of `count` tracks, playing the first.
    fn queue(count: usize) -> Queue {
        let tracks = (0..count).map(|i| track(&i.to_string())).collect();
        let mut queue = Queue::default();
        queue.replace(tracks, 0, PlayContext::Adhoc);
        queue
    }

    #[test]
    fn plays_in_order() {
        let mut queue = queue(3);
        assert_eq!(queue.current().map(|t| t.name.clone()), Some("0".into()));
        assert_eq!(queue.next(false).map(|t| t.name), Some("1".into()));
        assert_eq!(queue.next(false).map(|t| t.name), Some("2".into()));
        assert_eq!(queue.next(false), None, "the end stops the queue");
    }

    #[test]
    fn repeat_all_wraps_around() {
        let mut queue = queue(2);
        queue.set_repeat(RepeatMode::All);
        assert_eq!(queue.next(false).map(|t| t.name), Some("1".into()));
        assert_eq!(queue.next(false).map(|t| t.name), Some("0".into()));
    }

    #[test]
    fn repeat_one_holds_only_at_the_end_of_a_track() {
        let mut queue = queue(3);
        queue.set_repeat(RepeatMode::One);
        assert_eq!(
            queue.next(true).map(|t| t.name),
            Some("0".into()),
            "a track that ends plays again"
        );
        assert_eq!(
            queue.next(false).map(|t| t.name),
            Some("1".into()),
            "a person who presses next gets the next track"
        );
    }

    #[test]
    fn a_manual_track_plays_before_the_order_continues() {
        let mut queue = queue(3);
        queue.play_next(track("guest"));

        assert_eq!(queue.next(false).map(|t| t.name), Some("guest".into()));
        assert_eq!(
            queue.next(false).map(|t| t.name),
            Some("1".into()),
            "the list carries on where it was"
        );
    }

    #[test]
    fn previous_stops_at_the_first_track() {
        let mut queue = queue(3);
        assert_eq!(queue.previous().map(|t| t.name), Some("0".into()));
        assert_eq!(queue.position().0, 0);
    }

    #[test]
    fn shuffle_keeps_the_track_that_is_playing() {
        let mut queue = queue(20);
        queue.jump_to(7);
        let playing = queue.current().map(|t| t.name.clone());

        queue.set_shuffle(true);
        assert_eq!(queue.current().map(|t| t.name.clone()), playing);
        assert_eq!(queue.position().0, 0, "the playing track leads the order");
        assert_eq!(queue.position().1, 20, "every track is still in the queue");

        queue.set_shuffle(false);
        assert_eq!(queue.current().map(|t| t.name.clone()), playing);
        assert_eq!(queue.position().0, 7, "the list order is back");
    }

    #[test]
    fn shuffle_holds_every_track_once() {
        let mut queue = queue(50);
        queue.set_shuffle(true);

        let mut seen: Vec<String> = Vec::new();
        while let Some(track) = queue.current() {
            seen.push(track.name.clone());
            if queue.next(false).is_none() {
                break;
            }
        }
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 50);
    }

    #[test]
    fn upcoming_shows_manual_tracks_first() {
        let mut queue = queue(5);
        queue.play_next(track("guest"));

        let upcoming = queue.upcoming(3);
        let names: Vec<_> = upcoming.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["guest", "1", "2"]);
    }

    #[test]
    fn replacing_starts_where_it_is_told() {
        let mut queue = queue(3);
        let tracks = vec![track("a"), track("b"), track("c")];
        queue.replace(tracks, 2, PlayContext::Liked);

        assert_eq!(queue.current().map(|t| t.name.clone()), Some("c".into()));
        assert_eq!(queue.context(), &PlayContext::Liked);
    }
}
