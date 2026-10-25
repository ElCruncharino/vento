# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The Linux platform for a C backend, as `vento_android.cmake` is the
# Android one: `backends/platform/linux`, compiled once as two object
# libraries. `vento_linux` is the platform proper (`cpp/vento_log.c`, logging
# to stderr, which is all a session file needs from it to link) and carries the
# neutral include directory, so a target that links it finds
# `<vento/backend.h>`. `vento_probe` is the probe body, `probe/probe.c`, to
# which each module's host branch adds its own `main`, and carries `probe/` for
# its header. Object libraries do not pass their objects on through one
# another, so a probe links both by name.
#
# The probes are a development tool, built only with `-DVENTO_PROBES=ON`. Off,
# `vento_backend_probe` does nothing and the probe body is never compiled,
# which suits the Linux client's build of the same tree (`linux/build.rs`,
# target `all`): it would otherwise rebuild four unused executables whenever a
# session file changes. The client's own `vento <backend> <address> --probe SPEC`
# goes through the same session files.
#
# Guarded, because the host root includes it once and each module includes it
# again for the case where a module is configured on its own.
#
# `VENTO_SHARED` is the other form a backend can take on the host: a shared
# library per backend, `libvento_<name>.so` under `lib/` in the build tree,
# carrying its vendored static dependencies and the stderr `vento_log`, so that
# a process written in something other than C (the Rust client under `linux/`)
# opens the four by name at run time and needs nothing else of theirs. Off, each
# backend is the static library the probes are built on.
# `vento_backend_library` and `vento_backend_probe` handle the difference, so
# that a module's host branch does not have to.

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

# A backend's host library: `vento_<name>` from the session file and its
# platform sources, with the neutral include directory and the module's own
# `include/` as its interface (its `vento/<name>.h` is there, since nothing
# else includes that header). Static by default. Under VENTO_SHARED it is a
# shared library with the logger's object inside it and every symbol from the
# static archives it is built on hidden (`--exclude-libs`), so that it exports
# only the C API in `vento/<name>.h` and `vento_log` (four of these in one
# process must not interpose on each other's copies of a library), and all
# four go in one `lib/` directory for a consumer's rpath. Its references to its
# own exports bind to its own definitions (`-Bsymbolic`): the client opens it
# with its symbols kept local but RealVNC's in the global scope, which a lookup
# searches first and which also exports a `vento_log` and some of the same C++
# template code. `GLOBAL` leaves that out, for RealVNC's. The module adds its
# own includes and the libraries it depends on afterwards, through
# `${VENTO_BACKEND_LINK}`: PUBLIC for a static library, whose probe has to link
# them too, and PRIVATE for a shared one, which already holds them.
# `VENTO_FFMPEG_PREFIX`: the FFmpeg to link, as the static libraries the Linux
# platform crate builds from `third_party/ffmpeg` and `third_party/dav1d`
# (backends/platform/linux/build.rs; `linux/build.rs` gets the prefix from it
# and passes it on). When set, it is the only FFmpeg of every backend here
# (only FreeRDP's scaler uses it, since every backend's H.264 and sound is
# decoded by the Rust side through vento's decoders), and the Rust decoders
# link the same build, so a Linux client has no system FFmpeg in it at all.
# When empty, as for a plain `cmake -S backends` for the probes, a module finds
# the host's.
#
# One imported target per library, each carrying the libraries a static link
# of it needs after it (dav1d and libswresample after libavcodec, for its AV1
# and Opus decoders; libavutil after all; the libva shim after libavutil, whose
# VAAPI code calls libva through it: backends/platform/linux/cpp/va_shim.c,
# which loads libva only when something decodes in hardware, and no backend
# here does; and the system's threads, libm and libdl), so that a module names
# only the library it calls.
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
    cmake_parse_arguments(PARSE_ARGV 1 _backend "GLOBAL" "" "")
    if(VENTO_SHARED)
        add_library(${name} SHARED ${_backend_UNPARSED_ARGUMENTS})
        target_link_libraries(${name} PRIVATE vento_linux)
        target_link_options(${name} PRIVATE "LINKER:--exclude-libs,ALL")
        if(NOT _backend_GLOBAL)
            target_link_options(${name} PRIVATE "LINKER:-Bsymbolic")
        endif()
        set_target_properties(${name} PROPERTIES LIBRARY_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/lib")
    else()
        add_library(${name} STATIC ${_backend_UNPARSED_ARGUMENTS})
    endif()
    remotedesktop_warnings(${name})
    target_include_directories(${name} PUBLIC "${_VENTO_BACKENDS}/api/include"
            "${CMAKE_CURRENT_SOURCE_DIR}/include")
endfunction()

# The probe for a backend library: the body from `vento_probe` and, for a
# static library, the logger it does not carry. Nothing without VENTO_PROBES.
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
