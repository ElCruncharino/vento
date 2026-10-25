// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! A stream's frames in the device's format, where the two differ: a rate
//! the device would not open at, or a channel count it would not take.
//!
//! **Linear**, which is the cheapest conversion there is and the worst that
//! is worth having: between 44.1 and 48 kHz its images sit some 40 dB down
//! at the top of the band, which is inaudible in a remote desktop's
//! notification sounds and speech, and it has no delay and no filter state
//! to warm up. It is a fallback rather than the usual path: both platforms'
//! sound servers (PipeWire, directly or behind ALSA's default device;
//! AudioFlinger behind AAudio) take any rate and resample properly
//! themselves, so this runs only on a device that refused the stream's rate
//! outright.

use std::collections::VecDeque;

use super::{Format, MAX_CHANNELS};

/// One in 32.32 fixed point: the distance between two source frames.
const ONE: u64 = 1 << 32;

/// Where a conversion is between two source frames, kept between pulls so
/// that a rate change has no seam at the edge of each device buffer.
pub(super) struct Resampler {
    /// The source frames either side of the next output frame, `a` before.
    a: [i16; MAX_CHANNELS],
    b: [i16; MAX_CHANNELS],
    /// How far past `a` the next output frame is, in [`ONE`]s.
    frac: u64,
    loaded: bool,
}

impl Resampler {
    pub fn new() -> Resampler {
        Resampler {
            a: [0; MAX_CHANNELS],
            b: [0; MAX_CHANNELS],
            frac: 0,
            loaded: false,
        }
    }

    /// Forgets the frames it had: after the queue ran dry, what comes next
    /// is not a continuation of them.
    pub fn reset(&mut self) {
        self.loaded = false;
    }

    /// `out`, frames of `to`, from the front of `source`, samples of `from`.
    /// How many frames of `out` it filled, which is fewer than all of them
    /// only when `source` ran out.
    pub fn run(&mut self, source: &mut VecDeque<i16>, from: Format, to: Format, out: &mut [i16]) -> usize {
        let (sch, dch) = (from.channels, to.channels);
        let frames = out.len() / dch;
        if from.rate == to.rate {
            let n = frames.min(source.len() / sch);
            if sch == dch {
                for (o, s) in out[..n * dch].iter_mut().zip(source.drain(..n * sch)) {
                    *o = s;
                }
            } else {
                let mut frame = [0i16; MAX_CHANNELS];
                for i in 0..n {
                    for (f, s) in frame[..sch].iter_mut().zip(source.drain(..sch)) {
                        *f = s;
                    }
                    map(&frame[..sch], &mut out[i * dch..(i + 1) * dch]);
                }
            }
            return n;
        }

        if !self.loaded {
            if source.len() < 2 * sch {
                return 0;
            }
            pop(source, &mut self.a[..sch]);
            pop(source, &mut self.b[..sch]);
            self.frac = 0;
            self.loaded = true;
        }
        // Source frames for each output frame; the rates are bounded by
        // `Format::new`, so this neither overflows nor is zero.
        let step = ((from.rate as u64) << 32) / to.rate as u64;
        let mut frame = [0i16; MAX_CHANNELS];
        for i in 0..frames {
            while self.frac >= ONE {
                if source.len() < sch {
                    return i;
                }
                self.a = self.b;
                pop(source, &mut self.b[..sch]);
                self.frac -= ONE;
            }
            let t = self.frac as i64;
            for (c, f) in frame[..sch].iter_mut().enumerate() {
                let (a, b) = (self.a[c] as i64, self.b[c] as i64);
                *f = (a + (((b - a) * t) >> 32)) as i16;
            }
            map(&frame[..sch], &mut out[i * dch..(i + 1) * dch]);
            self.frac += step;
        }
        frames
    }
}

fn pop(source: &mut VecDeque<i16>, frame: &mut [i16]) {
    let n = frame.len();
    for (f, s) in frame.iter_mut().zip(source.drain(..n)) {
        *f = s;
    }
}

/// One frame into another channel count: mono to every channel, anything to
/// mono as the average, and otherwise channel for channel, with channels the
/// source does not have left silent and those the device does not have
/// dropped (the front pair comes first in every layout that is sent).
fn map(from: &[i16], to: &mut [i16]) {
    if from.len() == to.len() {
        to.copy_from_slice(from);
    } else if from.len() == 1 {
        to.fill(from[0]);
    } else if to.len() == 1 {
        to[0] = (from.iter().map(|&s| s as i32).sum::<i32>() / from.len() as i32) as i16;
    } else {
        for (c, t) in to.iter_mut().enumerate() {
            *t = from.get(c).copied().unwrap_or(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format(rate: u32, channels: usize) -> Format {
        Format::new(rate, channels).unwrap()
    }

    #[test]
    fn the_same_rate_is_a_copy_with_the_channels_mapped() {
        let mut r = Resampler::new();
        let mut source: VecDeque<i16> = [10, 20, 30, 40, 50, 60].into_iter().collect();
        let mut out = [0i16; 4];
        assert_eq!(r.run(&mut source, format(48_000, 2), format(48_000, 2), &mut out), 2);
        assert_eq!(out, [10, 20, 30, 40]);
        // Stereo to mono averages; and a source that runs out says so.
        let mut out = [0i16; 3];
        assert_eq!(r.run(&mut source, format(48_000, 2), format(48_000, 1), &mut out), 1);
        assert_eq!(out[0], 55);
        let mut source: VecDeque<i16> = [7, -7].into_iter().collect();
        let mut out = [0i16; 4];
        assert_eq!(r.run(&mut source, format(48_000, 1), format(48_000, 2), &mut out), 2);
        assert_eq!(out, [7, 7, -7, -7]);
    }

    #[test]
    fn a_ramp_stays_a_ramp_at_another_rate() {
        // A ramp is what linear interpolation reproduces exactly, so every
        // output sample is where the ramp is at that output frame's time.
        let source_rate = 44_100;
        let mut source: VecDeque<i16> = (0..4410).map(|i| i as i16).collect();
        let mut r = Resampler::new();
        let mut out = vec![0i16; 4000];
        let n = r.run(&mut source, format(source_rate, 1), format(48_000, 1), &mut out);
        assert_eq!(n, 4000);
        for (i, &s) in out.iter().enumerate() {
            let expected = i as f64 * source_rate as f64 / 48_000.0;
            assert!((s as f64 - expected).abs() <= 1.0, "frame {i}: {s} vs {expected}");
        }
        // Converted in pieces, the ramp carries on across the seams.
        let mut more = vec![0i16; 37];
        assert_eq!(r.run(&mut source, format(source_rate, 1), format(48_000, 1), &mut more), 37);
        let expected = 4000.0 * source_rate as f64 / 48_000.0;
        assert!((more[0] as f64 - expected).abs() <= 1.0);
    }

    #[test]
    fn a_tone_keeps_its_pitch_and_level_from_441_to_48() {
        let tone = |rate: u32, n: usize| -> Vec<i16> {
            (0..n)
                .map(|i| (16384.0 * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / rate as f64).sin()) as i16)
                .collect()
        };
        let mut source: VecDeque<i16> = tone(44_100, 44_100).into_iter().collect();
        let mut out = vec![0i16; 47_000];
        let mut r = Resampler::new();
        assert_eq!(r.run(&mut source, format(44_100, 1), format(48_000, 1), &mut out), 47_000);
        let reference = tone(48_000, 47_000);
        let worst = out.iter().zip(&reference).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
        // Linear error at 440 Hz is a few parts in ten thousand of full scale.
        assert!(worst < 40, "worst difference {worst}");
    }
}
