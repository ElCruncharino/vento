// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! Which certificate each address is supposed to have, as a file under
//! `$XDG_CONFIG_HOME/vento`: what the phone keeps in its `known_hosts`
//! preferences, kept here.
//!
//! A remote desktop server's certificate is one it made for itself on first
//! start, and no authority vouches for it — so, as on the phone and as over
//! SSH, the identity is the key: a fingerprint is pinned against the address
//! the first time somebody accepts it and says to remember it, a connection
//! is silent while it matches, and loud when it does not. A certificate an
//! authority would have vouched for is asked about all the same, since the
//! question is whether this is the machine it was last time, and an authority
//! has no opinion on that.
//!
//! This is the one store: no backend keeps certificates of its own (the
//! adapters tell each library not to), and both the window and the probe
//! read it. Only the window writes it, and only when a prompt was answered
//! with *remember server* — the probe's `--trust` is for that run.
//!
//! What a pin means — the key an address is filed under, how two
//! fingerprints compare, what is asked when they do not, and when an answer
//! is pinned — is `api::trust`, the phone's rule as well as this
//! client's; what is here is where this client keeps them, as a
//! `trust::Store`.
//!
//! The file is a line per address, `<address> <fingerprint>`, with `#`
//! comments; whitespace, control characters and `\` inside either are
//! written as `\xHH`. A line that does not read as an entry is ignored and
//! kept as it was the next time the file is written, so a hand edit gone
//! wrong costs that entry and nothing else. It is replaced whole — written
//! beside itself and renamed over — so a crash leaves the old one or the new
//! one, never half of either, and it is private to this user.

use api::catalog::About;
use api::kind::{Dirs, Kind};
use api::trust;
use std::fs::{DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// One line of a store file.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Line {
    Entry { key: String, value: String },
    /// A comment, a blank line, or a line that does not parse: kept as it
    /// was read.
    Other(Vec<u8>),
}

/// A store file's lines, in order: what is read, changed and written back.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Table {
    lines: Vec<Line>,
}

impl Table {
    /// Never fails: what is not an entry is kept as a line of something else.
    pub fn parse(bytes: &[u8]) -> Table {
        let mut lines: Vec<Line> = bytes.split(|&b| b == b'\n').map(parse_line).collect();
        // The newline that ends the last line is not a line after it.
        if bytes.is_empty() || bytes.ends_with(b"\n") {
            lines.pop();
        }
        Table { lines }
    }

    /// What `key` maps to: the first entry for it, as SSH reads its file.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries().find(|(k, _)| *k == key).map(|(_, v)| v)
    }

    /// Every entry, in file order.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &str)> {
        self.lines.iter().filter_map(|line| match line {
            Line::Entry { key, value } => Some((key.as_str(), value.as_str())),
            Line::Other(_) => None,
        })
    }

    /// `key` maps to `value` from now on: the first entry for it changed in
    /// place, any later ones dropped, and a new one at the end if there was
    /// none.
    pub fn set(&mut self, key: &str, value: &str) {
        let mut found = false;
        self.lines.retain_mut(|line| match line {
            Line::Entry { key: k, value: v } if k == key => {
                if found {
                    return false;
                }
                found = true;
                *v = value.to_string();
                true
            }
            _ => true,
        });
        if !found {
            self.lines.push(Line::Entry {
                key: key.to_string(),
                value: value.to_string(),
            });
        }
    }

    /// Takes out every entry whose key `forgets` says yes to, and hands them
    /// back as `(key, value)`; every other line stays where it was.
    pub fn remove_where(&mut self, mut forgets: impl FnMut(&str) -> bool) -> Vec<(String, String)> {
        let mut removed = Vec::new();
        self.lines.retain(|line| match line {
            Line::Entry { key, value } if forgets(key) => {
                removed.push((key.clone(), value.clone()));
                false
            }
            _ => true,
        });
        removed
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for line in &self.lines {
            match line {
                Line::Entry { key, value } => {
                    out.extend_from_slice(escape(key).as_bytes());
                    out.push(b' ');
                    out.extend_from_slice(escape(value).as_bytes());
                }
                Line::Other(raw) => out.extend_from_slice(raw),
            }
            out.push(b'\n');
        }
        out
    }
}

