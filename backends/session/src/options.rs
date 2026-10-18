// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! What a backend can be told, described well enough to offer without
//! knowing what it means: `BackendOption` in Java. The lists themselves are
//! each backend's own, in `backends/modules/<name>/catalog.rs` beside its
//! entry in the [`catalog`](crate::catalog), and are the one copy; what is
//! here is the shape of a row, the shorthands the lists are written in, and
//! the rows more than one backend offers in the same words. The phone's
//! lists are generated from them (`vento-codegen`, as a generated source of
//! each backend's module), in the phone's words; where a desktop needs other
//! words — a phone vibrates for a bell, a window cannot — a row says so
//! beside them ([`Spec::desktop_summary`], [`Choice::desktop_label`]).
//!
//! The values are strings, because that is what the backends' own
//! configuration layers take; [`Kind`] says how to ask for one, not how to
//! store it. A `-o key=value` on the command line and a row in a dialog are
//! the same thing here — a key checked against a backend's list, a value
//! checked against the kind — which is what keeps both of them generic.
//!
//! The lists are `static` and the descriptions are `&'static str`, so a
//! dialog can hold a reference to a row without owning anything; the
//! per-backend helpers that turn a value into what a library wants (a
//! compression level, an encodings string) are in that backend's adapter,
//! beside the call that takes it.

use crate::kind::Platform;
use std::collections::{BTreeMap, BTreeSet};

/// Which control to offer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `"true"` / `"false"`.
    Bool,
    /// One of the choices.
    Choice,
    /// A decimal integer.
    Int,
    /// Free text.
    Text,
}

/// Whose setting it is: the connection's, the backend's, or the connection's
/// with the backend's answer as its default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Connection,
    Global,
    Layered,
}

/// A value and what to call it on screen. `summary` is why you would pick
/// this one over the others, where that needs saying; `requires` is the key
/// of a `Bool` option this choice depends on, so a choice whose switch is off
/// is not offered at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Choice {
    pub value: &'static str,
    /// What to call it, as the phone words it.
    pub label: &'static str,
    /// What a desktop calls it where the phone's word is the phone's — "this
    /// phone" — or `None`. See [`Choice::label_on`].
    pub desktop_label: Option<&'static str>,
    pub summary: Option<&'static str>,
    pub requires: Option<&'static str>,
}

impl Choice {
    /// What to call it in the client it is shown by.
    pub fn label_on(&self, platform: Platform) -> &'static str {
        match platform {
            Platform::Android => self.label,
            Platform::Linux => self.desktop_label.unwrap_or(self.label),
        }
    }
}

/// What another option must say for this one to be offered at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gate {
    pub key: &'static str,
    pub values: &'static [&'static str],
}

impl Gate {
    pub fn open(&self, value: &str) -> bool {
        self.values.contains(&value)
    }
}

/// One setting a backend understands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spec {
    /// The backend's own name for it; the map key.
    pub key: &'static str,
    /// One line, for a preference title.
    pub label: &'static str,
    /// The longer explanation, as the phone words it.
    pub summary: Option<&'static str>,
    /// The same explanation where a desktop needs other words — a phone
    /// vibrates for a bell and a window cannot — or `None` where the phone's
    /// words do for both. See [`Spec::summary_on`].
    pub desktop_summary: Option<&'static str>,
    pub kind: Kind,
    /// For `Choice`, in display order; empty otherwise.
    pub choices: &'static [Choice],
    /// What the backend does when nothing is set.
    pub default: &'static str,
    pub scope: Scope,
    /// Whether `set_option` does anything mid-session.
    pub live: bool,
    pub gate: Option<Gate>,
}

