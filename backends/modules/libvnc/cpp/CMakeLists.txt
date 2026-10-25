# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Two builds from one file. Under Gradle this is a root, configured by the NDK
# toolchain, and makes `libremotedesktop_libvnc.so`: the session, with the
# module's Rust crate linked in as the JNI layer over it, and the vendored
# libjpeg-turbo and the GnuTLS port, since a phone has neither. Under `backends/CMakeLists.txt`
# it is a subdirectory on a Linux host and makes `vento_libvnc`, the session
# alone, against the root's libjpeg-turbo and the GnuTLS the host has, with a
# probe in front of it. What is shared is the middle — how LibVNCServer itself is
# configured — and the two halves around it are `if(ANDROID)` and `else()`,
# so that the Android build is the same sequence of statements it was.

cmake_minimum_required(VERSION 3.30)
project(remotedesktop_libvnc C)

get_filename_component(THIRD_PARTY "${CMAKE_CURRENT_SOURCE_DIR}/../../../../third_party" ABSOLUTE)
get_filename_component(VENTO_BACKENDS "${CMAKE_CURRENT_SOURCE_DIR}/../../.." ABSOLUTE)

if(NOT EXISTS "${THIRD_PARTY}/libvncserver/CMakeLists.txt")
    message(FATAL_ERROR "third_party/libvncserver is empty — run `git submodule update --init`")
endif()

# What is baked into the phone's library is pinned so that a build elsewhere
# matches; a host build is for running, and keeps its real paths for gdb.
if(ANDROID)
    include("${VENTO_BACKENDS}/cmake/reproducible.cmake")
endif()
include("${VENTO_BACKENDS}/cmake/warnings.cmake")
if(ANDROID)
    include("${VENTO_BACKENDS}/platform/android/cmake/vento_android.cmake")
else()
    include("${VENTO_BACKENDS}/platform/linux/cmake/vento_linux.cmake")
endif()

# One shared library per protocol, as every other backend module does, so a
# build that drops this protocol drops the whole of it. Everything below is
# linked into it statically — and into the host's static library the same way.
set(BUILD_SHARED_LIBS OFF CACHE BOOL "" FORCE)
set(BUILD_TESTING OFF CACHE BOOL "" FORCE)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)

# We import these projects for their static libraries only; neutering
# install/export stops them warning about targets nobody will install.
macro(install)
endmacro()
macro(export)
endmacro()

if(ANDROID)
    # AGP configures the debug variant at -O0, which for a JPEG and zlib decoder is
    # several times the CPU of a release one — and the hot-path measurements are
    # taken on debug builds. A trailing -O2 wins over the toolchain's earlier -O0.
    # -DNDEBUG goes with it deliberately: upstream's asserts are invariant checks
    # that abort the app, and a measurement build is not where they should fire.
    string(APPEND CMAKE_C_FLAGS_DEBUG " -O2 -DNDEBUG")

    # Shims find_package(JPEG) and find_package(GnuTLS) onto the in-tree
    # libjpeg-turbo and the port below; prepended so they win over the copies
    # CMake ships, which would probe the host. Android only: on the host those
    # copies finding the system libraries is the point.
    list(PREPEND CMAKE_MODULE_PATH "${CMAKE_CURRENT_SOURCE_DIR}/cmake")

    # libjpeg-turbo, for Tight — the encoding this backend exists to have. The SIMD
    # path on x86 needs NASM, which is a host tool this build does not require; on
    # arm64, the one ABI a phone uses, it is Neon intrinsics the compiler builds.
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

    # GnuTLS, which is one of the two libraries libvncclient reaches TLS through
    # and the one TigerVNC's client requires — so the port is built once and used
    # by both. `find_package(GnuTLS)` above finds the shim beside FindJPEG's.
    add_subdirectory("${VENTO_BACKENDS}/platform/android/ports/gnutls" gnutls)
else()
    # The host root's libjpeg-turbo (backends/CMakeLists.txt), the same
    # submodule, answered through the same FindJPEG shim: `cmake/host/` holds
    # that one alone, since GnuTLS on the host is still the system's, found
    # by CMake's own module. jconfig.h is generated there, so both of its
    # directories go on the path before libvncserver's sources are added.
    include_directories(${VENTO_JPEG_INCLUDE_DIRS})
    list(PREPEND CMAKE_MODULE_PATH "${CMAKE_CURRENT_SOURCE_DIR}/cmake/host")
endif()

# LibVNCServer, of which we build the client half only. These are cache
# variables, and under the host root the three vendored trees share one cache:
# FreeRDP has a WITH_FFMPEG, a WITH_JPEG and a WITH_OPENSSL of its own, so each
# tree's answers are forced here, immediately before the tree reads them, and
# nothing is left to an `option()` default that a neighbour may have taken.
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

# Their target carries no include directories of its own — their build puts
# the tree on the path globally — so the two are named here, as SYSTEM.
set(LIBVNC_INCLUDES
        "${THIRD_PARTY}/libvncserver/include"
        "${CMAKE_CURRENT_BINARY_DIR}/libvncserver/include")

if(ANDROID)
    # The session, which is the shim over libvncclient and names no platform,
    # and the Rust crate at the module's root, which is the backend
    # over the session and the JNI layer every backend shares.
    add_library(remotedesktop_libvnc SHARED libvnc_session.c)
    vento_android_library(remotedesktop_libvnc remotedesktop_libvnc)
    remotedesktop_warnings(remotedesktop_libvnc)
    target_include_directories(remotedesktop_libvnc SYSTEM PRIVATE ${LIBVNC_INCLUDES})
    target_link_libraries(remotedesktop_libvnc PRIVATE vncclient)
else()
    # The session alone, for whatever desktop process links it: the neutral
    # include directory is its interface, and `vento_log` is left to the
    # platform — the probe's `vento_linux`, or the library's own copy of it
    # under VENTO_SHARED (vento_linux.cmake).
    vento_backend_library(vento_libvnc libvnc_session.c)
    target_include_directories(vento_libvnc SYSTEM PRIVATE ${LIBVNC_INCLUDES})
    target_link_libraries(vento_libvnc ${VENTO_BACKEND_LINK} vncclient)

    vento_backend_probe(vento_libvnc libvnc_probe.c)
endif()
