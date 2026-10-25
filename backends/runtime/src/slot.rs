// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Where a protocol thread waits for the user's answer.

use std::sync::{Condvar, Mutex};

/// There are two of these in a [`Bridge`](crate::Bridge) (the credentials
/// and the trust decision), and both work the same way: the handler fires an
/// event, blocks here, and [`Slot::answer`] wakes it from whatever thread the
/// answer arrives on.
///
/// `answered` is what tells "cancelled" (a `None` answer) apart from "not yet",
/// so a cancel does not look like a spurious wake-up; taking the answer clears
/// it again, so a server that asks twice is asked twice.
pub struct Slot<T> {
    state: Mutex<(bool, Option<T>)>,
    ready: Condvar,
}

impl<T> Slot<T> {
    pub fn new() -> Slot<T> {
        Slot {
            state: Mutex::new((false, None)),
            ready: Condvar::new(),
        }
    }

    pub fn wait(&self) -> Option<T> {
        let mut state = self.state.lock().unwrap();
        while !state.0 {
            state = self.ready.wait(state).unwrap();
        }
        state.0 = false;
        state.1.take()
    }

    pub fn answer(&self, value: Option<T>) {
        let mut state = self.state.lock().unwrap();
        *state = (true, value);
        self.ready.notify_all();
    }
}

impl<T> Default for Slot<T> {
    fn default() -> Slot<T> {
        Slot::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn an_answer_wakes_a_waiter_and_is_taken_once() {
        let slot = Arc::new(Slot::new());
        let waiter = {
            let slot = Arc::clone(&slot);
            std::thread::spawn(move || slot.wait())
        };
        slot.answer(Some(7));
        assert_eq!(waiter.join().unwrap(), Some(7));

        // Taken: the next wait is a fresh question, not a replay.
        slot.answer(None);
        assert_eq!(slot.wait(), None);
    }
}