impl Spec {
    /// The explanation in the words of the client it is shown by.
    pub fn summary_on(&self, platform: Platform) -> Option<&'static str> {
        match platform {
            Platform::Android => self.summary,
            Platform::Linux => self.desktop_summary.or(self.summary),
        }
    }

    /// For a `Choice`: what to show for `value` (a `&'static str` from the
    /// list) in the client it is shown by, or the value itself when it is not
    /// one of the choices.
    pub fn label_for<'a>(&self, value: &'a str, platform: Platform) -> &'a str {
        self.choices
            .iter()
            .find(|c| c.value == value)
            .map(|c| c.label_on(platform))
            .unwrap_or(value)
    }

    /// Whether `value` is something this option takes, in words when not.
    pub fn check(&self, value: &str) -> Result<(), String> {
        match self.kind {
            Kind::Bool => match value {
                "true" | "false" => Ok(()),
                _ => Err(format!("{}: true or false, not {value:?}", self.key)),
            },
            Kind::Int => value
                .parse::<i64>()
                .map(|_| ())
                .map_err(|_| format!("{}: a whole number, not {value:?}", self.key)),
            Kind::Choice => {
                if self.choices.iter().any(|c| c.value == value) {
                    Ok(())
                } else {
                    let values: Vec<&str> = self.choices.iter().map(|c| c.value).collect();
                    Err(format!(
                        "{}: one of {}, not {value:?}",
                        self.key,
                        values.join(", ")
                    ))
                }
            }
            Kind::Text => Ok(()),
        }
    }

    /// For a `Choice`: the choice `value` names, where it requires a switch
    /// that `on` says is off — a value left standing after its switch went,
    /// or given without it. Such a value is not offered, and is not what
    /// the backend would do either (TigerVNC's `Encoding=h264` without
    /// `H264` negotiates something else), so it must not stay selected.
    pub fn stranded(&self, value: &str, on: impl Fn(&str) -> bool) -> Option<&'static Choice> {
        let choices: &'static [Choice] = self.choices;
        choices
            .iter()
            .find(|c| c.value == value)
            .filter(|c| c.requires.is_some_and(|key| !on(key)))
    }

    /// What a stranded choice falls back to: the option's default, as if it
    /// had never been picked, or the first choice on offer where the default
    /// is stranded too (no list here has one that is).
    pub fn fallback(&self, on: impl Fn(&str) -> bool) -> &'static str {
        if self.stranded(self.default, &on).is_none() {
            return self.default;
        }
        let choices: &'static [Choice] = self.choices;
        choices
            .iter()
            .find(|c| c.requires.is_none_or(&on))
            .map_or(self.default, |c| c.value)
    }
}

pub(crate) const fn choice(value: &'static str, label: &'static str) -> Choice {
    Choice {
        value,
        label,
        desktop_label: None,
        summary: None,
        requires: None,
    }
}

pub(crate) const fn described(value: &'static str, label: &'static str, summary: &'static str) -> Choice {
    Choice {
        value,
        label,
        desktop_label: None,
        summary: Some(summary),
        requires: None,
    }
}

pub(crate) const fn requiring(value: &'static str, label: &'static str, requires: &'static str) -> Choice {
    Choice {
        value,
        label,
        desktop_label: None,
        summary: None,
        requires: Some(requires),
    }
}

// Eight fields of one record set at once; the shorthands below are what
// the lists actually call. They and the shared rows after them are the
// crate's alone: the lists that use them are compiled here, under
// `catalog`.
#[allow(clippy::too_many_arguments)]
pub(crate) const fn spec(
    key: &'static str,
    label: &'static str,
    summary: Option<&'static str>,
    kind: Kind,
    choices: &'static [Choice],
    default: &'static str,
    scope: Scope,
    live: bool,
) -> Spec {
    Spec {
        key,
        label,
        summary,
        desktop_summary: None,
        kind,
        choices,
        default,
        scope,
        live,
        gate: None,
    }
}

pub(crate) const fn bool(
    key: &'static str,
    label: &'static str,
    summary: &'static str,
    default: bool,
    scope: Scope,
    live: bool,
) -> Spec {
    spec(
        key,
        label,
        Some(summary),
        Kind::Bool,
        &[],
        if default { "true" } else { "false" },
        scope,
        live,
    )
}

pub(crate) const fn integer(
    key: &'static str,
    label: &'static str,
    summary: &'static str,
    default: &'static str,
    scope: Scope,
    live: bool,
) -> Spec {
    spec(key, label, Some(summary), Kind::Int, &[], default, scope, live)
}

pub(crate) const fn text(
    key: &'static str,
    label: &'static str,
    summary: &'static str,
    default: &'static str,
    scope: Scope,
    live: bool,
) -> Spec {
    spec(key, label, Some(summary), Kind::Text, &[], default, scope, live)
}

pub(crate) const fn choices(
    key: &'static str,
    label: &'static str,
    summary: Option<&'static str>,
    default: &'static str,
    scope: Scope,
    live: bool,
    choices: &'static [Choice],
) -> Spec {
    spec(key, label, summary, Kind::Choice, choices, default, scope, live)
}

