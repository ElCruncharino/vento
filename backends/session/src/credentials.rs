// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! How a credentials prompt's answer is read, the same for every client:
//! the adapters apply these to what the phone's prompts and the Linux
//! client's deliver, and `tests/vectors.rs` holds them to their cases.

use crate::Credentials;
use crate::catalog::About;

/// `DOMAIN\user`, `user@domain` or a bare name, as `(domain, user)`. Both
/// forms, because both are typed: the backslash is what Windows shows and
/// the `@` is what a UPN looks like.
pub fn split_domain(value: &str) -> (String, String) {
    let name = trim(value);
    if let Some((domain, user)) = name.split_once('\\') {
        return (domain.to_string(), user.to_string());
    }
    if let Some((user, domain)) = name.split_once('@')
        && !user.is_empty()
    {
        return (domain.to_string(), user.to_string());
    }
    (String::new(), name.to_string())
}

/// A typed name without what Java's `trim()` takes off it — ASCII controls
/// and the space — which is what the phone's prompts took off when they read
/// the answer themselves, so that a name reads as it always has there.
fn trim(typed: &str) -> &str {
    typed.trim_matches(|c: char| c <= ' ')
}

/// Credentials as a prompt hands them back: an empty user name means the
/// one the session was created with. A name is read trimmed, so one that
/// is only spaces is an empty one (`tests/vectors.rs`).
pub fn fill_user(mut credentials: Credentials, user: &str) -> Credentials {
    let typed = trim(&credentials.user);
    credentials.user = if typed.is_empty() { user } else { typed }.to_string();
    credentials
}

/// An RDP prompt's answer as the `(domain, user)` to send, against the
/// session's own `domain` and `user` (already split). An empty name is the
/// session's, domain and all; a name typed with a domain, or a domain given
/// beside it, is that one; and a bare name keeps the session's domain, since
/// the prompt has no box for one and `--domain CORP` answered with `alice`
/// means `CORP\alice`. A local account on a machine with a domain is
/// `.\alice`, as Windows writes it.
pub fn rdp_identity(answer: &Credentials, domain: &str, user: &str) -> (String, String) {
    let typed = trim(&answer.user);
    if typed.is_empty() {
        return (domain.to_string(), user.to_string());
    }
    if !answer.domain.is_empty() {
        return (answer.domain.clone(), typed.to_string());
    }
    match split_domain(typed) {
        (typed, name) if typed.is_empty() => (domain.to_string(), name),
        split => split,
    }
}

/// The user name the phone's credentials prompt starts with, from the
/// record's: as written, except for a backend whose user name carries a
/// domain ([`About::user_has_domain`], RDP's), whose prompt has no box for
/// one and so starts with it in the name, `DOMAIN\user` — the form Windows
/// shows — whichever way the record wrote it. (The Linux client's session
/// has a domain of its own, `--domain`, and starts with the user as given.)
pub fn prompt_user(about: &About, user: &str) -> String {
    if !about.user_has_domain {
        return user.to_string();
    }
    match split_domain(user) {
        (domain, user) if domain.is_empty() => user,
        (domain, user) => format!("{domain}\\{user}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_domain_is_split_either_way_it_is_typed() {
        assert_eq!(split_domain("CORP\\alice"), ("CORP".into(), "alice".into()));
        assert_eq!(split_domain("alice@corp.example"), ("corp.example".into(), "alice".into()));
        assert_eq!(split_domain(" alice "), ("".into(), "alice".into()));
        assert_eq!(split_domain("@odd"), ("".into(), "@odd".into()));
    }

    #[test]
    fn an_rdp_answer_keeps_the_session_domain_unless_it_names_one() {
        let answer = |user: &str, domain: &str| Credentials {
            user: user.into(),
            domain: domain.into(),
            password: "pw".into(),
        };
        let id = |a: &Credentials| rdp_identity(a, "CORP", "proto");
        assert_eq!(id(&answer("", "")), ("CORP".into(), "proto".into()));
        assert_eq!(id(&answer("proto", "")), ("CORP".into(), "proto".into()));
        assert_eq!(id(&answer("alice", "")), ("CORP".into(), "alice".into()));
        assert_eq!(id(&answer("LAB\\alice", "")), ("LAB".into(), "alice".into()));
        assert_eq!(id(&answer("alice@lab.example", "")), ("lab.example".into(), "alice".into()));
        assert_eq!(id(&answer(".\\alice", "")), (".".into(), "alice".into()));
        assert_eq!(id(&answer("alice", "LAB")), ("LAB".into(), "alice".into()));
        assert_eq!(rdp_identity(&answer("alice", ""), "", "proto"), ("".into(), "alice".into()));
    }

    #[test]
    fn a_prompt_with_a_domain_starts_with_it_in_the_name() {
        use crate::catalog::FIXTURE;
        let domain = About {
            user_has_domain: true,
            ..FIXTURE
        };
        assert_eq!(prompt_user(&domain, "proto@corp.example"), "corp.example\\proto");
        assert_eq!(prompt_user(&domain, "CORP\\proto"), "CORP\\proto");
        assert_eq!(prompt_user(&domain, " proto "), "proto");
        assert_eq!(prompt_user(&FIXTURE, "proto@corp.example"), "proto@corp.example");
    }
}
