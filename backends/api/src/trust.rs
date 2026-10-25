// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Checking a far end's identity against the pins, the same for every
//! client: the phone and the Linux client both decide here, each with its
//! own [`Store`], and `tests/vectors.rs` tests the rule against the shared
//! cases.
//!
//! A remote desktop server's certificate is self-signed on first start, so,
//! as with SSH, the identity is the key: a fingerprint is pinned against the
//! address the first time the user accepts it and chooses to remember it,
//! and later connections proceed silently while it matches and warn when it
//! does not.
//!
//! The pin key is the address as written, folded for case: a session opened
//! from a command line has no record, two records may name one machine, and
//! the thing identified is the machine. A fingerprint is stored as the
//! backend gave it and compared by its hex digits only, since one library
//! prints `66:CC:…` and another `66:cc:…`. Each client chooses where pins are
//! stored (the phone's `known_hosts` preferences, the Linux client's
//! `known_hosts` in its state directory); this module defines what they mean:
//! [`decide`] when an identity arrives, [`answered`] when the user has
//! answered a question about one.

use crate::catalog::About;
use crate::events::Identity;
use crate::options::Options;

/// The address a pin is filed under: folded for case and nothing else,
/// since DNS names and IPv6 literals are both case-insensitive. A default
/// port left off is not filled in, so one machine can have two entries.
/// Trimmed as Java's `trim()` trims (ASCII controls and the space, nothing
/// wider), since the phone's existing pins are filed under such keys and
/// both must file one address in one place.
pub fn key(address: &str) -> String {
    address.trim_matches(|c: char| c <= ' ').to_lowercase()
}

/// A fingerprint in the form used for comparison: only the ASCII hex
/// digits, upper case, so that a library's formatting of a digest does not
/// matter.
pub fn canonical(fingerprint: &str) -> String {
    fingerprint
        .chars()
        .filter(char::is_ascii_hexdigit)
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// Whether `stored`, a key a store files a server under, names the same
/// server as `asked`, for forgetting it (`vento --forget`). Keys carry no
/// backend, so this ignores backends, and when unsure it returns true: a
/// forgotten server is asked about again, never trusted. A match is one of:
///
/// - the two are one [`key`] (the address as written, folded for case);
/// - `stored` is that key filed under a network, `<address>@<network>` (a
///   RustDesk ID under its rendezvous server, [`pinned_as`]): asking for the
///   address forgets it under every network, and asking for
///   `<address>@<network>` forgets only that one, by the first rule;
/// - both name a port and resolve to the same host and port under either
///   reading of a port (`runtime::address`): a number as a port, or under
///   100 as a VNC display, with `host::port` for the port itself, so
///   `host:1`, `host:5901` and `host::5901` are one server. A bare host has
///   no port and matches only the bare host, since its port was the
///   backend's default and the key does not say which backend.
pub fn same_server(stored: &str, asked: &str) -> bool {
    let (stored, asked) = (key(stored), key(asked));
    if stored == asked {
        return true;
    }
    if let Some((address, _network)) = stored.rsplit_once('@')
        && address == asked
    {
        return true;
    }
    use runtime::address::{Ports, split};
    // A default of 0 means no port: `split` returns 0 when none is given
    // and rejects a written 0.
    [Ports::Plain, Ports::Display].into_iter().any(|ports| {
        match (split(&stored, 0, ports), split(&asked, 0, ports)) {
            (Ok(a), Ok(b)) => a.1 != 0 && a == b,
            _ => false,
        }
    })
}

/// The host of a key, if it parses as an address. `vento --forget` uses it
/// to list the entries kept because they are on another port.
pub fn host_of(stored: &str) -> Option<String> {
    let stored = key(stored);
    let address = stored.rsplit_once('@').map_or(stored.as_str(), |(a, _)| a);
    runtime::address::split(address, 0, runtime::address::Ports::Display)
        .ok()
        .map(|(host, _)| host)
}

/// The result of checking a fingerprint against the pins.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The fingerprint this address was accepted with: nothing is asked.
    Matches,
    /// Not seen under this address. `identity` is `New`, `MatchesAnother`
    /// with the key it is pinned under in `matching_address`, or `Changed`
    /// when this address is pinned to something else.
    Ask {
        identity: Identity,
        matching_address: String,
    },
}

/// Checks `fingerprint` against `pins`, each a key as filed and the
/// fingerprint it was pinned with, in the store's order. The first entry for
/// a key wins, as SSH reads its file.
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
    // A known fingerprint under another key usually means one machine reached
    // by name and by number, or moved, which differs from a fingerprint
    // changing under the same address.
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

/// How a backend's identities are filed and first accepted, from its
/// [`About`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pinning {
    /// For a backend whose addresses are only meaningful on a network chosen
    /// by an option, that option, and the key is `address@network`; `None`
    /// files a pin under the address as written.
    pub network: Option<Network>,
    /// Whether a new fingerprint is pinned without asking: for a backend
    /// whose first key is vouched for by something other than the far end
    /// itself, so that the user would have nothing to check it against.
    pub first_sight_silent: bool,
}

/// The option that names an address's network, and the default network
/// when the option is empty.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Network {
    pub option: &'static str,
    pub default: &'static str,
}

impl Pinning {
    /// Used by every backend but one: the address as written, and the user
    /// is asked about a new fingerprint.
    pub const ADDRESS: Pinning = Pinning {
        network: None,
        first_sight_silent: false,
    };
}

/// What a backend's identities are pinned against, and how the first one is
/// accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pinned {
    /// The address as written, or `address@network` where the backend's
    /// [`Pinning`] names a network.
    pub key: String,
    /// [`Pinning::first_sight_silent`], for this backend.
    pub first_sight_silent: bool,
}

