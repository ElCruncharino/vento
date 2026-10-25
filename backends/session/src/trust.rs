// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! How a far end's identity meets the pins, the same for every client: the
//! phone decides here, over a [`Store`] of its own, and
//! `backends/vectors/known_hosts.json` holds the rule to its cases.
//!
//! A remote desktop server's certificate is one it made for itself on first
//! start, and no authority vouches for it — so, as over SSH, the identity is
//! the key: a fingerprint is pinned against the address the first time
//! somebody accepts it and says to remember it, a connection is silent while
//! it matches, and loud when it does not.
//!
//! The key is the address as written, folded for case: a session may have no
//! record, two records may name one machine, and the thing identified is the
//! machine. A fingerprint is kept as the backend handed it over and compared
//! as its hex digits alone, since one library prints `66:CC:…` and another
//! `66:cc:…`. Where pins are kept is each client's own business — the phone's
//! `known_hosts` preferences — and what they mean is this module's:
//! [`decide`] when an identity arrives, [`answered`] when a person has said
//! what to do about one.

use crate::catalog::About;
use crate::events::Identity;
use crate::options::Options;

/// The address a pin is filed under: folded for case and nothing else, which
/// is right twice over — a name is case-insensitive in DNS and an IPv6
/// literal in its own notation. A default port left off is not filled in, so
/// one machine can have two entries. Trimmed as Java's `trim()` trims —
/// ASCII controls and the space, nothing wider — since the phone's pins are
/// already filed under its keys, and the two must file one address in one
/// place.
pub fn key(address: &str) -> String {
    address.trim_matches(|c: char| c <= ' ').to_lowercase()
}

/// A fingerprint as two are compared: the ASCII hex digits alone, upper
/// case. Nothing about how a library spells a digest is part of the digest.
pub fn canonical(fingerprint: &str) -> String {
    fingerprint
        .chars()
        .filter(char::is_ascii_hexdigit)
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// What a fingerprint comes to against the pins.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The one this address was accepted with: nothing is asked.
    Matches,
    /// Not seen under this address. `identity` is `New`, `MatchesAnother`
    /// with the key it is pinned under in `matching_address`, or `Changed`
    /// when this address is pinned to something else.
    Ask {
        identity: Identity,
        matching_address: String,
    },
}

/// `fingerprint` against `pins`, each a key as filed and the fingerprint it
/// was pinned with, in the store's order. The first entry for a key is the
/// one that counts, as SSH reads its file.
pub fn verdict(pins: &[(&str, &str)], address: &str, fingerprint: &str) -> Verdict {
    let key = key(address);
    let wanted = canonical(fingerprint);
    if let Some((_, pin)) = pins.iter().find(|(k, _)| *k == key) {
        if canonical(pin) == wanted {
            return Verdict::Matches;
        }
        return Verdict::Ask {
            identity: Identity::Changed,
            matching_address: String::new(),
        };
    }
    // One machine reached by name and by number, or moved, is the ordinary
    // explanation, and a different thing from a fingerprint changed under an
    // address that has not.
    let elsewhere = (!fingerprint.is_empty())
        .then(|| pins.iter().find(|(k, v)| *k != key && canonical(v) == wanted))
        .flatten();
    match elsewhere {
        Some((other, _)) => Verdict::Ask {
            identity: Identity::MatchesAnother,
            matching_address: other.to_string(),
        },
        None => Verdict::Ask {
            identity: Identity::New,
            matching_address: String::new(),
        },
    }
}

/// What a backend's identities are pinned against, and how a first one is
/// taken.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pinned {
    /// The address as written for all but RustDesk, whose key says which
    /// network the id is on.
    pub key: String,
    /// Whether a fingerprint never seen is pinned without asking: RustDesk's
    /// alone, since its key arrives signed by the rendezvous server for the
    /// id that was dialled and a question would have nothing to check it by.
    pub first_sight_silent: bool,
}

/// The rendezvous server an empty `RendezvousServer` means, which names the
/// public network in a RustDesk pin's key.
pub const RUSTDESK_PUBLIC_NETWORK: &str = "rs-ny.rustdesk.com";