/// The same option, offered only where `key` says one of `values`.
pub(crate) const fn when(spec: Spec, key: &'static str, values: &'static [&'static str]) -> Spec {
    Spec {
        gate: Some(Gate { key, values }),
        ..spec
    }
}

pub(crate) const VIEW_ONLY: Spec = bool(
    "ViewOnly",
    "View only",
    "Do not send input or clipboard events.",
    false,
    Scope::Connection,
    true,
);

pub(crate) const BELL: Spec = Spec {
    desktop_summary: Some("Show a bell in the window's title when the remote server rings the bell."),
    ..bool(
        "AcceptBell",
        "Bell",
        "Vibrate when the remote servers rings the bell.",
        true,
        Scope::Global,
        false,
    )
};

pub(crate) const SHARED: Spec = bool(
    "Shared",
    "Shared",
    "Do not ask the server to disconnect other clients.",
    true,
    Scope::Connection,
    false,
);

pub(crate) const ANONYMOUS_TLS: Spec = bool(
    "AnonymousTLS",
    "Unverified encryption",
    "Ask before using encryption without a peer certificate.",
    true,
    Scope::Connection,
    false,
);

pub(crate) const COMPRESSION_LEVELS: &[Choice] = &[
    choice("auto", "Automatic"),
    choice("1", "1 (fastest)"),
    choice("3", "3"),
    choice("6", "6 (balanced)"),
    choice("9", "9 (smallest)"),
];

pub(crate) const DESKTOP_SIZES: &[Choice] = &[
    choice("remote", "Do not change"),
    choice("device", "Match local device"),
    choice("1920x1200", "1920 × 1200"),
    choice("1920x1080", "1920 × 1080"),
    choice("1600x1000", "1600 × 1000"),
    choice("1366x768", "1366 × 768"),
    choice("1280x800", "1280 × 800"),
    choice("2560x1440", "2560 × 1440"),
];

pub(crate) const MONITOR_COUNTS: &[Choice] = &[
    choice("1", "One"),
    choice("2", "Two"),
    choice("3", "Three"),
];

/// A backend's settings as it was built with: every key from its list at its
/// default, and whatever was set over that. Remembers which keys were set,
/// because one backend (RealVNC) treats a colour depth somebody chose
/// differently from one that is merely the default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Options {
    values: BTreeMap<String, String>,
    set: BTreeSet<String>,
}

impl Options {
    pub fn defaults(specs: &[Spec]) -> Options {
        Options {
            values: specs
                .iter()
                .map(|s| (s.key.to_string(), s.default.to_string()))
                .collect(),
            set: BTreeSet::new(),
        }
    }

    pub fn set(&mut self, key: &str, value: &str) {
        self.values.insert(key.to_string(), value.to_string());
        self.set.insert(key.to_string());
    }

    /// The value, or empty where there is none — which is what every reader
    /// here expects of an option nobody has answered.
    pub fn get(&self, key: &str) -> &str {
        self.values.get(key).map(String::as_str).unwrap_or("")
    }

    /// Whether somebody set this, as opposed to it being the default.
    pub fn is_set(&self, key: &str) -> bool {
        self.set.contains(key)
    }

    /// Every choice whose required switch is off, put back to what
    /// [`Spec::fallback`] says; the keys that changed, for a live session to
    /// pass on. A fallback is nobody's choice, so it is not counted as set.
    /// The dialog and the live panel both keep to this whenever a switch
    /// moves.
    pub fn settle(&mut self, specs: &[Spec]) -> Vec<&'static str> {
        let mut changed = Vec::new();
        for spec in specs {
            if spec.stranded(self.get(spec.key), |key| self.bool(key)).is_some() {
                let value = spec.fallback(|key| self.bool(key));
                self.values.insert(spec.key.to_string(), value.to_string());
                self.set.remove(spec.key);
                changed.push(spec.key);
            }
        }
        changed
    }

    /// The first choice given without its switch, in words: the command line
    /// refuses `-o Encoding=h264` without `-o H264=true` rather than guess
    /// which of the two was meant.
    pub fn check_requirements(&self, specs: &[Spec]) -> Result<(), String> {
        for spec in specs {
            if let Some(choice) = spec.stranded(self.get(spec.key), |key| self.bool(key)) {
                let needs = choice.requires.unwrap_or_default();
                return Err(format!("{}={} needs {needs}=true", spec.key, choice.value));
            }
        }
        Ok(())
    }

    /// `Boolean.parseBoolean`: true for `"true"` in any case, false for
    /// everything else including nothing.
    pub fn bool(&self, key: &str) -> bool {
        self.get(key).eq_ignore_ascii_case("true")
    }

    /// A whole number, or `default` where it is not one.
    pub fn int(&self, key: &str, default: i64) -> i64 {
        self.get(key).trim().parse().unwrap_or(default)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.values.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }
}

/// `key=value`, as `-o` takes it. The first `=` splits, so a value may hold
/// one; a missing one is an error in words.
pub fn parse_assignment(text: &str) -> Result<(&str, &str), String> {
    match text.split_once('=') {
        Some((key, value)) if !key.trim().is_empty() => Ok((key.trim(), value)),
        _ => Err(format!("{text:?} is not key=value")),
    }
}

