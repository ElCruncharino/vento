// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! How the answer to a credentials prompt is read, the same for every
//! client: the adapters apply these to the answers from the phone's and the
//! Linux client's prompts, and `tests/vectors.rs` tests them against the
//! shared cases.

use crate::Credentials;
use crate::catalog::About;

/// `DOMAIN\user`, `user@domain` or a bare name, as `(domain, user)`. Both
/// forms are accepted because both get typed: Windows shows the backslash
/// form, and a UPN uses `@`. A name with a backslash is split at the first
/// one, with the rest kept whole, so `MicrosoftAccount\user@outlook.com` is
/// that domain and that user. Only a name without one is split at its first
/// `@`, except that an address at a common email domain (`EMAIL_DOMAINS`,
/// `EMAIL_PROVIDERS`) is a Microsoft account, sent as
/// `MicrosoftAccount\user@outlook.com`.
pub fn split_domain(value: &str) -> (String, String) {
    let name = trim(value);
    if let Some((domain, user)) = name.split_once('\\') {
        return (domain.to_string(), user.to_string());
    }
    if let Some((user, domain)) = name.split_once('@')
        && !user.is_empty()
    {
        if is_email_domain(domain) {
            return (MICROSOFT_ACCOUNT.to_string(), name.to_string());
        }
        return (domain.to_string(), user.to_string());
    }
    (String::new(), name.to_string())
}

/// The domain Windows signs in a Microsoft account with, as
/// `MicrosoftAccount\address`.
pub const MICROSOFT_ACCOUNT: &str = "MicrosoftAccount";

/// Email domains an address at which is taken to be a Microsoft account
/// rather than `user@domain` in a Windows domain: Microsoft's own, and the
/// providers most Microsoft accounts are registered with. Matched whole,
/// ignoring ASCII case, so a subdomain does not match.
const EMAIL_DOMAINS: &[&str] = &[
    "msn.com",
    "passport.com",
    "windowslive.com",
    "gmail.com",
    "googlemail.com",
    "ymail.com",
    "aol.com",
    "icloud.com",
    "me.com",
    "mac.com",
    "proton.me",
    "protonmail.com",
    "pm.me",
];

/// Email providers with a domain per country: `outlook.com`, `outlook.de`,
/// `hotmail.co.uk`, `yahoo.com.au` and so on ([`is_country_suffix`]).
const EMAIL_PROVIDERS: &[&str] = &["outlook", "hotmail", "live", "yahoo", "gmx"];

/// Whether an address at `domain` is taken to be a Microsoft account
/// ([`EMAIL_DOMAINS`], [`EMAIL_PROVIDERS`]).
fn is_email_domain(domain: &str) -> bool {
    let domain = domain.to_ascii_lowercase();
    if EMAIL_DOMAINS.contains(&domain.as_str()) {
        return true;
    }
    domain
        .split_once('.')
        .is_some_and(|(provider, suffix)| EMAIL_PROVIDERS.contains(&provider) && is_country_suffix(suffix))
}

/// `com`, `net`, a two-letter country code, or `co.` or `com.` and one.
fn is_country_suffix(suffix: &str) -> bool {
    let country = |s: &str| s.len() == 2 && s.bytes().all(|b| b.is_ascii_lowercase());
    match suffix.split_once('.') {
        None => matches!(suffix, "com" | "net") || country(suffix),
        Some((second, code)) => matches!(second, "co" | "com") && country(code),
    }
}

/// What a connection says when the server refused the user name or
/// password, in every backend (`VENTO_LOGON_FAILED` in `vento/backend.h`).
pub const LOGON_FAILED: &str = "Logon failed.";

/// [`LOGON_FAILED`] with advice for the domain the credentials were sent
/// with: for a Microsoft account, what Windows requires of one; and for a
/// domain with a dot in it, which is how an email address typed without
/// `MicrosoftAccount\` is split, how to type a Microsoft account. `.` is a
/// local account, not a dotted domain.
pub fn logon_failed(domain: &str) -> String {
    let advice = if domain.eq_ignore_ascii_case(MICROSOFT_ACCOUNT) {
        "For a Microsoft account, use its password, not the PIN. It must have signed in on that PC with \
         the password once, and the PC's \"only allow Windows Hello sign-in\" setting must be off."
    } else if domain.contains('.') && domain != "." {
        "A Microsoft account is entered as MicrosoftAccount\\email@address.com."
    } else {
        return LOGON_FAILED.to_string();
    };
    format!("{LOGON_FAILED} {advice}")
}

