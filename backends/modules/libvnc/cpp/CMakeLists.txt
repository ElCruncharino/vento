# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Two builds from one file. Under Gradle this is a root project, configured by
# the NDK toolchain, and builds `libremotedesktop_libvnc.so`: the session, with
# the module's Rust crate linked in as its JNI layer, plus the vendored
# libjpeg-turbo and the GnuTLS port, since a phone has neither. Under
# `backends/CMakeLists.txt` it is a subdirectory on a Linux host and builds
# `vento_libvnc`, the session alone, against the root's libjpeg-turbo and
# GnuTLS, optionally with a probe. The LibVNCServer configuration in the middle
# is shared; the parts before and after it are split by `if(ANDROID)`.

cmake_minimum_required(VERSION 3.30)
project(remotedesktop_libvnc C)

get_filename_component(THIRD_PARTY "${CMAKE_CURRENT_SOURCE_DIR}/../../../../third_party" ABSOLUTE)
get_filename_component(VENTO_BACKENDS "${CMAKE_CURRENT_SOURCE_DIR}/../../.." ABSOLUTE)

if(NOT EXISTS "${THIRD_PARTY}/libvncserver/CMakeLists.txt")
    message(FATAL_ERROR "third_party/libvncserver is empty — run `git submodule update --init`")
endif()

# The phone's library is built without build paths and timestamps so that a
# build elsewhere matches; a host build keeps its real paths for gdb.
if(ANDROID)
    include("${VENTO_BACKENDS}/cmake/reproducible.cmake")
endif()
include("${VENTO_BACKENDS}/cmake/warnings.cmake")
if(ANDROID)
    include("${VENTO_BACKENDS}/platform/android/cmake/vento_android.cmake")
else()
    include("${VENTO_BACKENDS}/platform/linux/cmake/vento_linux.cmake")
endif()

# One shared library per protocol, as in every other backend module, so a
# build that drops this protocol drops all of it. Everything below is linked
# into it statically, and into the host's `vento_libvnc` the same way.
set(BUILD_SHARED_LIBS OFF CACHE BOOL "" FORCE)
set(BUILD_TESTING OFF CACHE BOOL "" FORCE)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)

# We import these projects for their static libraries only; disabling
# install/export stops their warnings about targets that are never installed.
macro(install)
endmacro()
macro(export)
endmacro()

if(ANDROID)
    # AGP configures the debug variant at -O0, which for a JPEG and zlib decoder
    # costs several times the CPU of a release build, and the hot-path
    # measurements are taken on debug builds. A trailing -O2 wins over the
    # toolchain's earlier -O0. -DNDEBUG is deliberate: upstream's asserts are
    # invariant checks that abort the app, and should not fire in a measurement
    # build.
    string(APPEND CMAKE_C_FLAGS_DEBUG " -O2 -DNDEBUG")

    # Points find_package(JPEG) and find_package(GnuTLS) at the in-tree
    # libjpeg-turbo and the port below; prepended so they win over the modules
    # CMake ships, which would probe the host. Android only: the host build
    # uses the root's libraries through `cmake/host/` below.
    list(PREPEND CMAKE_MODULE_PATH "${CMAKE_CURRENT_SOURCE_DIR}/cmake")

    # libjpeg-turbo, for Tight, the main reason this backend exists. The SIMD
    # path on x86 needs NASM, a host tool this build does not require; on arm64,
    # the ABI phones use, it is Neon intrinsics built by the compiler.
    set(ENABLE_SHARED OFF CACHE BOOL "" FORCE)
    set(ENABLE_STATIC ON CACHE BOOL "" FORCE)
    set(WITH_TURBOJPEG OFF CACHE BOOL "" FORCE)
    set(WITH_FUZZ OFF CACHE BOOL "" FORCE)
    if(CMAKE_ANDROID_ARCH_ABI MATCHES "^x86")
        set(WITH_SIMD OFF CACHE BOOL "" FORCE)
    endif()
    add_subdirectory("${THIRD_PARTY}/libjpeg-turbo" libjpeg-turbo EXCLUDE_FROM_ALL)

    # jconfig.h and jconfigint.h are generated, so the headers are in two places;
    # set before add_subdirectory so libvncserver's sources inherit both.
    include_directories(
            "${THIRD_PARTY}/libjpeg-turbo/src"
            "${CMAKE_CURRENT_BINARY_DIR}/libjpeg-turbo")

    # GnuTLS: one of the two TLS libraries libvncclient supports, and the one
    # TigerVNC's client requires, so both use the same port.
    # `find_package(GnuTLS)` finds the shim next to FindJPEG.cmake.
    add_subdirectory("${VENTO_BACKENDS}/platform/android/ports/gnutls" gnutls)
