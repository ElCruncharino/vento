// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! A C backend's own library as the Linux client reaches it: opened by name
//! the first time anything asks, rather than linked, so that a library that
//! cannot be loaded (a system library it needs missing or too old, a file of
//! another build, or no file at all) makes that one backend not available
//! and the client starts with the rest.
//!
//! A backend's crate declares one [`Library`] over the table its build script
//! generated from its header (bindgen's `dynamic_library_name`), calls the
//! header's functions through [`call!`](crate::call), and has its `Kind`'s
//! `create` say the backend is not available where the library cannot be
//! opened through [`create_if_loaded!`](crate::create_if_loaded). On the phone
//! the C is linked into the same library as the crate, which exports nothing
//! but `JNI_OnLoad`, so there the library is a `Linked`, the calls are the
//! extern declarations and `create` is the crate's own function. The vtable a
//! session is driven by is reached through the session's own pointer and was
//! never a symbol, so [`start`](super::start) and [`CSession`](super::CSession)
//! are the same on both.

#[cfg(target_os = "linux")]
use std::sync::OnceLock;

/// How the Linux client opens a backend's library.
#[cfg(target_os = "linux")]
#[derive(Clone, Copy)]
pub enum Scope {
    /// Its symbols for itself (`RTLD_LOCAL`): nothing opened after it binds
    /// to them. For the open backends' libraries, which export only their
    /// header's functions and `vento_log`.
    Local,
    /// Its symbols in the process's global scope (`RTLD_GLOBAL`), as for a
    /// library the binary is linked against: for RealVNC's hosting layer,
    /// whose loader looks symbols up there.
    Global,
}

/// A backend's library and its functions, `T` being the table bindgen
/// generated over its header: opened once, the first time [`get`](Self::get)
/// is called, and never closed (the libraries register exit handlers, and a
/// session's threads run in them). A failure is kept too, as the sentence
/// every later call is told.
#[cfg(target_os = "linux")]
pub struct Library<T: 'static> {
    name: &'static str,
    header: &'static str,
    scope: Scope,
    bind: unsafe fn(libloading::Library) -> Result<T, libloading::Error>,
    loaded: OnceLock<Result<T, String>>,
}

#[cfg(target_os = "linux")]
impl<T> Library<T> {
    /// `name` is the file as the client's CMake build writes it, found through
    /// the binary's rpath (or `LD_LIBRARY_PATH`); `header` is the header the
    /// table was generated from, for the error when the file is another
    /// library; `bind` is the generated table's `from_library`.
    pub const fn new(
        name: &'static str,
        header: &'static str,
        scope: Scope,
        bind: unsafe fn(libloading::Library) -> Result<T, libloading::Error>,
    ) -> Self {
        Library { name, header, scope, bind, loaded: OnceLock::new() }
    }

    /// The table, or why the library could not be opened.
    pub fn get(&self) -> Result<&T, &str> {
        self.loaded.get_or_init(|| self.open()).as_ref().map_err(String::as_str)
    }

    /// The table of a library [`get`](Self::get) has already opened: under a
    /// session, which is started only once it has.
    pub fn loaded(&self) -> &T {
        self.get().expect("a session is started only over its opened library")
    }

    /// A backend's `version` over this library: its own version, as `read`
    /// reads it, or "not available: <why>".
    pub fn version(&self, read: impl FnOnce(&T) -> String) -> String {
        match self.get() {
            Ok(table) => read(table),
            Err(why) => format!("not available: {why}"),
        }
    }

