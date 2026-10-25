# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The host build of the C backends: the session files under
# `modules/*/cpp`, against the libraries a Linux desktop has instead
# of the ports and vendored trees the phone needs (but for libjpeg-turbo,
# pixman, GnuTLS and nettle, which are built from the phone's sources here
# too), and, with `-DVENTO_PROBES=ON`,
# a probe in front of each that connects, prints every callback and writes the
# framebuffer out.
# Three of them are open clients; the fourth, RealVNC, is their proprietary
# Android core run in this process by the hosting layer in its module
# (`modules/realvnc/cpp/host`), from two libraries the user takes out
# of the viewer's APK (README.md says how) — the build needs nothing of
# theirs, the probe needs the two files.
# Gradle never reads this file — each module's CMakeLists is its own root
# there, under the NDK toolchain — so it is the Linux client's build of the
# C backends (`linux/build.rs` runs it, with VENTO_SHARED on) and
# development's: a session file that builds and connects here has been
# proven through the neutral C API alone.
#
#   cmake -S backends -B build/scratch/host -G Ninja -DVENTO_PROBES=ON
#   cmake --build build/scratch/host
#   build/scratch/host/libvnc/vento_libvnc_probe HOST:1 --password ... --out shot.ppm
#
# Options: -DVENTO_PROBES=ON builds the four probes (and FreeRDP's tests,
# which `ctest` runs), which are off by default
# so that the Linux client's build of this tree compiles none of them;
# -DSTRICT_WARNINGS=ON makes a warning in our own sources an error,
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
# gives it the one `backends/platform/linux/build.rs` made (see
# platform/linux/cmake/vento_linux.cmake).
#
# Needs, as development packages: openssl, zlib, FFmpeg's libswscale and
# libavutil
# (unless VENTO_FFMPEG_PREFIX is given; FreeRDP links the host's libavcodec
# too where there is one, for its camera channel's MJPEG, which nothing
# loads), plus
# pkg-config, make and m4 (GnuTLS and nettle are built by their own
# autotools build; nettle's assembly needs m4, and is left out without it),
# nasm (libjpeg-turbo's x86 SIMD; it is C without), and a C and
# C++ compiler CMake can find — clang for RealVNC (`CC=clang CXX=clang++ cmake ...`), whose
# four host-only submodules are initialised with `--recursive`. GnuTLS and
# nettle are the release tarballs the phone's ports build, unpacked into
# `third_party/gnutls` and `third_party/nettle` by
# `./gradlew :backends:fetchCryptoSources`.

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
# the phone's libraries are what they were; a tree that sets its own (libjnivm
# per target, mcpelauncher-linker in its directory) keeps its own.
set(CMAKE_CXX_STANDARD 17 CACHE STRING "")
set(CMAKE_CXX_STANDARD_REQUIRED ON CACHE BOOL "")

include(cmake/warnings.cmake)
include(platform/linux/cmake/vento_linux.cmake)

# libjpeg-turbo, from `third_party/` as the phone has it, rather than the
# host's: libvncclient's Tight and TigerVNC's both decode with it, and it is
# linked into each of their libraries statically (`vento_linux.cmake` keeps
# its symbols out of their exports). The host's would tie a built tree to its
# distribution — libjpeg-turbo's soname is libjpeg.so.62 on Fedora and Debian
# and libjpeg.so.8 on Ubuntu, where .so.62 is IJG's 6b — so nothing of it is
# looked for. Built once here, where the phone builds a copy per module root,
# since the two modules share this tree and a target can have one name in it;
# it is generated when it is configured (jconfig.h), as a host build may be.
# Its options are its own and go into the one cache, so they are forced, and
# the scope is a block's so that position-independent code, which a static
# archive linked into a shared library needs, is asked of it alone here.
# NASM, which the video decoders want already, gives it its x86 SIMD; without
# it it builds in C. The build string is the fixed one the phone's
# `reproducible.cmake` gives it, not the date of the build. Its warnings are
# not silenced as the other vendored trees' are: that `-w` reaches its NASM
# sources too, where it is not the flag it is to a C compiler.
if(NOT EXISTS "${CMAKE_CURRENT_SOURCE_DIR}/../third_party/libjpeg-turbo/CMakeLists.txt")
    message(FATAL_ERROR "third_party/libjpeg-turbo is empty — run `git submodule update --init`")
