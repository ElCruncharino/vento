# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Two builds from one file. Under Gradle this is a root, configured by the NDK
# toolchain, and makes `libremotedesktop_realvnc.so`: the one native thing the
# phone's half of this module has, the estimator read in `realvnc_traffic.c`,
# which links against nothing — the viewer core is already in the process,
# and what it reads through is a pointer their own Java handed us rather than
# a symbol of theirs. Under `backends/CMakeLists.txt` it is a subdirectory on
# a Linux host and makes `vento_realvnc`: the session in `realvnc_session.cxx`
# over the hosting layer in `host/`, which runs the core's Android build in a
# glibc process, with a probe in front of it.
#
# The host half needs clang: libc-shim does not compile under GCC (its
# `vector::insert` overloads trip -Wchanges-meaning, as of GCC 16), and the
# root skips this module when the compiler is anything else. It also needs
# the two libraries at run time, out of the viewer's APK — see README.md —
# and nothing of theirs at build time.
#
# The four submodules the host half takes are the host's only and are in no
# APK or licence page; each is pinned at the commit the hosting layer was
# proven with:
#
#   logger               minecraft-linux/logger          The Unlicense (public domain)
#   libc-shim            minecraft-linux/libc-shim       no licence file; GitHub lists none.
#                                                        Two bionic-derived headers in it carry
#                                                        AOSP's Apache-2.0 and BSD notices.
#   mcpelauncher-linker  minecraft-linux/mcpelauncher-linker  MIT (its own files); its nested
#                                                        `bionic` and `core` are AOSP, Apache-2.0
#                                                        and BSD
#   libjnivm             ChristopherHX/libjnivm          MIT

cmake_minimum_required(VERSION 3.30)
project(remotedesktop_realvnc C CXX)

get_filename_component(THIRD_PARTY "${CMAKE_CURRENT_SOURCE_DIR}/../../../../../../third_party" ABSOLUTE)
get_filename_component(VENTO_BACKENDS "${CMAKE_CURRENT_SOURCE_DIR}/../../../../.." ABSOLUTE)
get_filename_component(REPO "${CMAKE_CURRENT_SOURCE_DIR}/../../../../../.." ABSOLUTE)

if(ANDROID)
    include("${VENTO_BACKENDS}/cmake/reproducible.cmake")
endif()
include("${VENTO_BACKENDS}/cmake/warnings.cmake")

if(ANDROID)
    add_library(remotedesktop_realvnc SHARED realvnc_traffic.c)
    remotedesktop_warnings(remotedesktop_realvnc)
