# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The host build of the C backends: the session files under
# `modules/*/src/main/cpp`, against the libraries a Linux desktop has instead
# of the ports and vendored trees the phone needs, with a probe in front of
# each that connects, prints every callback and writes the framebuffer out.
# Three of them are open clients; the fourth, RealVNC, is their proprietary
# Android core run in this process by the hosting layer under
# `platform/desktop/realvnc`, from two libraries the user takes out of the
# viewer's APK (README.md says how) — the build needs nothing of theirs, the
# probe needs the two files.
# Gradle never reads this file — each module's CMakeLists is its own root
# there, under the NDK toolchain — so what it is for is development: a
# session file that builds and connects here has been proven through the
# neutral C API alone, and a desktop client of these backends would start
# from the same three targets.
#
#   cmake -S backends -B build/scratch/host -G Ninja
#   cmake --build build/scratch/host
#   build/scratch/host/libvnc/vento_libvnc_probe HOST:1 --password ... --out shot.ppm
#
# Options: -DSTRICT_WARNINGS=ON makes a warning in our own sources an error,
# as `scripts/check-warnings.sh` does for the phone's build; CMAKE_BUILD_TYPE
# defaults to RelWithDebInfo, since the decoders at -O0 are not what anybody
# wants to watch and a probe with no line numbers is not either;
# -DVENTO_REALVNC=OFF leaves the RealVNC module out, which it is anyway under
# a compiler other than clang — libc-shim, which the hosting layer stands on,
# does not build under GCC — with a message saying so rather than an error;
# -DVENTO_SHARED=ON makes each `vento_<name>` a shared library under `lib/`
# carrying its dependencies and the logger, which is how the Rust client under
# `linux/` links them (its build.rs runs this file with that switch on);
# -DVENTO_FFMPEG_PREFIX=DIR links FreeRDP's scaler against the static
# FFmpeg under DIR instead of the host's, which is how that build
# gives it the one `backends/platform/desktop/build.rs` made (see
# cmake/vento_desktop.cmake).
#
# Needs, as development packages: openssl, gnutls, nettle (with hogweed and
# gmp), libjpeg-turbo, pixman, zlib, FFmpeg's libswscale and libavutil
# (unless VENTO_FFMPEG_PREFIX is given; FreeRDP links the host's libavcodec
# too where there is one, for its camera channel's MJPEG, which nothing
# loads), plus
# pkg-config, and a C and C++ compiler
# CMake can find — clang for RealVNC (`CC=clang CXX=clang++ cmake ...`), whose
# four host-only submodules are initialised with `--recursive`.

cmake_minimum_required(VERSION 3.30)
project(vento_backends C CXX)

if(ANDROID)
    message(FATAL_ERROR "this is the host build; Gradle configures each module's own CMakeLists")
endif()

if(NOT CMAKE_BUILD_TYPE)
    set(CMAKE_BUILD_TYPE RelWithDebInfo CACHE STRING "" FORCE)
endif()

include(cmake/warnings.cmake)
include(cmake/vento_desktop.cmake)

# Four roots under Gradle, four subdirectories here; each branches on
# `ANDROID` and takes its host half. The vendored trees under them share
# this one cache, which is why every option a tree reads is forced in its
# module rather than left to a default.
add_subdirectory(modules/libvnc/src/main/cpp libvnc)
add_subdirectory(modules/tigervnc/src/main/cpp tigervnc)
add_subdirectory(modules/freerdp/src/main/cpp freerdp)

option(VENTO_REALVNC "Build the RealVNC backend's host half (needs clang)" ON)
if(NOT VENTO_REALVNC)
    message(STATUS "RealVNC: skipped (VENTO_REALVNC is off)")
elseif(NOT CMAKE_CXX_COMPILER_ID MATCHES "Clang" OR NOT CMAKE_C_COMPILER_ID MATCHES "Clang")
    message(STATUS "RealVNC: skipped, since its hosting layer needs clang and this is "
            "${CMAKE_CXX_COMPILER_ID} (CC=clang CXX=clang++ cmake ... to build it)")
else()
    add_subdirectory(modules/realvnc/src/main/cpp realvnc)
endif()
