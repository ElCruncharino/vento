# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The host build of the C backends: the session files under
# `modules/*/cpp`, against the libraries a Linux desktop has instead
# of the ports and vendored trees the phone needs, and, with `-DVENTO_PROBES=ON`,
# a probe in front of each that connects, prints every callback and writes the
# framebuffer out. Gradle never reads this file — each module's CMakeLists is
# its own root there, under the NDK toolchain — so it is the C backends' build
# for development: a session file that builds and connects here has been proven
# through the neutral C API alone.
#
#   cmake -S backends -B build/scratch/host -G Ninja -DVENTO_PROBES=ON
#   cmake --build build/scratch/host
#   build/scratch/host/libvnc/vento_libvnc_probe HOST:1 --password ... --out shot.ppm
#
# Options: -DVENTO_PROBES=ON builds the three probes (and FreeRDP's tests,
# which `ctest` runs), which are off by default; -DSTRICT_WARNINGS=ON makes a
# warning in our own sources an error, as `scripts/check-warnings.sh` does for
# the phone's build; CMAKE_BUILD_TYPE defaults to RelWithDebInfo, since the
# decoders at -O0 are not what anybody wants to watch and a probe with no line
# numbers is not either; -DVENTO_SHARED=ON makes each `vento_<name>` a shared
# library under `lib/` carrying its dependencies and the logger;
# -DVENTO_FFMPEG_PREFIX=DIR links FreeRDP's scaler against the static FFmpeg
# under DIR instead of the host's, such as the one
# `backends/platform/linux/build.rs` made (see
# platform/linux/cmake/vento_linux.cmake).
#
# Needs, as development packages: openssl, gnutls, nettle (with hogweed and
# gmp), libjpeg-turbo, pixman, zlib, FFmpeg's libswscale and libavutil
# (unless VENTO_FFMPEG_PREFIX is given; FreeRDP links the host's libavcodec
# too where there is one, for its camera channel's MJPEG, which nothing
# loads), plus
# pkg-config, and a C and C++ compiler CMake can find.

cmake_minimum_required(VERSION 3.30)
project(vento_backends C CXX)

if(ANDROID)
    message(FATAL_ERROR "this is the host build; Gradle configures each module's own CMakeLists")
endif()

if(NOT CMAKE_BUILD_TYPE)
    set(CMAKE_BUILD_TYPE RelWithDebInfo CACHE STRING "" FORCE)
endif()

# Besides its link-time optimisation, which its module turns off, one
# setting of FreeRDP's reaches past its own tree: its C standard, which its
# cmake/ProjectCStandard.cmake caches. On a first configure everything
# made before it — vento_linux.cmake's logger, libvncserver, TigerVNC — is
# past before the entry exists, and on every later one reads it, so their C
# was the compiler's default or gnu11 by the tree's history. Cached here,
# ahead of all of them, it is gnu11 from the first configure: what a tree
# that had ever been reconfigured was built as already.
set(CMAKE_C_STANDARD 11 CACHE STRING "")
set(CMAKE_C_STANDARD_REQUIRED ON CACHE BOOL "")

# The sessions' C++ is C++17 (TigerVNC's uses std::optional and
# std::shared_mutex), which nothing asked for while every compiler it met
# defaulted to it: clang from 16 and GCC from 11 do, the NDK's clang does,
# and Gradle never reads this file. An older one (clang 14, Ubuntu 22.04's)
# defaults to C++14 and fails the build. Asked for here, and only here, so
# the phone's libraries are what they were.
set(CMAKE_CXX_STANDARD 17 CACHE STRING "")
set(CMAKE_CXX_STANDARD_REQUIRED ON CACHE BOOL "")

include(cmake/warnings.cmake)
include(platform/linux/cmake/vento_linux.cmake)

# A root each under Gradle, a subdirectory each here; each branches on
# `ANDROID` and takes its host half. The vendored trees under them share this
# one cache, which is why every option a tree reads is forced in its module
# rather than left to a default. With the probes, `ctest` in the build tree
# runs the tests a module has beside its probe (so far FreeRDP's,
# freerdp_test.c).
if(VENTO_PROBES)
    enable_testing()
endif()

add_subdirectory(modules/libvnc/cpp libvnc)
add_subdirectory(modules/tigervnc/cpp tigervnc)
add_subdirectory(modules/freerdp/cpp freerdp)
