# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The host build of the three C backends: the session files under
# `modules/*/src/main/cpp`, against the libraries a Linux desktop has instead
# of the ports and vendored trees the phone needs, with a probe in front of
# each that connects, prints every callback and writes the framebuffer out.
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
# wants to watch and a probe with no line numbers is not either.
#
# Needs, as development packages: openssl, gnutls, nettle (with hogweed and
# gmp), libjpeg-turbo, pixman, zlib, FFmpeg's libavcodec, libavutil and
# libswscale, libpulse and alsa-lib, plus pkg-config, and a C and C++ compiler
# CMake can find.

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

# Three roots under Gradle, three subdirectories here; each branches on
# `ANDROID` and takes its host half. The vendored trees under them share
# this one cache, which is why every option a tree reads is forced in its
# module rather than left to a default.
add_subdirectory(modules/libvnc/src/main/cpp libvnc)
add_subdirectory(modules/tigervnc/src/main/cpp tigervnc)
add_subdirectory(modules/freerdp/src/main/cpp freerdp)