/// A typed name with ASCII controls and spaces trimmed, as Java's `trim()`
/// does, matching how the phone's prompts read the answer before this code
/// was shared.
fn trim(typed: &str) -> &str {
    typed.trim_matches(|c: char| c <= ' ')
}

/// Credentials as a prompt returns them: an empty user name means the one
/// the session was created with. A name is trimmed, so one that is only
/// spaces counts as empty (`tests/vectors.rs`).
pub fn fill_user(mut credentials: Credentials, user: &str) -> Credentials {
    let typed = trim(&credentials.user);
    credentials.user = if typed.is_empty() { user } else { typed }.to_string();
    credentials
}

/// An RDP prompt's answer as the `(domain, user)` to send, given the
/// session's own `domain` and `user` (already split). An empty name means
/// the session's user and domain; a name typed with a domain uses that
/// domain, as [`split_domain`] reads it, over one given separately, as
/// [`Connect::domain_and_user`](crate::Connect::domain_and_user) does; a
/// bare name uses a domain given separately; and otherwise it keeps the
/// session's domain, since the prompt has no domain field and `--domain
/// CORP` answered with `alice` means `CORP\alice`. A local account on a
/// machine with a domain is `.\alice`, as Windows writes it.
pub fn rdp_identity(answer: &Credentials, domain: &str, user: &str) -> (String, String) {
    let typed = trim(&answer.user);
    if typed.is_empty() {
        return (domain.to_string(), user.to_string());
    }
    match split_domain(typed) {
        (named, name) if !named.is_empty() => (named, name),
        (_, name) if !answer.domain.is_empty() => (answer.domain.clone(), name),
        (_, name) => (domain.to_string(), name),
    }
}

/// The initial user name in the phone's credentials prompt, from the
/// record's: as written, except for a backend whose user name carries a
/// domain ([`About::user_has_domain`], as in RDP). Its prompt has no domain
/// field, so it starts with `DOMAIN\user` (the form Windows shows),
/// whichever form the record used. (The Linux client has a separate
/// `--domain` and starts with the user as given.)
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
        assert_eq!(
            split_domain("MicrosoftAccount\\alice@corp.example"),
            ("MicrosoftAccount".into(), "alice@corp.example".into())
        );
        assert_eq!(split_domain("alice@Outlook.com"), ("MicrosoftAccount".into(), "alice@Outlook.com".into()));
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
    fn a_logon_failure_has_advice_for_its_domain() {
        assert_eq!(logon_failed(""), "Logon failed.");
        assert_eq!(logon_failed("CORP"), "Logon failed.");
        assert_eq!(logon_failed("."), "Logon failed.");
        assert_eq!(logon_failed("DESKTOP-12B9288"), "Logon failed.");
        for domain in ["MicrosoftAccount", "microsoftaccount", "MICROSOFTACCOUNT"] {
            let text = logon_failed(domain);
            assert!(text.starts_with("Logon failed. For a Microsoft account, use its password, not the PIN."), "{text}");
            assert!(text.contains("Windows Hello"), "{text}");
        }
        for domain in ["example.invalid", "corp.example", "outlook.com.", ".local"] {
            assert_eq!(
                logon_failed(domain),
                "Logon failed. A Microsoft account is entered as MicrosoftAccount\\email@address.com."
            );
        }
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
        assert_eq!(prompt_user(&domain, "proto@outlook.com"), "MicrosoftAccount\\proto@outlook.com");
        assert_eq!(
            prompt_user(&domain, "MicrosoftAccount\\proto@corp.example"),
            "MicrosoftAccount\\proto@corp.example"
        );
        assert_eq!(prompt_user(&domain, " proto "), "proto");
        assert_eq!(prompt_user(&FIXTURE, "proto@corp.example"), "proto@corp.example");
    }
}