/// The spec for `key` in `specs`, in words when there is none.
pub fn find<'a>(specs: &'a [Spec], key: &str) -> Result<&'a Spec, String> {
    specs.iter().find(|s| s.key == key).ok_or_else(|| {
        let keys: Vec<&str> = specs.iter().map(|s| s.key).collect();
        format!("no option {key:?}; the options are {}", keys.join(", "))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rows of every kind, as a backend's list has them: a choice with one
    /// value behind a switch and one in a desktop's own words, a gated row,
    /// a number, free text, and three of the shared rows.
    static FIXTURE: &[Spec] = &[
        choices(
            "Encoding",
            "Encoding",
            None,
            "tight",
            Scope::Layered,
            true,
            &[choice("tight", "Tight"), requiring("h264", "H.264", "H264"), choice("rre", "RRE")],
        ),
        choices(
            "Sound",
            "Sound",
            None,
            "off",
            Scope::Layered,
            false,
            &[
                choice("off", "Off"),
                Choice {
                    desktop_label: Some("This computer"),
                    ..choice("local", "This phone")
                },
            ],
        ),
        when(text("Server", "Server", "Anything.", "", Scope::Layered, false), "Sound", &["local"]),
        integer("KeepAlive", "Keep-alive", "Seconds.", "30", Scope::Global, false),
        bool("H264", "H.264", "Offer it.", false, Scope::Global, true),
        VIEW_ONLY,
        SHARED,
        BELL,
    ];

    #[test]
    fn assignments_split_at_the_first_equals() {
        assert_eq!(parse_assignment("Encoding=zrle"), Ok(("Encoding", "zrle")));
        assert_eq!(parse_assignment("Key=a=b"), Ok(("Key", "a=b")));
        assert_eq!(parse_assignment("Compression="), Ok(("Compression", "")));
        assert!(parse_assignment("=x").is_err());
        assert!(parse_assignment("novalue").is_err());
    }

    #[test]
    fn a_value_is_checked_against_its_kind() {
        let encoding = find(FIXTURE, "Encoding").unwrap();
        assert!(encoding.check("rre").is_ok());
        assert!(encoding.check("zrle").unwrap_err().contains("one of"));
        assert_eq!(encoding.label_for("rre", Platform::Linux), "RRE");
        assert_eq!(encoding.label_for("nonsense", Platform::Linux), "nonsense");
        let sound = find(FIXTURE, "Sound").unwrap();
        assert_eq!(sound.label_for("local", Platform::Android), "This phone");
        assert_eq!(sound.label_for("local", Platform::Linux), "This computer");
        let bell = find(FIXTURE, "AcceptBell").unwrap();
        assert!(bell.summary_on(Platform::Android).unwrap().starts_with("Vibrate"));
        assert!(bell.summary_on(Platform::Linux).unwrap().starts_with("Show a bell"));
        let shared = find(FIXTURE, "Shared").unwrap();
        assert!(shared.check("true").is_ok());
        assert!(shared.check("yes").is_err());
        let keep = find(FIXTURE, "KeepAlive").unwrap();
        assert!(keep.check("45").is_ok());
        assert!(keep.check("soon").is_err());
        assert!(find(FIXTURE, "Quality").unwrap_err().contains("the options are Encoding, Sound"));
        assert!(find(FIXTURE, "Server").unwrap().check("anything").is_ok());
    }

    #[test]
    fn defaults_are_the_lists_and_overrides_are_remembered() {
        let mut options = Options::defaults(FIXTURE);
        assert_eq!(options.get("Encoding"), "tight");
        assert!(!options.is_set("Encoding"));
        assert!(options.bool("Shared"));
        assert_eq!(options.int("KeepAlive", 0), 30);
        options.set("Encoding", "rre");
        assert!(options.is_set("Encoding"));
        assert_eq!(options.get("Encoding"), "rre");
        assert_eq!(options.get("Nothing"), "");
        assert!(!options.bool("Nothing"));
        assert!(options.int("Encoding", 7) == 7);
    }

    #[test]
    fn a_gate_opens_on_its_values() {
        let gate = find(FIXTURE, "Server").unwrap().gate.unwrap();
        assert_eq!(gate.key, "Sound");
        assert!(gate.open("local"));
        assert!(!gate.open("off"));
    }

    #[test]
    fn a_choice_whose_switch_is_off_falls_back_to_the_default() {
        let mut options = Options::defaults(FIXTURE);
        options.set("H264", "true");
        options.set("Encoding", "h264");
        assert!(options.settle(FIXTURE).is_empty(), "its switch is on");
        assert_eq!(options.check_requirements(FIXTURE), Ok(()));
        options.set("H264", "false");
        assert_eq!(options.check_requirements(FIXTURE), Err("Encoding=h264 needs H264=true".to_string()));
        assert_eq!(options.settle(FIXTURE), ["Encoding"]);
        assert_eq!(options.get("Encoding"), "tight");
        assert!(!options.is_set("Encoding"), "a fallback is nobody's choice");
        assert!(options.settle(FIXTURE).is_empty(), "and settled stays settled");
        options.set("Encoding", "rre");
        assert!(options.settle(FIXTURE).is_empty());
        assert_eq!(options.get("Encoding"), "rre");
    }
}