pub fn pinned_as(about: &About, address: &str, options: &Options) -> Pinned {
    if about.id == crate::catalog::RUSTDESK.id {
        // The id and the network it means something on. An id is handed out
        // by a rendezvous server and re-issued when a machine's
        // configuration is wiped, so the same digits on two networks are two
        // machines, and one entry under the bare id would have them
        // overwriting each other's identity.
        let server = options.get("RendezvousServer").trim();
        let network = if server.is_empty() { RUSTDESK_PUBLIC_NETWORK } else { server };
        return Pinned {
            key: format!("{address}@{network}"),
            first_sight_silent: true,
        };
    }
    Pinned {
        key: address.to_string(),
        first_sight_silent: false,
    }
}

/// Where a client keeps its pins: what [`decide`] and [`answered`] need of
/// it, and nothing about how it is kept.
pub trait Store {
    /// Every pin as filed — a key and the fingerprint it was accepted with —
    /// in the store's order. A store that cannot be read has none, which is
    /// the answer a missing one has; saying so is the store's to do.
    fn pins(&self) -> Vec<(String, String)>;

    /// Files `fingerprint` under `key`, already a [`key`], in place of what
    /// was there. An error is for the log: a store that cannot be written
    /// costs the next connection a question, not this one its answer.
    fn put(&self, key: &str, fingerprint: &str) -> Result<(), String>;
}

/// A store read and never written, for a session whose trust is for the one
/// run and leaves the pins as it found them.
pub struct ReadOnly<'a>(pub &'a dyn Store);

impl Store for ReadOnly<'_> {
    fn pins(&self) -> Vec<(String, String)> {
        self.0.pins()
    }

    fn put(&self, _key: &str, _fingerprint: &str) -> Result<(), String> {
        Ok(())
    }
}

/// What a session does with an identity that has just arrived.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// The one it was accepted with: go on, and ask nothing.
    Matches,
    /// Never seen, by a backend whose first sight is vouched for (see
    /// [`Pinned::first_sight_silent`]): pinned already, and go on.
    FirstSight,
    /// Put to the person, with why: [`Verdict::Ask`]'s two halves. Their
    /// answer goes to [`answered`].
    Ask {
        identity: Identity,
        matching_address: String,
    },
}

/// `fingerprint` against the pins in `store`, for the backend whose pins are
/// filed as `pinned` says: a match is silent, a first sight a backend says is
/// vouched for is pinned without asking, and anything else — a change above
/// all, which is always asked — is a question.
pub fn decide(store: &dyn Store, pinned: &Pinned, fingerprint: &str) -> Decision {
    let pins = store.pins();
    let pins: Vec<(&str, &str)> = pins.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    match verdict(&pins, &pinned.key, fingerprint) {
        Verdict::Matches => Decision::Matches,
        // Pinned under this key is the one case that is still asked.
        Verdict::Ask { identity, .. } if pinned.first_sight_silent && identity != Identity::Changed => {
            pin(store, &pinned.key, fingerprint);
            Decision::FirstSight
        }
        Verdict::Ask {
            identity,
            matching_address,
        } => Decision::Ask {
            identity,
            matching_address,
        },
    }
}

/// A person's answer to [`Decision::Ask`], which is what the session is
/// answered: pinned only where they both accepted and said to remember it,
/// the one way a question ends in a pin.
pub fn answered(store: &dyn Store, pinned: &Pinned, fingerprint: &str, accept: bool, remember: bool) -> bool {
    if accept && remember {
        pin(store, &pinned.key, fingerprint);
    }
    accept
}

