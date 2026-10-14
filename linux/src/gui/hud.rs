// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The debug readout: the phone's `Hud` and `SessionHud`, as a translucent
//! box of monospace lines in the bottom left of the desktop.
//!
//! What is here is the box and the counters — a rate off a running total,
//! averaged over a window so it does not flicker — and the lines are the
//! session's, built where every collaborator is in reach. The box is drawn
//! only while it is on, and costs a handful of labels a frame then; the
//! counters it reads are kept whether it is on or not, since a count that
//! only starts when somebody looks has nothing to say about the frame they
//! wondered about.

use egui::{Align2, Color32, CornerRadius, Frame, Id, Margin, Order, RichText, TextWrapMode, vec2};
use std::time::{Duration, Instant};

/// How long a rate is averaged over.
pub const WINDOW: Duration = Duration::from_secs(1);

/// A per-second rate off a monotonically increasing counter, sampled once a
/// frame: the `Hud.Rate` of the phone, over a second rather than half of
/// one, since this window repaints only when something changes and a short
/// window with two frames in it is not a rate.
#[derive(Debug, Default)]
pub struct Rate {
    window_start: Option<Instant>,
    base: u64,
    rate: u64,
}

impl Rate {
    /// The running total now; the rate as of the last full window.
    pub fn sample(&mut self, total: u64, now: Instant) -> u64 {
        match self.window_start {
            None => {
                self.window_start = Some(now);
                self.base = total;
            }
            Some(start) if now.duration_since(start) >= WINDOW => {
                let elapsed = now.duration_since(start).as_secs_f64();
                self.rate = ((total.saturating_sub(self.base)) as f64 / elapsed).round() as u64;
                self.window_start = Some(now);
                self.base = total;
            }
            Some(_) => {}
        }
        self.rate
    }
}

/// The box, and the rates every readout wants.
#[derive(Debug, Default)]
pub struct Hud {
    pub on: bool,
    pub frames: u64,
    pub fps: Rate,
    pub damage: Rate,
    pub bytes: Rate,
}

impl Hud {
    /// Draws `lines` in the box, over everything but the prompts.
    pub fn draw(ctx: &egui::Context, lines: &[String]) {
        egui::Area::new(Id::new("hud"))
            .order(Order::Foreground)
            .anchor(Align2::LEFT_BOTTOM, vec2(8.0, -8.0))
            .interactable(false)
            .show(ctx, |ui| {
                Frame::new()
                    .fill(Color32::from_rgba_unmultiplied(16, 20, 24, 176))
                    .corner_radius(CornerRadius::same(4))
                    .inner_margin(Margin::symmetric(8, 6))
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        // A line is a line: the box grows to the widest
                        // rather than folding one.
                        ui.style_mut().wrap_mode = Some(TextWrapMode::Extend);
                        for line in lines {
                            ui.label(
                                RichText::new(line)
                                    .monospace()
                                    .size(11.0)
                                    .color(Color32::from_rgb(232, 240, 255)),
                            );
                        }
                    });
            });
    }
}

/// RFB's button mask, in letters: the phone's `Button.maskName`.
pub fn buttons_name(mask: i32) -> String {
    let mut s = String::new();
    for (bit, letter) in [(1, 'L'), (2, 'M'), (4, 'R')] {
        if mask & bit != 0 {
            s.push(letter);
        }
    }
    if s.is_empty() {
        s.push('-');
    }
    s
}

/// A byte rate in the unit that keeps it under four digits.
pub fn bytes_per_second(rate: u64) -> String {
    if rate >= 10_000_000 {
        format!("{:.1} MB/s", rate as f64 / 1e6)
    } else if rate >= 10_000 {
        format!("{} kB/s", rate / 1000)
    } else {
        format!("{rate} B/s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rate_is_the_count_over_the_last_full_window() {
        let start = Instant::now();
        let mut rate = Rate::default();
        assert_eq!(rate.sample(0, start), 0);
        // Half a window in: nothing to say yet.
        assert_eq!(rate.sample(30, start + Duration::from_millis(500)), 0);
        // A whole window: sixty in a second.
        assert_eq!(rate.sample(60, start + Duration::from_millis(1000)), 60);
        // Held until the next window is full, then the new count.
        assert_eq!(rate.sample(70, start + Duration::from_millis(1500)), 60);
        assert_eq!(rate.sample(80, start + Duration::from_millis(2000)), 20);
    }

    #[test]
    fn buttons_and_bytes_read_as_on_the_phone() {
        assert_eq!(buttons_name(0), "-");
        assert_eq!(buttons_name(1), "L");
        assert_eq!(buttons_name(5), "LR");
        assert_eq!(bytes_per_second(900), "900 B/s");
        assert_eq!(bytes_per_second(123_456), "123 kB/s");
        assert_eq!(bytes_per_second(12_345_678), "12.3 MB/s");
    }
}