    fn open(&self) -> Result<T, String> {
        use libloading::os::unix::{Library, RTLD_GLOBAL, RTLD_LOCAL, RTLD_NOW};
        let scope = match self.scope {
            Scope::Local => RTLD_LOCAL,
            Scope::Global => RTLD_GLOBAL,
        };
        // Every symbol bound now: a missing symbol or symbol version (a glibc
        // older than the build machine's) fails here, where it can be said,
        // rather than in the middle of a session. Opening it runs its
        // constructors, the vendored libraries' and the C++ runtime's, and the
        // lookups read the header's functions: the library is one the
        // client's own build made.
        let library = unsafe { Library::open(Some(self.name), RTLD_NOW | scope) }.map_err(|e| {
            // glibc names the file it could not load, which for a missing
            // dependency is the dependency: the library's name goes in front,
            // so that the sentence says whose it is.
            let e = e.to_string();
            if e.starts_with(self.name) { e } else { format!("{}: {e}", self.name) }
        })?;
        unsafe { (self.bind)(library.into()) }
            .map_err(|e| format!("{} is not the library {} describes: {e}", self.name, self.header))
    }
}

/// One of a C backend's own functions (its header's, beside the vtable),
/// called: on the Linux client as a member of `$library`, the table a
/// `Library` opened; on the phone, where the C is linked into the same
/// library as the crate, as the extern declaration `$ffi::$f`, and `$library`
/// is a `Linked`. Inside an `unsafe` block, as any call into the C is.
#[macro_export]
macro_rules! call {
    ($library:expr, $ffi:ident :: $f:ident ( $($arg:expr),* $(,)? )) => {{
        #[cfg(target_os = "linux")]
        let result = ($library).$f($($arg),*);
        #[cfg(not(target_os = "linux"))]
        let result = {
            let _: &$crate::c::library::Linked = $library;
            $ffi::$f($($arg),*)
        };
        result
    }};
}

/// A `Kind`'s `create` over a backend's library: on the Linux client a
/// function that opens `$library` first and, where it cannot be opened, fails
/// with "<$name> is not available: <why>"; on the phone `$create` itself.
#[macro_export]
macro_rules! create_if_loaded {
    ($library:expr, $name:literal, $create:path) => {{
        #[cfg(target_os = "linux")]
        let create = |c: &$crate::kind::Connect| match ($library).get() {
            Ok(_) => $create(c),
            Err(why) => Err(format!(concat!($name, " is not available: {}"), why)),
        };
        #[cfg(not(target_os = "linux"))]
        let create = $create;
        create
    }};
}

/// A backend's library where its C is linked into the same library as its
/// crate, with [`Library`]'s calls: always there. Inlined, so that a crate's
/// code on the phone is what it was when it called the C directly.
#[cfg(not(target_os = "linux"))]
pub struct Linked;

#[cfg(not(target_os = "linux"))]
impl Linked {
    #[inline]
    pub fn loaded(&self) -> &Linked {
        self
    }

    #[inline]
    pub fn version(&self, read: impl FnOnce(&Linked) -> String) -> String {
        read(self)
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    unsafe fn nothing(_: libloading::Library) -> Result<(), libloading::Error> {
        Ok(())
    }

    unsafe fn no_such_symbol(library: libloading::Library) -> Result<(), libloading::Error> {
        unsafe { library.get::<unsafe extern "C" fn()>(b"vento_nothing_version\0") }.map(|_| ())
    }

    /// A library that is not there is one backend not available, in a
    /// sentence that names the file, and asking again is told the same.
    #[test]
    fn a_missing_library_is_not_available_with_its_name() {
        static MISSING: Library<()> =
            Library::new("libvento_nothing.so", "vento/nothing.h", Scope::Local, nothing);
        let version = MISSING.version(|_| unreachable!());
        assert!(version.starts_with("not available: libvento_nothing.so: "), "{version}");
        assert_eq!(MISSING.get().unwrap_err(), version.trim_start_matches("not available: "));
    }

    /// A library that opens but lacks a function of the header is not the
    /// library the header describes, and says which function.
    #[test]
    fn another_library_is_not_the_one_its_header_describes() {
        static OTHER: Library<()> = Library::new("libc.so.6", "vento/nothing.h", Scope::Local, no_such_symbol);
        let why = OTHER.get().unwrap_err();
        assert!(why.starts_with("libc.so.6 is not the library vento/nothing.h describes: "), "{why}");
        assert!(why.contains("vento_nothing_version"), "{why}");
    }
}