else()
    include("${VENTO_BACKENDS}/cmake/vento_desktop.cmake")

    # Off by default: every JNI call the core makes, as the jnivm trace prints
    # it, is how a class or method the stand-ins lack is found — the line
    # `Call Unknown ... Class=... Method=... Signature=...` before the core's
    # own assertion says exactly what to add — and is several thousand lines
    # a session otherwise.
    option(VENTO_REALVNC_TRACE "Trace every JNI call the RealVNC core makes" OFF)

    # Position-independent, as the other three modules ask of their trees: the
    # four below are static archives, and under VENTO_SHARED they are linked
    # into a shared library.
    set(CMAKE_POSITION_INDEPENDENT_CODE ON)

    foreach(sub logger libc-shim mcpelauncher-linker libjnivm)
        if(NOT EXISTS "${THIRD_PARTY}/${sub}/CMakeLists.txt")
            message(FATAL_ERROR "third_party/${sub} is empty — run `git submodule update --init --recursive`")
        endif()
    endforeach()
    if(NOT EXISTS "${THIRD_PARTY}/mcpelauncher-linker/bionic/linker/linker.cpp")
        message(FATAL_ERROR "third_party/mcpelauncher-linker's own submodules are empty — run `git submodule update --init --recursive`")
    endif()

    # Theirs, as they come: the logger first, since libc-shim and jnivm link
    # it by name. jnivm's garbage collection and debug codegen stay at their
    # defaults, which is what the hosting layer was proven with.
    set(JNIVM_ENABLE_TRACE ${VENTO_REALVNC_TRACE} CACHE BOOL "" FORCE)
    add_subdirectory("${THIRD_PARTY}/logger" logger EXCLUDE_FROM_ALL)
    add_subdirectory("${THIRD_PARTY}/libc-shim" libc-shim EXCLUDE_FROM_ALL)
    add_subdirectory("${THIRD_PARTY}/mcpelauncher-linker" mcpelauncher-linker EXCLUDE_FROM_ALL)
    add_subdirectory("${THIRD_PARTY}/libjnivm" libjnivm EXCLUDE_FROM_ALL)
    foreach(sub logger libc-shim mcpelauncher-linker libjnivm)
        remotedesktop_vendored_warnings("${THIRD_PARTY}/${sub}")
    endforeach()

    # The pins, out of the one place they are stated: `Library.java` is where
    # the hashes of the build these declarations were read from live, and the
    # session refuses any other. Generated at configure time so that a moved
    # pin cannot leave a stale copy here.
    set(LIBRARY_JAVA "${REPO}/realvnc/src/main/java/com/realvnc/vncviewer/jni/Library.java")
    set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS "${LIBRARY_JAVA}")
    file(READ "${LIBRARY_JAVA}" _library_java)
    if(NOT _library_java MATCHES "VERSION = \"([0-9.]+)\"")
        message(FATAL_ERROR "no VERSION in ${LIBRARY_JAVA}")
    endif()
    set(_pin_version "${CMAKE_MATCH_1}")
    set(_pins "// Generated by this module's CMakeLists from Library.java; do not edit.\n")
    string(APPEND _pins "#define VENTO_REALVNC_VERSION \"${_pin_version}\"\n")
    foreach(abi arm64-v8a x86_64)
        foreach(lib libvncviewer libc++_shared)
            # The key as the map has it, then the first quoted hex string after
            # it: no regex over a name with `++` in it.
            string(FIND "${_library_java}" "\"${abi}/${lib}.so\"," _at)
            if(_at LESS 0)
                message(FATAL_ERROR "no SHA-256 for ${abi}/${lib}.so in ${LIBRARY_JAVA}")
            endif()
            string(SUBSTRING "${_library_java}" ${_at} 200 _tail)
            string(REGEX MATCH "\"([0-9a-f]+)\"[ \n\r]*[,)]" _hash "${_tail}")
            set(_hash "${CMAKE_MATCH_1}")
            string(LENGTH "${_hash}" _hash_length)
            if(NOT _hash_length EQUAL 64)
                message(FATAL_ERROR "no SHA-256 for ${abi}/${lib}.so in ${LIBRARY_JAVA}")
            endif()
            string(TOUPPER "${abi}_${lib}" _name)
            string(REPLACE "-" "_" _name "${_name}")
            string(REPLACE "++" "XX" _name "${_name}")
            string(APPEND _pins "#define VENTO_REALVNC_SHA256_${_name} \"${_hash}\"\n")
        endforeach()
    endforeach()
    file(CONFIGURE OUTPUT "${CMAKE_CURRENT_BINARY_DIR}/gen/realvnc_pins.h" CONTENT "${_pins}")

    # The session and the hosting layer, for whatever desktop process links
    # them: the neutral include directory is the interface, and `vento_log` is
    # left to the platform — the probe's `vento_desktop`, or the library's own
    # copy of it under VENTO_SHARED (vento_desktop.cmake). The hosting layer's
    # headers are its own and not on the interface.
    vento_backend_library(vento_realvnc realvnc_session.cxx
            host/DesktopCore.cxx host/Loader.cxx host/Looper.cxx host/Classes.cxx host/Files.cxx)
    target_compile_features(vento_realvnc PRIVATE cxx_std_17)
    target_include_directories(vento_realvnc PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}"
            "${CMAKE_CURRENT_SOURCE_DIR}/host" "${CMAKE_CURRENT_BINARY_DIR}/gen")
    target_include_directories(vento_realvnc SYSTEM PRIVATE
            "${THIRD_PARTY}/libjnivm/include" "${THIRD_PARTY}/libc-shim/include"
            "${THIRD_PARTY}/mcpelauncher-linker/public_include")
    target_link_libraries(vento_realvnc ${VENTO_BACKEND_LINK} linker libc-shim logger jnivm
            ${CMAKE_DL_LIBS} pthread)

    vento_backend_probe(vento_realvnc realvnc_probe.cxx)
endif()
