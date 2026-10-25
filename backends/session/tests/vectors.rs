// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The shared test vectors in `backends/vectors/`. `options.json` is read
//! by the Java tests in `backends/src/test` too, for the one rule with a Java
//! copy (`BackendOption.settle`, which the option screens run as somebody
//! edits), so that the copy cannot drift from this one without one of these
//! failing; the credentials and known-hosts cases are this side's alone,
//! since the phone applies those rules from here.

use serde_json::Value;
use std::path::PathBuf;
use session::Credentials;
use session::credentials::{fill_user, rdp_identity, split_domain};
use session::events::Identity;
use session::options::{Choice, Kind, Options, Scope, Spec};
use session::trust::{self, Verdict};

fn vectors(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../vectors")
        .join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn str<'a>(case: &'a Value, key: &str) -> &'a str {
    case[key].as_str().unwrap_or_else(|| panic!("{key} in {case}"))
}

fn cases<'a>(file: &'a Value, section: &str) -> &'a Vec<Value> {
    file[section].as_array().unwrap_or_else(|| panic!("no {section}"))
}

#[test]
fn split_domain_agrees_with_the_vectors() {
    let file = vectors("credentials.json");
    for case in cases(&file, "split_domain") {
        let (domain, user) = split_domain(str(case, "typed"));
        assert_eq!((domain.as_str(), user.as_str()), (str(case, "domain"), str(case, "user")), "{case}");
    }
}

#[test]
fn rdp_identity_agrees_with_the_vectors() {
    let file = vectors("credentials.json");
    for case in cases(&file, "rdp_identity") {
        let answer = Credentials {
            user: str(case, "answer_user").into(),
            domain: case["answer_domain"].as_str().unwrap_or("").into(),
            password: "pw".into(),
        };
        let (domain, user) = rdp_identity(&answer, str(case, "session_domain"), str(case, "session_user"));
        assert_eq!((domain.as_str(), user.as_str()), (str(case, "domain"), str(case, "user")), "{case}");
    }
}

#[test]
fn fill_user_agrees_with_the_vectors() {
    let file = vectors("credentials.json");
    for case in cases(&file, "fill_user") {
        let answer = Credentials {
            user: str(case, "answer_user").into(),
            domain: String::new(),
            password: "pw".into(),
        };
        let filled = fill_user(answer, str(case, "session_user"));
        assert_eq!(filled.user, str(case, "user"), "{case}");
        assert_eq!(filled.password, "pw");
    }
}

#[test]
fn known_hosts_keys_and_fingerprints_agree_with_the_vectors() {
    let file = vectors("known_hosts.json");
    for case in cases(&file, "key") {
        assert_eq!(trust::key(str(case, "address")), str(case, "key"), "{case}");
    }
    for case in cases(&file, "canonical") {
        assert_eq!(trust::canonical(str(case, "fingerprint")), str(case, "canonical"), "{case}");
    }
}

#[test]
fn known_hosts_verdicts_agree_with_the_vectors() {
    let file = vectors("known_hosts.json");
    for case in cases(&file, "verdict") {
        let pins: Vec<(&str, &str)> = case["pins"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(key, pin)| (key.as_str(), pin.as_str().unwrap()))
            .collect();
        let want = match str(case, "verdict") {
            "matches" => Verdict::Matches,
            "ask" => Verdict::Ask {
                identity: match str(case, "identity") {
                    "new" => Identity::New,
                    "matches_another" => Identity::MatchesAnother,
                    "changed" => Identity::Changed,
                    other => panic!("identity {other}"),
                },
                matching_address: str(case, "matching_address").into(),
            },
            other => panic!("verdict {other}"),
        };
        let got = trust::verdict(&pins, str(case, "address"), str(case, "fingerprint"));
        assert_eq!(got, want, "{case}");
    }
}

/// A spec from the vectors' own description of one. Leaked, since a spec
/// is `'static` by design and a test has a handful.
fn spec(value: &Value) -> Spec {
    let leak = |s: &str| -> &'static str { Box::leak(s.to_string().into_boxed_str()) };
    let choices: Vec<Choice> = value["choices"]
        .as_array()
        .map(|list| {
            list.iter()
                .map(|c| Choice {
                    value: leak(str(c, "value")),
                    label: leak(str(c, "value")),
                    desktop_label: None,
                    summary: None,
                    requires: c["requires"].as_str().map(leak),
                })
                .collect()
        })
        .unwrap_or_default();
    Spec {
        key: leak(str(value, "key")),
        label: leak(str(value, "key")),
        summary: None,
        desktop_summary: None,
        kind: match str(value, "type") {
            "choice" => Kind::Choice,
            "bool" => Kind::Bool,
            other => panic!("type {other}"),
        },
        choices: Box::leak(choices.into_boxed_slice()),
        default: leak(str(value, "default")),
        scope: Scope::Connection,
        live: false,
        gate: None,
    }
}

#[test]
fn settle_agrees_with_the_vectors() {
    let file = vectors("options.json");
    let specs: &'static [Spec] = Box::leak(cases(&file, "specs").iter().map(spec).collect::<Vec<_>>().into_boxed_slice());
    for case in cases(&file, "settle") {
        let mut options = Options::defaults(specs);
        for (key, value) in case["values"].as_object().unwrap() {
            options.set(key, value.as_str().unwrap());
        }
        let changed = options.settle(specs);
        let want: Vec<&str> = case["changed"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(changed, want, "{case}");
        for (key, value) in case["settled"].as_object().unwrap() {
            assert_eq!(options.get(key), value.as_str().unwrap(), "{key} in {case}");
        }
    }
}