fn pin(store: &dyn Store, address: &str, fingerprint: &str) {
    let key = key(address);
    match store.put(&key, fingerprint) {
        Ok(()) => log::info!("pinned {key}"),
        Err(e) => log::warn!("could not pin {key}: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{IRONRDP, RUSTDESK};

    const FP: &str = "66:cc:ed:01:02";

    #[test]
    fn the_key_is_folded_for_case_and_the_fingerprint_for_spelling() {
        assert_eq!(key("  [FE80::1]:5900 "), "[fe80::1]:5900");
        assert_eq!(key("Host.Example"), "host.example");
        assert_eq!(canonical("66:cc:ED"), "66CCED");
        assert_eq!(canonical("66 CC-ed"), "66CCED");
    }

    #[test]
    fn a_match_is_silent_and_anything_else_is_a_question_with_its_reason() {
        let upper = FP.to_uppercase();
        let pins = [("host:3389", upper.as_str())];
        assert_eq!(verdict(&pins, "HOST:3389", FP), Verdict::Matches);
        let ask = |identity, matching_address: &str| Verdict::Ask {
            identity,
            matching_address: matching_address.into(),
        };
        assert_eq!(verdict(&pins, "host:3389", "00:11"), ask(Identity::Changed, ""));
        assert_eq!(verdict(&pins, "10.0.0.5:3389", FP), ask(Identity::MatchesAnother, "host:3389"));
        assert_eq!(verdict(&pins, "10.0.0.5:3389", "00:11"), ask(Identity::New, ""));
        // An empty fingerprint matches nothing elsewhere.
        assert_eq!(verdict(&[("a", "")], "b", ""), ask(Identity::New, ""));
    }

    #[test]
    fn a_pin_is_the_address_as_written_except_for_a_rustdesk_id() {
        let rdp = pinned_as(&IRONRDP, "Host:3389", &Options::defaults(IRONRDP.options));
        assert_eq!(rdp.key, "Host:3389");
        assert!(!rdp.first_sight_silent);
        let mut o = Options::defaults(RUSTDESK.options);
        let id = pinned_as(&RUSTDESK, "123456789", &o);
        assert_eq!(id.key, "123456789@rs-ny.rustdesk.com");
        assert!(id.first_sight_silent);
        o.set("RendezvousServer", " hbbs.example ");
        assert_eq!(pinned_as(&RUSTDESK, "123456789", &o).key, "123456789@hbbs.example");
    }

    /// Pins in memory, in the order they were filed.
    #[derive(Default)]
    struct Memory(std::cell::RefCell<Vec<(String, String)>>);

    impl Store for Memory {
        fn pins(&self) -> Vec<(String, String)> {
            self.0.borrow().clone()
        }

        fn put(&self, key: &str, fingerprint: &str) -> Result<(), String> {
            let mut pins = self.0.borrow_mut();
            pins.retain(|(k, _)| k != key);
            pins.push((key.into(), fingerprint.into()));
            Ok(())
        }
    }

    fn asked(identity: Identity, matching_address: &str) -> Decision {
        Decision::Ask {
            identity,
            matching_address: matching_address.into(),
        }
    }

    #[test]
    fn a_question_ends_in_a_pin_only_when_accepted_and_remembered() {
        let store = Memory::default();
        let host = Pinned {
            key: "Host:3389".into(),
            first_sight_silent: false,
        };
        assert_eq!(decide(&store, &host, FP), asked(Identity::New, ""));
        assert!(!answered(&store, &host, FP, false, true));
        assert!(answered(&store, &host, FP, true, false));
        assert!(store.pins().is_empty());
        assert!(answered(&store, &host, FP, true, true));
        // Filed under the folded key, as given.
        assert_eq!(store.pins(), [("host:3389".to_string(), FP.to_string())]);
        assert_eq!(decide(&store, &host, &FP.to_uppercase()), Decision::Matches);
        assert_eq!(decide(&store, &host, "00:11"), asked(Identity::Changed, ""));
        let other = Pinned {
            key: "10.0.0.5:3389".into(),
            first_sight_silent: false,
        };
        assert_eq!(decide(&store, &other, FP), asked(Identity::MatchesAnother, "host:3389"));
    }

    #[test]
    fn a_vouched_first_sight_is_pinned_silently_and_a_change_still_asked() {
        let store = Memory::default();
        let id = Pinned {
            key: "123456789@rs-ny.rustdesk.com".into(),
            first_sight_silent: true,
        };
        assert_eq!(decide(&store, &id, FP), Decision::FirstSight);
        assert_eq!(store.pins().len(), 1);
        assert_eq!(decide(&store, &id, FP), Decision::Matches);
        assert_eq!(decide(&store, &id, "00:11"), asked(Identity::Changed, ""));
        assert_eq!(store.pins()[0].1, FP);
        // Read only: the same decision, and nothing filed.
        let empty = Memory::default();
        assert_eq!(decide(&ReadOnly(&empty), &id, FP), Decision::FirstSight);
        assert!(empty.pins().is_empty());
    }
}