pub fn pinned_as(about: &About, address: &str, options: &Options) -> Pinned {
    let pinning = about.pinning;
    let key = match pinning.network {
        Some(network) => {
            let named = options.get(network.option).trim();
            let network = if named.is_empty() { network.default } else { named };
            format!("{address}@{network}")
        }
        None => address.to_string(),
    };
    Pinned {
        key,
        first_sight_silent: pinning.first_sight_silent,
    }
}

/// A client's pin storage, as far as [`decide`] and [`answered`] need it.
pub trait Store {
    /// Every pin as filed (a key and the fingerprint it was accepted with),
    /// in the store's order. An unreadable store returns none, like a
    /// missing one; reporting the error is up to the store.
    fn pins(&self) -> Vec<(String, String)>;

    /// Files `fingerprint` under `key` (already a [`key`]), replacing any
    /// existing entry. An error is only logged: an unwritable store means the
    /// next connection asks again, and does not change this one's answer.
    fn put(&self, key: &str, fingerprint: &str) -> Result<(), String>;
}

/// A store that is read and never written: the Linux probe's, whose
/// `--trust` applies to one run and leaves the pins unchanged.
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
    /// The fingerprint it was accepted with: continue without asking.
    Matches,
    /// A new fingerprint for a backend whose first one is vouched for (see
    /// [`Pinned::first_sight_silent`]): already pinned; continue.
    FirstSight,
    /// Ask the user, with the reason: [`Verdict::Ask`]'s two fields. The
    /// answer goes to [`answered`].
    Ask {
        identity: Identity,
        matching_address: String,
    },
}

/// Checks `fingerprint` against the pins in `store`, for a backend whose
/// pins are filed as `pinned` says: a match is silent, a new fingerprint the
/// backend marks as vouched for is pinned without asking, and anything else
/// is a question (a change is always asked about).
pub fn decide(store: &dyn Store, pinned: &Pinned, fingerprint: &str) -> Decision {
    let pins = store.pins();
    let pins: Vec<(&str, &str)> = pins.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    match verdict(&pins, &pinned.key, fingerprint) {
        Verdict::Matches => Decision::Matches,
        // A different fingerprint under this key is still asked about.
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

/// Applies the user's answer to [`Decision::Ask`] and returns the answer
/// for the session. The fingerprint is pinned only when the user both
/// accepted and chose to remember it.
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
    use crate::catalog::FIXTURE;

    const FP: &str = "66:cc:ed:01:02";

    #[test]
    fn the_key_is_folded_for_case_and_the_fingerprint_for_spelling() {
        assert_eq!(key("  [FE80::1]:5900 "), "[fe80::1]:5900");
        assert_eq!(key("Host.Example"), "host.example");
        assert_eq!(canonical("66:cc:ED"), "66CCED");
        assert_eq!(canonical("66 CC-ed"), "66CCED");
    }

    #[test]
    fn a_server_is_forgotten_by_any_spelling_of_its_port_and_under_any_network() {
        // One key, folded.
        assert!(same_server("host.example:3389", " Host.Example:3389 "));
        // A VNC display, its port, and `::port` are one server.
        for a in ["127.0.0.1:1", "127.0.0.1:5901", "127.0.0.1::5901"] {
            for b in ["127.0.0.1:1", "127.0.0.1:5901", "127.0.0.1::5901"] {
                assert!(same_server(a, b), "{a} {b}");
            }
        }
        assert!(same_server("[FE80::1]:5900", "[fe80::1]:0"));
        // Another port, another host, or a bare host against a port: kept.
        assert!(!same_server("127.0.0.1:5902", "127.0.0.1:5901"));
        assert!(!same_server("127.0.0.2:5901", "127.0.0.1:5901"));
        assert!(!same_server("desktop:5900", "desktop"));
        assert!(!same_server("desktop", "desktop:3389"));
        assert!(!same_server("desktop.example", "desktop"));
        // A RustDesk ID under every network, or under the one named.
        assert!(same_server("123456789@rs-ny.rustdesk.com", "123456789"));
        assert!(same_server("123456789@rs-ny.rustdesk.com", "123456789@RS-NY.rustdesk.com"));
        assert!(!same_server("123456789@rs-ny.rustdesk.com", "123456789@other.example"));
        assert!(!same_server("123456789@rs-ny.rustdesk.com", "12345678"));
        // What is not an address at all is matched as written only.
        assert!(same_server("not an address::", "NOT an address::"));
        assert!(!same_server("a:b", "a:c"));
        assert_eq!(host_of("Desktop:5900").as_deref(), Some("desktop"));
        assert_eq!(host_of("123456789@rs-ny.rustdesk.com").as_deref(), Some("123456789"));
        assert_eq!(host_of("[::1]:5900").as_deref(), Some("::1"));
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
    fn a_pin_is_the_address_as_written_unless_the_backend_names_a_network() {
        let plain = pinned_as(&FIXTURE, "Host:3389", &Options::default());
        assert_eq!(plain.key, "Host:3389");
        assert!(!plain.first_sight_silent);
        let networked = About {
            pinning: Pinning {
                network: Some(Network {
                    option: "Server",
                    default: "public.example",
                }),
                first_sight_silent: true,
            },
            ..FIXTURE
        };
        let mut o = Options::default();
        let id = pinned_as(&networked, "123456789", &o);
        assert_eq!(id.key, "123456789@public.example");
        assert!(id.first_sight_silent);
        o.set("Server", " own.example ");
        assert_eq!(pinned_as(&networked, "123456789", &o).key, "123456789@own.example");
    }

    /// Pins in memory, in filing order.
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
            key: "123456789@public.example".into(),
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