fn parse_line(raw: &[u8]) -> Line {
    let other = || Line::Other(raw.to_vec());
    let Ok(text) = std::str::from_utf8(raw) else {
        return other();
    };
    let text = text.trim_ascii();
    if text.is_empty() || text.starts_with('#') {
        return other();
    }
    let mut tokens = text.split_ascii_whitespace();
    let (Some(key), Some(value), None) = (tokens.next(), tokens.next(), tokens.next()) else {
        return other();
    };
    match (unescape(key), unescape(value)) {
        (Some(key), Some(value)) => Line::Entry { key, value },
        _ => other(),
    }
}

/// What would split a token or start a comment, as `\xHH`; `\` itself too,
/// so that `\xHH` is the only escape there is.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for (i, c) in s.char_indices() {
        if c.is_ascii_control() || c == ' ' || c == '\\' || (i == 0 && c == '#') {
            out.push_str(&format!("\\x{:02x}", c as u32));
        } else {
            out.push(c);
        }
    }
    out
}

fn unescape(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            let hex = bytes.get(i + 2..i + 4).filter(|h| bytes[i + 1] == b'x' && h.iter().all(u8::is_ascii_hexdigit))?;
            out.push(u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?);
            i += 4;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// A store file on disk, read afresh for every question so that an edit to
/// it counts from the next connection.
#[derive(Clone, Debug)]
pub struct Store {
    path: PathBuf,
}

impl Store {
    pub fn new(path: PathBuf) -> Store {
        Store { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The file as it is; a file that is not there is an empty one.
    pub fn read(&self) -> io::Result<Table> {
        match std::fs::read(&self.path) {
            Ok(bytes) => Ok(Table::parse(&bytes)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Table::default()),
            Err(e) => Err(e),
        }
    }

    /// The file as it is, or as empty where it cannot be read — said once in
    /// the log, since every question asked of an unreadable store has the
    /// answer a missing one has.
    pub fn read_or_empty(&self) -> Table {
        self.read().unwrap_or_else(|e| {
            log::warn!("{}: {e}; treated as empty", self.path.display());
            Table::default()
        })
    }

    /// `key` set to `value`, on disk. Read first, so that what another
    /// process wrote in the meantime is kept; a file that is there and cannot
    /// be read is left alone rather than replaced by one with a single line.
    pub fn set(&self, key: &str, value: &str) -> io::Result<()> {
        let mut table = self.read()?;
        if table.get(key) == Some(value) {
            return Ok(());
        }
        table.set(key, value);
        self.write(&table)
    }

    /// The entries whose key `forgets` says yes to, taken out on disk (the
    /// file written only if there were any): read first, and a file that is
    /// there and cannot be read is left alone, as for [`set`](Store::set).
    pub fn remove_where(&self, forgets: impl FnMut(&str) -> bool) -> io::Result<Vec<(String, String)>> {
        let mut table = self.read()?;
        let removed = table.remove_where(forgets);
        if !removed.is_empty() {
            self.write(&table)?;
        }
        Ok(removed)
    }

    /// Replaces the file with `table`: written beside it, flushed, and
    /// renamed over it, private to this user.
    fn write(&self, table: &Table) -> io::Result<()> {
        let dir = self.path.parent().unwrap_or(Path::new("."));
        DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        let name = self.path.file_name().and_then(|n| n.to_str()).unwrap_or("store");
        let temp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
        let written = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&temp)?;
            // A file left from an earlier crash keeps the mode it was made
            // with, which is not necessarily this one.
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            file.write_all(&table.to_bytes())?;
            file.sync_all()?;
            std::fs::rename(&temp, &self.path)
        })();
        if written.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        written
    }
}