endif()
block(SCOPE_FOR VARIABLES)
    macro(install)
    endmacro()
    macro(export)
    endmacro()
    set(CMAKE_POSITION_INDEPENDENT_CODE ON)
    set(ENABLE_SHARED OFF CACHE BOOL "" FORCE)
    set(ENABLE_STATIC ON CACHE BOOL "" FORCE)
    set(WITH_TURBOJPEG OFF CACHE BOOL "" FORCE)
    set(WITH_TESTS OFF CACHE BOOL "" FORCE)
    set(WITH_FUZZ OFF CACHE BOOL "" FORCE)
    set(BUILD "0" CACHE STRING "" FORCE)
    # It refuses a blank documentation or manual directory, which is what
    # CMake's own GNUInstallDirs caches for both (FreeRDP's and libvncserver's
    # include it) in a tree configured before this was here; a value for its
    # check, where the cache has a blank one. (Where the cache has none, as
    # on a first configure, it makes its own, and a variable here would have
    # it set a property on a cache entry that does not exist.) Nothing is
    # installed from this tree, so any value does.
    foreach(dir DOCDIR MANDIR)
        if(DEFINED CACHE{CMAKE_INSTALL_${dir}} AND "$CACHE{CMAKE_INSTALL_${dir}}" STREQUAL "")
            set(CMAKE_INSTALL_${dir} ${dir})
        endif()
    endforeach()
    add_subdirectory("${CMAKE_CURRENT_SOURCE_DIR}/../third_party/libjpeg-turbo" libjpeg-turbo EXCLUDE_FROM_ALL)
endblock()
# Its headers are in two places, since jconfig.h is generated: the modules
# put both on their path with the target, `jpeg-static`.
set(VENTO_JPEG_INCLUDE_DIRS
        "${CMAKE_CURRENT_SOURCE_DIR}/../third_party/libjpeg-turbo/src"
        "${CMAKE_CURRENT_BINARY_DIR}/libjpeg-turbo")

# GnuTLS and nettle, from the tarballs the phone's ports build, configured
# and built by their own autotools build at build time (the file says why).
# libvncclient and TigerVNC link them in, as they do libjpeg-turbo.
include(platform/linux/cmake/gnutls.cmake)

# Four roots under Gradle, four subdirectories here; each branches on
# `ANDROID` and takes its host half. The vendored trees under them share
# this one cache, which is why every option a tree reads is forced in its
# module rather than left to a default.
# With the probes, `ctest` in the build tree runs the tests a module has
# beside its probe (so far FreeRDP's, freerdp_test.c).
if(VENTO_PROBES)
    enable_testing()
endif()

add_subdirectory(modules/libvnc/cpp libvnc)
add_subdirectory(modules/tigervnc/cpp tigervnc)
add_subdirectory(modules/freerdp/cpp freerdp)

option(VENTO_REALVNC "Build the RealVNC backend's host half (needs clang)" ON)
if(NOT VENTO_REALVNC)
    message(STATUS "RealVNC: skipped (VENTO_REALVNC is off)")
elseif(NOT CMAKE_CXX_COMPILER_ID MATCHES "Clang" OR NOT CMAKE_C_COMPILER_ID MATCHES "Clang")
    message(STATUS "RealVNC: skipped, since its hosting layer needs clang and this is "
            "${CMAKE_CXX_COMPILER_ID} (CC=clang CXX=clang++ cmake ... to build it)")
else()
    add_subdirectory(modules/realvnc/cpp realvnc)
endif()
