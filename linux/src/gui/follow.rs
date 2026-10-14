// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Asks the far end for a desktop the shape of this window: the phone's
//! `WindowFollower`, as a debounce and a negotiation with no window in it.
//!
//! Off unless somebody has said so per session: resizing a desktop is a
//! change to somebody else's machine, and one that happens because a window
//! was tiled differently is not one they asked for.
//!
//! It is asked on a tick rather than told, because whether the far end will
//! take a size becomes true some time after the connection — a VNC server
//! says so by sending a rectangle, which may arrive seconds in — and a
//! one-shot that fired too early would never fire again. The session ticks
//! it once a second with its poll, and [`DEBOUNCE`] after the window last
//! changed shape; nothing here touches a backend or a clock.

use std::time::{Duration, Instant};

/// How soon after the window changes shape the question is asked. A tiling
/// compositor settles a window in one event and a corner being dragged is
/// dozens; a resize is a round trip and, on RDP, a whole reactivation.
pub const DEBOUNCE: Duration = Duration::from_millis(600);

/// A sanity check on the rectangle rather than the protocol's limit:
/// anything outside this is not a window somebody is looking at a desktop
/// in, and asking for it would be asking for nonsense.
const MIN: u32 = 200;
const MAX: u32 = 16384;

#[derive(Debug, Default)]
pub struct Follower {
    enabled: bool,
    /// The size last asked for, so an unchanged window asks for nothing.
    followed: Option<(u32, u32)>,
    /// The window at the previous tick: two the same means it has settled.
    settled: Option<(u32, u32)>,
    /// When the window last changed shape asks for a tick.
    due: Option<Instant>,
}

impl Follower {
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// The size last asked for, for the debug overlay.
    pub fn followed(&self) -> Option<(u32, u32)> {
        self.followed
    }

    /// Turning it on forgets what was asked for before, so that it takes
    /// effect where it was turned on — the desktop may have been given a
    /// size by hand meanwhile, and the window is then a new question.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if enabled {
            self.followed = None;
        }
    }

    /// The window changed shape: bring the next tick forward to `DEBOUNCE`
    /// from now, rather than starting a second timer, so there is one place
    /// the question is asked.
    pub fn resized(&mut self, now: Instant) {
        if self.enabled {
            self.due = Some(now + DEBOUNCE);
        }
    }

    /// When the debounce next wants a tick, if it does.
    pub fn due(&self) -> Option<Instant> {
        self.due
    }

    /// A tick: the size to ask for, if this session has asked for that, the
    /// window has stopped moving, and the far end is taking sizes.
    ///
    /// "Stopped moving" is two ticks the same, which is the debounce.
    /// Nothing is asked twice — the same window is the same question, and a
    /// far end that refused it will refuse it again — so a refusal costs one
    /// request rather than one a second. `can_resize` is the caller's own
    /// poll, since asking again here would be a second crossing for an
    /// answer that arrived a line ago.
    pub fn tick(&mut self, window: (u32, u32), can_resize: bool) -> Option<(u32, u32)> {
        self.due = None;
        if !self.enabled {
            return None;
        }
        let (w, h) = window;
        if !(MIN..=MAX).contains(&w) || !(MIN..=MAX).contains(&h) {
            return None;
        }
        let settled = self.settled == Some(window);
        self.settled = Some(window);
        if !settled || self.followed == Some(window) || !can_resize {
            return None;
        }
        self.followed = Some(window);
        Some(window)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asks_once_the_window_has_settled_and_the_far_end_will_take_it() {
        let mut f = Follower::default();
        f.set_enabled(true);
        // Two ticks the same before anything is asked.
        assert_eq!(f.tick((1600, 900), true), None);
        assert_eq!(f.tick((1600, 900), true), Some((1600, 900)));
        // The same window is the same question.
        assert_eq!(f.tick((1600, 900), true), None);
        // A new size, settled, is a new question.
        assert_eq!(f.tick((1280, 800), true), None);
        assert_eq!(f.tick((1280, 800), true), Some((1280, 800)));
    }

    #[test]
    fn waits_for_the_far_end_without_losing_the_question() {
        let mut f = Follower::default();
        f.set_enabled(true);
        assert_eq!(f.tick((1600, 900), false), None);
        assert_eq!(f.tick((1600, 900), false), None);
        // The far end comes round: the next tick asks.
        assert_eq!(f.tick((1600, 900), true), Some((1600, 900)));
    }

    #[test]
    fn off_asks_nothing_and_turning_on_asks_again() {
        let mut f = Follower::default();
        assert_eq!(f.tick((1600, 900), true), None);
        assert_eq!(f.tick((1600, 900), true), None);
        // Off, it did not even watch the window: on, it settles first.
        f.set_enabled(true);
        assert_eq!(f.tick((1600, 900), true), None);
        assert_eq!(f.tick((1600, 900), true), Some((1600, 900)));
        f.set_enabled(false);
        assert_eq!(f.tick((1600, 900), true), None);
        // Back on: the same window, already settled, asked for again, since
        // the desktop may have been given another size meanwhile.
        f.set_enabled(true);
        assert_eq!(f.tick((1600, 900), true), Some((1600, 900)));
    }

    #[test]
    fn a_window_nobody_looks_at_a_desktop_in_is_not_offered() {
        let mut f = Follower::default();
        f.set_enabled(true);
        assert_eq!(f.tick((100, 900), true), None);
        assert_eq!(f.tick((100, 900), true), None);
        assert_eq!(f.tick((20000, 900), true), None);
        assert_eq!(f.tick((20000, 900), true), None);
    }

    #[test]
    fn a_resize_brings_the_tick_forward_only_while_enabled() {
        let now = Instant::now();
        let mut f = Follower::default();
        f.resized(now);
        assert_eq!(f.due(), None);
        f.set_enabled(true);
        f.resized(now);
        assert_eq!(f.due(), Some(now + DEBOUNCE));
        f.tick((1600, 900), true);
        assert_eq!(f.due(), None);
    }
}