/// The pin store: `api::trust`'s [`Store`](trust::Store) over a
/// [`Store`] file, which is what the window and the probe decide against.
#[derive(Clone, Debug)]
pub struct KnownHosts {
    store: Store,
}

impl KnownHosts {
    /// `known_hosts` in `config`, which is `Dirs::config`.
    pub fn in_dir(config: &Path) -> KnownHosts {
        KnownHosts {
            store: Store::new(config.join("known_hosts")),
        }
    }

    pub fn path(&self) -> &Path {
        self.store.path()
    }
}

impl trust::Store for KnownHosts {
    /// Read afresh for every question, so that an edit to the file counts
    /// from the next connection.
    fn pins(&self) -> Vec<(String, String)> {
        let table = self.store.read_or_empty();
        table.entries().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn put(&self, key: &str, fingerprint: &str) -> Result<(), String> {
        self.store
            .set(key, fingerprint)
            .map_err(|e| format!("{}: {e}", self.path().display()))
    }
}

/// RealVNC's identities: `RealVncBackend.identities`, the opaque blob the
/// core asks to have remembered, kept by address as written (not folded,
/// as on the phone) and handed back as the next session's `Identity`. Only
/// the core can compare one, so it is a store of its own rather than a pin.
pub fn realvnc_identities(config: &Path) -> Store {
    Store::new(config.join("realvnc_identities"))
}

/// The identity kept for `address`, for a session's
/// [`Connect::identity`](api::kind::Connect::identity) on `about`: read only
/// for a backend that takes one back ([`About::saves_identity`]), so that no
/// other backend's session opens the file, and only read, since a new one is
/// kept when a session ends, and only by the window.
pub fn saved_identity(config: &Path, about: &About, address: &str) -> Option<String> {
    if !about.saves_identity {
        return None;
    }
    realvnc_identities(config).read_or_empty().get(address).map(str::to_owned)
}

/// What [`forget`] did, a line for the person each.
#[derive(Debug, Default)]
pub struct Forgotten {
    pub removed: Vec<String>,
    /// Entries for the same host that were kept, being another port or a
    /// bare host, whose port was a backend's default: forgotten by that
    /// name.
    pub kept: Vec<String>,
    pub failed: Vec<String>,
}

/// `vento --forget`: the server `address` names forgotten everywhere it is
/// remembered, so that the next connection to it asks again — its pins in
/// the known hosts, its RealVNC identity, and what each backend's library
/// keeps of its own ([`Kind::forget`], RealVNC's core's record of the
/// servers it accepted). An address matches as `api::trust::same_server`
/// says: one key whatever its case, every network a RustDesk ID was pinned
/// under, and one host and port however the port was written (`host:1`,
/// `host:5901`, `host::5901`); a bare host only itself. Across every
/// backend, since no store says which backend filed an entry. Nothing is
/// connected to.
pub fn forget(dirs: &Dirs, kinds: &[&Kind], address: &str) -> Forgotten {
    let mut done = Forgotten::default();
    let host = trust::host_of(address);
    for store in [KnownHosts::in_dir(&dirs.config).store, realvnc_identities(&dirs.config)] {
        let path = store.path().display().to_string();
        match store.remove_where(|key| trust::same_server(key, address)) {
            Ok(removed) => done.removed.extend(removed.into_iter().map(|(key, _)| format!("{path}: {key}"))),
            Err(e) => {
                done.failed.push(format!("{path}: {e}"));
                continue;
            }
        }
        if let (Some(host), Ok(table)) = (&host, store.read()) {
            done.kept.extend(
                table
                    .entries()
                    .filter(|(key, _)| trust::host_of(key).as_ref() == Some(host))
                    .map(|(key, _)| format!("{path}: {key}")),
            );
        }
    }
    for kind in kinds {
        if let Some(forget) = kind.forget {
            match forget(dirs, address) {
                Ok(removed) => done.removed.extend(removed),
                Err(e) => done.failed.push(format!("{}: {e}", kind.id)),
            }
        }
    }
    done
}

#[cfg(test)]
mod tests {
    use super::*;
    use api::events::Identity;
    use api::trust::{Decision, Pinned, Store as _, answered, decide};

