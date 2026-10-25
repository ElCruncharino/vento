// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Asks the far end for a desktop the shape of its windows: the phone's
//! `WindowFollower`, as a debounce and a negotiation with no window in it.
//! Each window showing one of the far end's monitors asks for that monitor
//! the window's size, where the far end takes a layout; otherwise the main
//! window asks for the desktop its size, where the far end has one screen.
//!
//! Off unless turned on for the session: resizing a desktop changes the far
//! end's machine, which may be another person's, and a window tiled
//! differently is not a request to resize it.
//!
//! It is asked on a tick rather than told, because whether the far end will
//! take a size becomes true some time after the connection (a VNC server
//! says so by sending a rectangle, which may arrive seconds in), and a
//! one-shot that fired too early would never fire again. The session ticks
//! it once a second with its poll, and [`DEBOUNCE`] after any window last
//! changed shape; nothing here touches a backend or a clock.

use crate::backend::Monitor;
use std::time::{Duration, Instant};

/// How soon after a window changes shape the question is asked. A tiling
/// compositor settles a window in one event and a corner being dragged is
/// dozens; a resize is a round trip and, on RDP, a whole reactivation. One
/// debounce for every window, so that windows changing shape together (a
/// tiling compositor re-tiling an output) are one question and not several.
pub const DEBOUNCE: Duration = Duration::from_millis(600);

/// A sanity check on the rectangle rather than the protocol's limit: a
/// window outside this is too small or too large to show a desktop in, and
/// is not asked for.
const MIN: u32 = 200;
const MAX: u32 = 16384;

/// One window, as far as following goes: the far end's monitor it shows,
/// by the far end's id (`None` for the whole desktop), and its size in
/// pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shown {
    pub monitor: Option<u32>,
    pub size: (u32, u32),
}

/// What the far end takes now, from the session's poll: a single size
/// (`Facts::can_resize`) and a layout of several screens
/// (`Facts::can_layout`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Takes {
    pub size: bool,
    pub layout: bool,
}

/// What the far end is asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ask {
    /// The whole desktop this size.
    Size(u32, u32),
    /// A desktop `width × height` of these screens, the far end's ids kept.
    Layout { width: u32, height: u32, screens: Vec<Monitor> },
}

impl Ask {
    /// The desktop this asks for.
    pub fn size(&self) -> (u32, u32) {
        match self {
            Ask::Size(w, h) => (*w, *h),
            Ask::Layout { width, height, .. } => (*width, *height),
        }
    }
}

#[derive(Debug, Default)]
pub struct Follower {
    enabled: bool,
    /// What was asked for last, so that the same windows over the same
    /// layout ask for nothing.
    followed: Option<Ask>,
    /// The windows at the previous tick: two the same means they have
    /// settled.
    settled: Option<Vec<Shown>>,
    /// When the tick asked for by a window's last change of shape is due.
    due: Option<Instant>,
}

impl Follower {
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// The desktop size last asked for, for the debug overlay.
    pub fn followed(&self) -> Option<(u32, u32)> {
        self.followed.as_ref().map(Ask::size)
    }

    /// Turning it on forgets what was asked for before, so that it takes
    /// effect where it was turned on: the desktop may have been given a size
    /// by hand meanwhile, and the windows are then a new question.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if enabled {
            self.followed = None;
        }
    }

    /// A window changed shape: bring the next tick forward to `DEBOUNCE`
    /// from now, rather than starting a second timer, so there is one place
    /// the question is asked, and one debounce whichever window it was.
    pub fn resized(&mut self, now: Instant) {
        if self.enabled {
            self.due = Some(now + DEBOUNCE);
        }
    }

    /// When the debounce next wants a tick, if it does.
    pub fn due(&self) -> Option<Instant> {
        self.due
    }

    /// A tick: what to ask for, if this session has asked for that, every
    /// window has stopped moving, and the far end takes what they make.
    ///
    /// `windows` are the session's, the main window first; `monitors` the
    /// far end's layout as the poll last read it. "Stopped moving" is two
    /// ticks the same, which is the debounce. Nothing is asked twice (the
    /// same question, and a far end that refused it will refuse it again),
    /// so a refusal costs one request rather than one a second. `takes` is
    /// from the caller's own poll, since asking again here would be a second
    /// call into the backend for an answer the poll has just read.
    pub fn tick(&mut self, windows: &[Shown], monitors: &[Monitor], takes: Takes) -> Option<Ask> {
        self.due = None;
        if !self.enabled || windows.is_empty() {
            return None;
        }
        let sane = |(w, h): (u32, u32)| (MIN..=MAX).contains(&w) && (MIN..=MAX).contains(&h);
        if !windows.iter().all(|w| sane(w.size)) {
            return None;
        }
        let settled = self.settled.as_deref() == Some(windows);
        self.settled = Some(windows.to_vec());
        if !settled {
            return None;
        }
        let ask = plan(windows, monitors, takes)?;
        if self.followed.as_ref() == Some(&ask) {
            return None;
        }
        self.followed = Some(ask.clone());
        Some(ask)
    }
}

