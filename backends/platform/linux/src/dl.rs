// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! `dlopen` and `dlsym`, for the system libraries the sound outputs load at
//! run time rather than link (libpipewire-0.3 in `pipewire.rs`, libasound in
//! `alsa.rs`), so that the binary starts on a machine with neither, and that
//! machine's sessions go without sound rather than the client without a
//! window. (libva is loaded the same way, from C: `cpp/va_shim.c`.)
//!
//! A library is opened for good: what is looked up in it is kept for the
//! life of the process, so nothing here ever closes one.

use std::ffi::{CStr, c_char, c_int, c_void};

#[link(name = "dl")]
unsafe extern "C" {
    fn dlopen(file: *const c_char, mode: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    fn dlerror() -> *const c_char;
}
const RTLD_NOW: c_int = 2;

/// A library this process opened, by its handle.
#[derive(Clone, Copy)]
pub struct Library(*mut c_void);

impl Library {
    /// Opens `soname`, with every symbol bound at once, or returns the
    /// loader's error message (`libasound.so.2: cannot open shared object
    /// file: No such file or directory`).
    pub fn open(soname: &CStr) -> Result<Library, String> {
        // Opening a system library runs its constructors, which is what
        // linking it would have done at start.
        let lib = unsafe { dlopen(soname.as_ptr(), RTLD_NOW) };
        if !lib.is_null() {
            return Ok(Library(lib));
        }
        let why = unsafe { dlerror() };
        Err(if why.is_null() {
            format!("{} did not load", soname.to_string_lossy())
        } else {
            unsafe { CStr::from_ptr(why) }.to_string_lossy().into_owned()
        })
    }

    /// The function `name` (NUL-terminated, as `concat!(name, "\0")` makes
    /// it), as the function pointer type it is declared as at the call, or
    /// `None` where the library has no such symbol.
    ///
    /// # Safety
    ///
    /// `F` must be the function's type: nothing checks it.
    pub unsafe fn function<F: Copy>(self, name: &str) -> Option<F> {
        const { assert!(size_of::<F>() == size_of::<*mut c_void>()) };
        let name = CStr::from_bytes_with_nul(name.as_bytes()).expect("a NUL-terminated name");
        let p = unsafe { dlsym(self.0, name.as_ptr()) };
        (!p.is_null()).then(|| unsafe { std::mem::transmute_copy::<*mut c_void, F>(&p) })
    }
}