    const FP: &str = "66:cc:ed:01:02:03:04:05:06:07:08:09:0a:0b:0c:0d";

    fn table(text: &str) -> Table {
        Table::parse(text.as_bytes())
    }

    #[test]
    fn reads_entries_and_keeps_everything_else_as_it_was() {
        let text: &[u8] = b"# a comment\n\n127.0.0.1:3389 AA:BB\nnot an entry at all\n\xff\xfe junk\n[fe80::1]:5900 CC:DD\n";
        let t = Table::parse(text);
        assert_eq!(t.get("127.0.0.1:3389"), Some("AA:BB"));
        assert_eq!(t.get("[fe80::1]:5900"), Some("CC:DD"));
        assert_eq!(t.entries().count(), 2);
        assert_eq!(t.to_bytes(), text);
        // No newline at the end is the same file.
        assert_eq!(table("a b").to_bytes(), b"a b\n");
        assert_eq!(Table::parse(b"").to_bytes(), b"");
    }

    #[test]
    fn a_value_with_spaces_or_a_newline_survives_the_round_trip() {
        let mut t = Table::default();
        t.set("a b\\c", "x\ny z");
        t.set("#odd", "v");
        let bytes = t.to_bytes();
        assert_eq!(bytes, b"a\\x20b\\x5cc x\\x0ay\\x20z\n\\x23odd v\n");
        let back = Table::parse(&bytes);
        assert_eq!(back.get("a b\\c"), Some("x\ny z"));
        assert_eq!(back.get("#odd"), Some("v"));
        // A broken escape is a line that is not an entry.
        assert_eq!(table("a\\x2 b\n").entries().count(), 0);
        assert_eq!(table("a\\q b\n").entries().count(), 0);
        assert_eq!(table("a\\x+1 b\n").entries().count(), 0);
    }

    #[test]
    fn set_changes_the_first_entry_drops_the_rest_and_appends_a_new_one() {
        let mut t = table("# c\nh 1\nother 2\nh 3\n");
        assert_eq!(t.get("h"), Some("1"));
        t.set("h", "4");
        assert_eq!(t.to_bytes(), b"# c\nh 4\nother 2\n");
        t.set("new", "5");
        assert_eq!(t.to_bytes(), b"# c\nh 4\nother 2\nnew 5\n");
    }

