// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! How a credentials prompt's answer is read, the same for every client:
//! the shared vectors in `backends/vectors/credentials.json` hold the Java
//! side to these.

use crate::Credentials;

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
/// and the space — so that the phone and this client read one answer alike.
fn trim(typed: &str) -> &str {
    typed.trim_matches(|c: char| c <= ' ')
}

/// Credentials as the Java prompts deliver them: an empty user name means
/// the one the session was created with. A name is read trimmed, so one that
/// is only spaces is an empty one (`backends/vectors/credentials.json`).
pub fn fill_user(mut credentials: Credentials, user: &str) -> Credentials {
    let typed = trim(&credentials.user);
    credentials.user = if typed.is_empty() { user } else { typed }.to_string();
    credentials
}

/// An RDP prompt's answer as the `(domain, user)` to send, against the
/// session's own `domain` and `user` (already split). An empty name is the
/// session's, domain and all; a name typed with a domain, or a domain given
/// beside it, is that one; and a bare name keeps the session's domain, since
/// the prompt has no box for one and a session domain of `CORP` answered
/// with `alice` means `CORP\alice`. A local account on a machine with a
/// domain is `.\alice`, as Windows writes it.
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
}
