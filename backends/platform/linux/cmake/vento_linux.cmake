# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The Linux platform for a C backend, as `vento_android.cmake` is the
# Android one: `backends/platform/linux`, compiled once as two object
# libraries. `vento_linux` is the platform proper — `cpp/vento_log.c`, onto
# stderr, which is all a session file needs of one to link — and carries the
# neutral include directory, so a target that links it finds
# `<vento/backend.h>`. `vento_probe` is the probe body, `probe/probe.c`, which
# each module's host branch puts a `main` of its own in front of, and carries
# `probe/` for its header. Object libraries do not pass their objects on
# through one another, so a probe links both by name.
#
# The probes are a development tool, built only when asked for with
# `-DVENTO_PROBES=ON`: off, `vento_backend_probe` does nothing and the probe
# body is never compiled.
#
# Guarded, because the host root includes it once and each module includes it
# again for the case where a module is configured on its own.
#
# `VENTO_SHARED` is the other shape a backend can take on the host: a shared
# library per backend, `libvento_<name>.so` under `lib/` in the build tree,
# carrying its vendored static dependencies and the stderr `vento_log`, so that
# a process written in something other than C links them by name and nothing
# else of theirs. Off, and each backend is the static library the probes are
# built on, which is what it was. `vento_backend_library` and
# `vento_backend_probe` are the two helpers that know the difference, so that a
# module's host branch does not.

get_filename_component(_VENTO_LINUX "${CMAKE_CURRENT_LIST_DIR}/.." ABSOLUTE)
get_filename_component(_VENTO_BACKENDS "${_VENTO_LINUX}/../.." ABSOLUTE)

option(VENTO_SHARED "Build each host backend as a shared library carrying its dependencies and the logger" OFF)
option(VENTO_PROBES "Build a probe in front of each host backend" OFF)

if(NOT TARGET vento_linux)
    add_library(vento_linux OBJECT "${_VENTO_LINUX}/cpp/vento_log.c")
    remotedesktop_warnings(vento_linux)
    # Its object goes into a shared library under VENTO_SHARED, and it is
    # compiled before any module has asked for position-independent code.
    set_target_properties(vento_linux PROPERTIES POSITION_INDEPENDENT_CODE ON)
    target_include_directories(vento_linux PUBLIC "${_VENTO_BACKENDS}/api/include")

    if(VENTO_PROBES)
        add_library(vento_probe OBJECT "${_VENTO_LINUX}/probe/probe.c")
        remotedesktop_warnings(vento_probe)
        target_include_directories(vento_probe PUBLIC "${_VENTO_LINUX}/probe")
        target_link_libraries(vento_probe PUBLIC vento_linux)
    endif()
endif()

# A backend's host library: `vento_<name>` from the session file and whatever
# platform sources go with it, with the neutral include directory and the
# module's own `include/` — where its `vento/<name>.h` is, since nothing else
# includes that header — as its interface. Static by default; under VENTO_SHARED a shared library with the
# logger's object inside it, every symbol out of the static archives it is
# built on hidden (`--exclude-libs`), so that what it exports is the C API in
# `vento/<name>.h` and `vento_log` — four of these in one process must not
# interpose on each other's copies of a library — and all four put in one
# `lib/` directory for a consumer to point an rpath at. The module still adds
# its own includes and the libraries it stands on afterwards, through
# `${VENTO_BACKEND_LINK}`: PUBLIC for a static library, whose probe has to
# link them too, and PRIVATE for a shared one, which already holds them.
# `VENTO_FFMPEG_PREFIX`: the FFmpeg to link, as the static libraries the
# Linux platform crate builds from `third_party/ffmpeg` and
# `third_party/dav1d` (backends/platform/linux/build.rs). Given, it is the one
# FFmpeg of every backend here — which is FreeRDP's scaler alone now, since
# every backend's H.264 and sound is decoded by the Rust side through vento's
# decoders — and the Rust decoders link the same build, so nothing here has a
# system FFmpeg in it; left empty, a module finds the host's as before, which is
# what a plain `cmake -S backends` for the probes does.
#
# One imported target per library, each carrying what a static link of it
# needs after it — dav1d and libswresample after libavcodec (its AV1 and its
# Opus decoder), libavutil after all, the libva shim after libavutil (its
# VAAPI code calls libva through it: backends/platform/linux/cpp/va_shim.c,
# which loads libva only when something decodes in hardware, and no backend
# here asks to), and the system's threads, libm and libdl — so that a module
# names only the library it calls.
set(VENTO_FFMPEG_PREFIX "" CACHE PATH "FFmpeg and dav1d built statically under this prefix, or empty for the host's FFmpeg")
if(VENTO_FFMPEG_PREFIX AND NOT TARGET vento_ffmpeg::avcodec)
    find_package(Threads REQUIRED)
    foreach(lib vento_va avutil dav1d swresample swscale avcodec)
        set(_file "${VENTO_FFMPEG_PREFIX}/lib/lib${lib}.a")
        if(NOT EXISTS "${_file}")
            message(FATAL_ERROR "VENTO_FFMPEG_PREFIX has no ${_file}")
        endif()
        add_library(vento_ffmpeg::${lib} STATIC IMPORTED GLOBAL)
        set_target_properties(vento_ffmpeg::${lib} PROPERTIES
                IMPORTED_LOCATION "${_file}"
                INTERFACE_INCLUDE_DIRECTORIES "${VENTO_FFMPEG_PREFIX}/include")
    endforeach()
    target_link_libraries(vento_ffmpeg::vento_va INTERFACE ${CMAKE_DL_LIBS})
    target_link_libraries(vento_ffmpeg::avutil INTERFACE vento_ffmpeg::vento_va Threads::Threads m)
    target_link_libraries(vento_ffmpeg::dav1d INTERFACE Threads::Threads ${CMAKE_DL_LIBS})
    target_link_libraries(vento_ffmpeg::swresample INTERFACE vento_ffmpeg::avutil)
    target_link_libraries(vento_ffmpeg::swscale INTERFACE vento_ffmpeg::avutil)
    target_link_libraries(vento_ffmpeg::avcodec INTERFACE
            vento_ffmpeg::dav1d vento_ffmpeg::swresample vento_ffmpeg::avutil)
endif()

if(VENTO_SHARED)
    set(VENTO_BACKEND_LINK PRIVATE)
else()
    set(VENTO_BACKEND_LINK PUBLIC)
endif()

function(vento_backend_library name)
    if(VENTO_SHARED)
        add_library(${name} SHARED ${ARGN})
        target_link_libraries(${name} PRIVATE vento_linux)
        target_link_options(${name} PRIVATE "LINKER:--exclude-libs,ALL")
        set_target_properties(${name} PROPERTIES LIBRARY_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/lib")
    else()
        add_library(${name} STATIC ${ARGN})
    endif()
    remotedesktop_warnings(${name})
    target_include_directories(${name} PUBLIC "${_VENTO_BACKENDS}/api/include"
            "${CMAKE_CURRENT_SOURCE_DIR}/include")
endfunction()

# The probe in front of it: the body from `vento_probe` and, for a static
# library, the logger it does not carry. Nothing without VENTO_PROBES.
function(vento_backend_probe name source)
    if(NOT VENTO_PROBES)
        return()
    endif()
    add_executable(${name}_probe ${source})
    remotedesktop_warnings(${name}_probe)
    target_link_libraries(${name}_probe PRIVATE ${name} vento_probe)
    if(NOT VENTO_SHARED)
        target_link_libraries(${name}_probe PRIVATE vento_linux)
    endif()
endfunction()