    fn pinned(address: &str) -> Pinned {
        Pinned {
            key: address.into(),
            first_sight_silent: false,
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vento-known-hosts-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_server_is_forgotten_in_every_store_by_any_spelling_and_nothing_else() {
        let dir = scratch("forget");
        let dirs = Dirs {
            config: dir.join("config"),
            state: dir.join("state"),
            realvnc_libs: dir.join("libs"),
        };
        let hosts = KnownHosts::in_dir(&dirs.config);
        for (key, fp) in [
            ("127.0.0.1:5901", "AA"),
            ("# a comment", ""),
            ("127.0.0.1::5901", "BB"),
            ("127.0.0.1:5902", "CC"),
            ("127.0.0.1", "DD"),
            ("other:5901", "EE"),
        ] {
            if fp.is_empty() {
                let mut text = std::fs::read(hosts.path()).unwrap_or_default();
                text.extend_from_slice(format!("{key}\n").as_bytes());
                std::fs::write(hosts.path(), text).unwrap();
            } else {
                hosts.put(key, fp).unwrap();
            }
        }
        realvnc_identities(&dirs.config).set("127.0.0.1::5901", "blob").unwrap();
        // A backend that keeps something of its own is asked to forget it.
        static KIND: Kind = Kind {
            about: &api::catalog::REALVNC,
            version: String::new,
            create: |_| Err("not here".into()),
            forget: Some(|_, address| Ok(vec![format!("its own: {address}")])),
        };
        let done = forget(&dirs, &[&KIND], "127.0.0.1:1");
        let known = hosts.path().display().to_string();
        let ids = realvnc_identities(&dirs.config).path().display().to_string();
        assert_eq!(
            done.removed,
            [
                format!("{known}: 127.0.0.1:5901"),
                format!("{known}: 127.0.0.1::5901"),
                format!("{ids}: 127.0.0.1::5901"),
                "its own: 127.0.0.1:1".to_string(),
            ]
        );
        // Another port and the bare host are kept, and said.
        assert_eq!(done.kept, [format!("{known}: 127.0.0.1:5902"), format!("{known}: 127.0.0.1")]);
        assert!(done.failed.is_empty());
        let after = std::fs::read_to_string(hosts.path()).unwrap();
        assert_eq!(after, "# a comment\n127.0.0.1:5902 CC\n127.0.0.1 DD\nother:5901 EE\n");
        assert_eq!(std::fs::read_to_string(realvnc_identities(&dirs.config).path()).unwrap(), "");
        // Again: nothing left to forget, and nothing written.
        assert_eq!(forget(&dirs, &[], "127.0.0.1::5901").removed, Vec::<String>::new());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pinning_writes_a_private_file_and_keeps_what_was_there() {
        let dir = scratch("pin");
        let hosts = KnownHosts::in_dir(&dir.join("vento"));
        let host = pinned("Host");
        assert_eq!(
            decide(&hosts, &host, FP),
            Decision::Ask {
                identity: Identity::New,
                matching_address: String::new()
            }
        );
        assert!(answered(&hosts, &host, FP, true, true));
        assert_eq!(decide(&hosts, &pinned("host"), &FP.to_uppercase()), Decision::Matches);
        let mode = std::fs::metadata(hosts.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let dir_mode = std::fs::metadata(dir.join("vento")).unwrap().permissions().mode();
        assert_eq!(dir_mode & 0o777, 0o700);

        // A hand edit gone wrong costs that line and nothing else.
        let mut text = std::fs::read(hosts.path()).unwrap();
        text.extend_from_slice(b"garbage\n");
        std::fs::write(hosts.path(), &text).unwrap();
        hosts.put("other", "AA").unwrap();
        let after = std::fs::read_to_string(hosts.path()).unwrap();
        assert_eq!(after, format!("host {FP}\ngarbage\nother AA\n"));
        // Nothing is left beside it.
        assert_eq!(std::fs::read_dir(dir.join("vento")).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Only a backend that takes an identity back is handed the one kept
    /// for the address; every other is handed none, whatever the file holds.
    #[test]
    fn only_a_backend_that_saves_identities_is_handed_one() {
        let dir = scratch("identity");
        realvnc_identities(&dir).set("127.0.0.1::5901", "blob").unwrap();
        let address = "127.0.0.1::5901";
        assert_eq!(saved_identity(&dir, &api::catalog::REALVNC, address).as_deref(), Some("blob"));
        for about in api::catalog::ALL.iter().filter(|a| a.id != "realvnc") {
            assert!(!about.saves_identity, "{}", about.id);
            assert_eq!(saved_identity(&dir, about, address), None, "{}", about.id);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_store_that_cannot_be_read_asks_and_is_not_replaced() {
        let dir = scratch("unreadable");
        // A directory where the file should be: unreadable as a file.
        std::fs::create_dir_all(dir.join("known_hosts")).unwrap();
        let hosts = KnownHosts::in_dir(&dir);
        assert!(matches!(decide(&hosts, &pinned("host"), FP), Decision::Ask { .. }));
        assert!(hosts.put("host", FP).is_err());
        assert!(dir.join("known_hosts").is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