/// What `windows` (the main one first) make of the far end's `monitors`.
///
/// Where the far end takes a layout and a window shows one of its monitors,
/// a layout: every monitor in the far end's order, left to right, a monitor
/// in a window the window's size and one in none its own, the ids kept so
/// that the far end moves its screens rather than making new ones. (Two
/// windows on one monitor: the first, which is the main window if it is one
/// of them.) Otherwise one size, the main window's, and only over a far end
/// of one screen, which is the one desktop a single size cannot rearrange.
fn plan(windows: &[Shown], monitors: &[Monitor], takes: Takes) -> Option<Ask> {
    if takes.layout {
        let size_of = |id: u32| windows.iter().find(|w| w.monitor == Some(id)).map(|w| w.size);
        if monitors.iter().any(|m| size_of(m.id).is_some()) {
            let (mut x, mut height) = (0i32, 0i32);
            let screens = monitors
                .iter()
                .map(|m| {
                    let (w, h) = size_of(m.id).map_or((m.width, m.height), |(w, h)| (w as i32, h as i32));
                    let screen = Monitor { id: m.id, x, y: 0, width: w, height: h };
                    x += w;
                    height = height.max(h);
                    screen
                })
                .collect();
            return Some(Ask::Layout { width: x as u32, height: height as u32, screens });
        }
    }
    let main = windows.first()?;
    (takes.size && monitors.len() <= 1).then_some(Ask::Size(main.size.0, main.size.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: Takes = Takes { size: true, layout: false };
    const BOTH: Takes = Takes { size: true, layout: true };
    const NONE: Takes = Takes { size: false, layout: false };

    fn main(w: u32, h: u32) -> Shown {
        Shown { monitor: None, size: (w, h) }
    }

    fn on(id: u32, w: u32, h: u32) -> Shown {
        Shown { monitor: Some(id), size: (w, h) }
    }

    fn screen(id: u32, x: i32, width: i32, height: i32) -> Monitor {
        Monitor { id, x, y: 0, width, height }
    }

    fn layout(screens: &[Monitor]) -> Ask {
        Ask::Layout {
            width: screens.iter().map(|m| m.width as u32).sum(),
            height: screens.iter().map(|m| m.height as u32).max().unwrap_or(0),
            screens: screens.to_vec(),
        }
    }

    #[test]
    fn asks_once_the_window_has_settled_and_the_far_end_will_take_it() {
        let mut f = Follower::default();
        f.set_enabled(true);
        // Two ticks the same before anything is asked.
        assert_eq!(f.tick(&[main(1600, 900)], &[], ONE), None);
        assert_eq!(f.tick(&[main(1600, 900)], &[], ONE), Some(Ask::Size(1600, 900)));
        // The same window is the same question.
        assert_eq!(f.tick(&[main(1600, 900)], &[], ONE), None);
        // A new size, settled, is a new question.
        assert_eq!(f.tick(&[main(1280, 800)], &[], ONE), None);
        assert_eq!(f.tick(&[main(1280, 800)], &[], ONE), Some(Ask::Size(1280, 800)));
        assert_eq!(f.followed(), Some((1280, 800)));
    }

    #[test]
    fn waits_for_the_far_end_without_losing_the_question() {
        let mut f = Follower::default();
        f.set_enabled(true);
        assert_eq!(f.tick(&[main(1600, 900)], &[], NONE), None);
        assert_eq!(f.tick(&[main(1600, 900)], &[], NONE), None);
        // The far end comes round: the next tick asks.
        assert_eq!(f.tick(&[main(1600, 900)], &[], ONE), Some(Ask::Size(1600, 900)));
    }

    #[test]
    fn off_asks_nothing_and_turning_on_asks_again() {
        let mut f = Follower::default();
        assert_eq!(f.tick(&[main(1600, 900)], &[], ONE), None);
        assert_eq!(f.tick(&[main(1600, 900)], &[], ONE), None);
        // Off, it did not even watch the window: on, it settles first.
        f.set_enabled(true);
        assert_eq!(f.tick(&[main(1600, 900)], &[], ONE), None);
        assert_eq!(f.tick(&[main(1600, 900)], &[], ONE), Some(Ask::Size(1600, 900)));
        f.set_enabled(false);
        assert_eq!(f.tick(&[main(1600, 900)], &[], ONE), None);
        // Back on: the same window, already settled, asked for again, since
        // the desktop may have been given another size meanwhile.
        f.set_enabled(true);
        assert_eq!(f.tick(&[main(1600, 900)], &[], ONE), Some(Ask::Size(1600, 900)));
    }

    #[test]
    fn a_window_nobody_looks_at_a_desktop_in_is_not_offered() {
        let mut f = Follower::default();
        f.set_enabled(true);
        assert_eq!(f.tick(&[main(100, 900)], &[], ONE), None);
        assert_eq!(f.tick(&[main(100, 900)], &[], ONE), None);
        assert_eq!(f.tick(&[main(20000, 900)], &[], ONE), None);
        assert_eq!(f.tick(&[main(20000, 900)], &[], ONE), None);
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
        f.tick(&[main(1600, 900)], &[], ONE);
        assert_eq!(f.due(), None);
    }

    #[test]
    fn one_size_is_only_for_a_far_end_of_one_screen() {
        let two = [screen(1, 0, 1280, 800), screen(2, 1280, 1280, 800)];
        let mut f = Follower::default();
        f.set_enabled(true);
        // Two screens and no layout: a single size would merge them, so the
        // main window asks nothing, whatever it shows.
        for _ in 0..3 {
            assert_eq!(f.tick(&[main(1600, 900)], &two, ONE), None);
            assert_eq!(f.tick(&[on(1, 1600, 900)], &two, ONE), None);
        }
        // One screen: the main window's size, as ever.
        let one = [screen(1, 0, 1280, 800)];
        assert_eq!(f.tick(&[main(1600, 900)], &one, ONE), None);
        assert_eq!(f.tick(&[main(1600, 900)], &one, ONE), Some(Ask::Size(1600, 900)));
    }

    #[test]
    fn each_window_asks_for_its_monitor_and_the_rest_keep_theirs() {
        let three = [screen(5, 0, 1024, 768), screen(1, 1024, 1280, 800), screen(9, 2304, 800, 600)];
        let mut f = Follower::default();
        f.set_enabled(true);
        // The main window on monitor 1 and a pop-out on monitor 9; monitor 5
        // has no window and keeps its size. Left to right in the far end's
        // order, ids kept.
        let windows = [on(1, 1600, 900), on(9, 1000, 700)];
        assert_eq!(f.tick(&windows, &three, BOTH), None);
        let asked = f.tick(&windows, &three, BOTH);
        assert_eq!(
            asked,
            Some(layout(&[screen(5, 0, 1024, 768), screen(1, 1024, 1600, 900), screen(9, 2624, 1000, 700)]))
        );
        assert_eq!(f.followed(), Some((3624, 900)));
        // Granted: the far end's layout is now the one asked for, and the
        // same windows ask nothing more.
        let granted = [screen(5, 0, 1024, 768), screen(1, 1024, 1600, 900), screen(9, 2624, 1000, 700)];
        assert_eq!(f.tick(&windows, &granted, BOTH), None);
        assert_eq!(f.tick(&windows, &granted, BOTH), None);
    }

    #[test]
    fn a_main_window_on_the_whole_desktop_is_left_out_of_a_layout() {
        let two = [screen(1, 0, 1280, 800), screen(2, 1280, 1280, 800)];
        let mut f = Follower::default();
        f.set_enabled(true);
        // The main window shows everything and a pop-out shows monitor 2:
        // the pop-out's size for 2, monitor 1 as it is.
        let windows = [main(1920, 1080), on(2, 1000, 700)];
        assert_eq!(f.tick(&windows, &two, BOTH), None);
        assert_eq!(f.tick(&windows, &two, BOTH), Some(layout(&[two[0], screen(2, 1280, 1000, 700)])));
        // The main window changing shape changes no question.
        let windows = [main(1600, 900), on(2, 1000, 700)];
        assert_eq!(f.tick(&windows, &two, BOTH), None);
        assert_eq!(f.tick(&windows, &two, BOTH), None);
        // With nothing on a single monitor, there is no layout to ask for.
        let mut f = Follower::default();
        f.set_enabled(true);
        assert_eq!(f.tick(&[main(1600, 900)], &two, BOTH), None);
        assert_eq!(f.tick(&[main(1600, 900)], &two, BOTH), None);
    }

    #[test]
    fn any_window_moving_unsettles_them_all() {
        let two = [screen(1, 0, 1280, 800), screen(2, 1280, 1280, 800)];
        let mut f = Follower::default();
        f.set_enabled(true);
        assert_eq!(f.tick(&[on(1, 1600, 900), on(2, 1000, 700)], &two, BOTH), None);
        // The pop-out is still being dragged: nothing yet, not even for the
        // main window, which has settled.
        assert_eq!(f.tick(&[on(1, 1600, 900), on(2, 1100, 700)], &two, BOTH), None);
        assert_eq!(f.tick(&[on(1, 1600, 900), on(2, 1200, 700)], &two, BOTH), None);
        // Both settled: one layout with both.
        assert_eq!(
            f.tick(&[on(1, 1600, 900), on(2, 1200, 700)], &two, BOTH),
            Some(layout(&[screen(1, 0, 1600, 900), screen(2, 1600, 1200, 700)]))
        );
    }

    #[test]
    fn a_layout_waits_for_the_far_end_to_take_one() {
        let two = [screen(1, 0, 1280, 800), screen(2, 1280, 1280, 800)];
        let mut f = Follower::default();
        f.set_enabled(true);
        let windows = [on(1, 1600, 900), on(2, 1000, 700)];
        assert_eq!(f.tick(&windows, &two, NONE), None);
        assert_eq!(f.tick(&windows, &two, NONE), None);
        assert_eq!(
            f.tick(&windows, &two, BOTH),
            Some(layout(&[screen(1, 0, 1600, 900), screen(2, 1600, 1000, 700)]))
        );
    }
}
