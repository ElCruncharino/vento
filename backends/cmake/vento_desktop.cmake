# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The desktop platform for a C backend, as `vento_jni.cmake` is the Android
# one: `backends/platform/desktop/c`, compiled once as two object libraries.
# `vento_desktop` is the platform proper — `vento_log` onto stderr, which is
# all a session file needs of one to link — and carries the neutral include
# directory, so a target that links it finds `<vento/backend.h>`.
# `vento_probe` is the probe body, which each module's host branch puts a
# `main` of its own in front of. Object libraries do not pass their objects on
# through one another, so a probe links both by name.
#
# Guarded, because the host root includes it once and each module includes it
# again for the case where a module is configured on its own; the TigerVNC
# audio sink is not here but under `platform/desktop/tigervnc`, compiled by
# that module, since it speaks TigerVNC's headers.
#
# `VENTO_SHARED` is the other shape a backend can take on the host: a shared
# library per backend, `libvento_<name>.so` under `lib/` in the build tree,
# carrying its vendored static dependencies and the stderr `vento_log`, so that
# a process written in something other than C — the Rust client under `linux/`
# — links the four by name and nothing else of theirs. Off, and each backend
# is the static library the probes are built on, which is what it was.
# `vento_backend_library` and `vento_backend_probe` are the two helpers that
# know the difference, so that a module's host branch does not.

get_filename_component(_VENTO_BACKENDS "${CMAKE_CURRENT_LIST_DIR}/.." ABSOLUTE)

option(VENTO_SHARED "Build each host backend as a shared library carrying its dependencies and the logger" OFF)

if(NOT TARGET vento_desktop)
    add_library(vento_desktop OBJECT "${_VENTO_BACKENDS}/platform/desktop/c/vento_log.c")
    remotedesktop_warnings(vento_desktop)
    # Its object goes into a shared library under VENTO_SHARED, and it is
    # compiled before any module has asked for position-independent code.
    set_target_properties(vento_desktop PROPERTIES POSITION_INDEPENDENT_CODE ON)
    target_include_directories(vento_desktop PUBLIC
            "${_VENTO_BACKENDS}/common/include"
            "${_VENTO_BACKENDS}/platform/desktop/c")

    add_library(vento_probe OBJECT "${_VENTO_BACKENDS}/platform/desktop/c/probe.c")
    remotedesktop_warnings(vento_probe)
    target_link_libraries(vento_probe PUBLIC vento_desktop)
endif()

# A backend's host library: `vento_<name>` from the session file and whatever
# platform sources go with it, with the neutral include directory as its
# interface. Static by default; under VENTO_SHARED a shared library with the
# logger's object inside it, every symbol out of the static archives it is
# built on hidden (`--exclude-libs`), so that what it exports is the C API in
# `vento/<name>.h` and `vento_log` — four of these in one process must not
# interpose on each other's copies of a library — and all four put in one
# `lib/` directory for a consumer to point an rpath at. The module still adds
# its own includes and the libraries it stands on afterwards, through
# `${VENTO_BACKEND_LINK}`: PUBLIC for a static library, whose probe has to
# link them too, and PRIVATE for a shared one, which already holds them.
if(VENTO_SHARED)
    set(VENTO_BACKEND_LINK PRIVATE)
else()
    set(VENTO_BACKEND_LINK PUBLIC)
endif()

function(vento_backend_library name)
    if(VENTO_SHARED)
        add_library(${name} SHARED ${ARGN})
        target_link_libraries(${name} PRIVATE vento_desktop)
        target_link_options(${name} PRIVATE "LINKER:--exclude-libs,ALL")
        set_target_properties(${name} PROPERTIES LIBRARY_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/lib")
    else()
        add_library(${name} STATIC ${ARGN})
    endif()
    remotedesktop_warnings(${name})
    target_include_directories(${name} PUBLIC "${_VENTO_BACKENDS}/common/include")
endfunction()

# The probe in front of it: the body from `vento_probe` and, for a static
# library, the logger it does not carry.
function(vento_backend_probe name source)
    add_executable(${name}_probe ${source})
    remotedesktop_warnings(${name}_probe)
    target_link_libraries(${name}_probe PRIVATE ${name} vento_probe)
    if(NOT VENTO_SHARED)
        target_link_libraries(${name}_probe PRIVATE vento_desktop)
    endif()
endfunction()