else()
    # The host root's libjpeg-turbo and GnuTLS (backends/CMakeLists.txt),
    # found through the modules in `cmake/host/`: the same FindJPEG as the
    # phone's, and a FindGnuTLS for the root's GnuTLS, which is built from the
    # same release tarball by autotools rather than from the port. jconfig.h
    # is generated in the build tree, so both libjpeg-turbo include directories
    # go on the path before libvncserver's sources are added.
    include_directories(${VENTO_JPEG_INCLUDE_DIRS})
    list(PREPEND CMAKE_MODULE_PATH "${CMAKE_CURRENT_SOURCE_DIR}/cmake/host")
endif()

# LibVNCServer, client half only. These are cache variables, and under the
# host root the vendored trees share one cache: FreeRDP has its own
# WITH_FFMPEG, WITH_JPEG and WITH_OPENSSL, so each value is forced here,
# immediately before the tree reads it, rather than left to an `option()`
# default that another tree may already have set.
set(WITH_LIBVNCSERVER OFF CACHE BOOL "" FORCE)
set(WITH_LIBVNCCLIENT ON CACHE BOOL "" FORCE)
set(WITH_EXAMPLES OFF CACHE BOOL "" FORCE)
set(WITH_TESTS OFF CACHE BOOL "" FORCE)
set(LIBVNCSERVER_INSTALL OFF CACHE BOOL "" FORCE)
set(WITH_ZLIB ON CACHE BOOL "" FORCE)
set(WITH_JPEG ON CACHE BOOL "" FORCE)
set(WITH_OPENSSL OFF CACHE BOOL "" FORCE)
set(WITH_GNUTLS ON CACHE BOOL "" FORCE)
set(WITH_GCRYPT OFF CACHE BOOL "" FORCE)
set(WITH_SASL OFF CACHE BOOL "" FORCE)
set(WITH_PNG OFF CACHE BOOL "" FORCE)
set(WITH_LZO OFF CACHE BOOL "" FORCE)
set(WITH_SDL OFF CACHE BOOL "" FORCE)
set(WITH_GTK OFF CACHE BOOL "" FORCE)
set(WITH_QT OFF CACHE BOOL "" FORCE)
set(WITH_XCB OFF CACHE BOOL "" FORCE)
set(WITH_FFMPEG OFF CACHE BOOL "" FORCE)
set(WITH_SYSTEMD OFF CACHE BOOL "" FORCE)
set(WITH_LIBSSHTUNNEL OFF CACHE BOOL "" FORCE)
set(WITH_WEBSOCKETS OFF CACHE BOOL "" FORCE)
set(WITH_TIGHTVNC_FILETRANSFER OFF CACHE BOOL "" FORCE)
add_subdirectory("${THIRD_PARTY}/libvncserver" libvncserver EXCLUDE_FROM_ALL)
remotedesktop_vendored_warnings("${THIRD_PARTY}/libvncserver")

# The vncclient target carries no include directories (LibVNCServer's build
# adds the tree to the path globally), so the two are listed here, as SYSTEM.
set(LIBVNC_INCLUDES
        "${THIRD_PARTY}/libvncserver/include"
        "${CMAKE_CURRENT_BINARY_DIR}/libvncserver/include")

if(ANDROID)
    # The session (the platform-independent shim over libvncclient), and the
    # Rust crate at the module's root, which wraps it as a backend and holds
    # the shared JNI layer.
    add_library(remotedesktop_libvnc SHARED libvnc_session.c)
    vento_android_library(remotedesktop_libvnc remotedesktop_libvnc)
    remotedesktop_warnings(remotedesktop_libvnc)
    target_include_directories(remotedesktop_libvnc SYSTEM PRIVATE ${LIBVNC_INCLUDES})
    target_link_libraries(remotedesktop_libvnc PRIVATE vncclient)
else()
    # The session alone, for any desktop process: the neutral include
    # directory is its interface, and `vento_log` comes from the platform
    # (the probe's `vento_linux`, or the library's own copy under VENTO_SHARED;
    # see vento_linux.cmake).
    vento_backend_library(vento_libvnc libvnc_session.c)
    target_include_directories(vento_libvnc SYSTEM PRIVATE ${LIBVNC_INCLUDES})
    target_link_libraries(vento_libvnc ${VENTO_BACKEND_LINK} vncclient)

    vento_backend_probe(vento_libvnc libvnc_probe.c)
endif()
