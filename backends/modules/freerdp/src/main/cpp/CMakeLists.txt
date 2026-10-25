# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Under Gradle this is a root, configured by the NDK toolchain, and makes
# `libremotedesktop_freerdp.so`: the session, with the module's Rust crate
# linked in as the JNI layer over it, the OpenSSL port, and OpenSL ES behind its
# sound. Its H.264 is vento's decoders, the phone's MediaCodec, through the
# subsystem in h264_vento.c.

cmake_minimum_required(VERSION 3.30)
project(remotedesktop_freerdp C)

get_filename_component(THIRD_PARTY "${CMAKE_CURRENT_SOURCE_DIR}/../../../../../../third_party" ABSOLUTE)
get_filename_component(VENTO_BACKENDS "${CMAKE_CURRENT_SOURCE_DIR}/../../../../.." ABSOLUTE)

if(NOT EXISTS "${THIRD_PARTY}/freerdp/CMakeLists.txt")
    message(FATAL_ERROR "third_party/freerdp is empty — run `git submodule update --init`")
endif()

# What is baked into the phone's library is pinned so that a build elsewhere
# matches.
include("${VENTO_BACKENDS}/cmake/reproducible.cmake")
include("${VENTO_BACKENDS}/cmake/warnings.cmake")
include("${VENTO_BACKENDS}/cmake/vento_android.cmake")

# One shared library per protocol, as every other backend module does, so a
# build that drops this protocol drops the whole of it. Everything below is
# linked into it statically.
set(BUILD_SHARED_LIBS OFF CACHE BOOL "" FORCE)
set(BUILD_TESTING OFF CACHE BOOL "" FORCE)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)

# We import this project for its static libraries only; neutering install/export
# stops it warning about targets nobody will install.
macro(install)
endmacro()
macro(export)
endmacro()

string(APPEND CMAKE_C_FLAGS_DEBUG " -O2 -DNDEBUG")

# Answers find_package(OpenSSL) with the port in backends/ports/openssl instead
# of the copy CMake ships, which would either fail against a sysroot that has no
# OpenSSL or find the host's. An override of the command rather than a module on
# CMAKE_MODULE_PATH, because winpr *replaces* that path with FreeRDP's own — and
# it is winpr, in the scope where a module of ours is invisible, that looks for
# OpenSSL a second time and fails the configure when it is not there.
set(SHIM_CMAKE_DIR "${CMAKE_CURRENT_SOURCE_DIR}/cmake")
macro(find_package name)
    if("${name}" STREQUAL "OpenSSL")
        include("${SHIM_CMAKE_DIR}/FindOpenSSL.cmake")
    else()
        _find_package(${ARGV})
    endif()
endmacro()

# Upstream's own Android build (client/Android/cmake/ExternalFreeRDP.cmake) is
# where this list comes from, less everything a phone has no use for. Three of
# them are load-bearing rather than taste: a current OpenSSL keeps RC4, MD4 and
# MD5 in the legacy provider, which neither this build nor a distribution's
# library loads, and RDP needs all three on the wire. The last is ours: JSON is
# read for Azure sign-in and for keyboard layouts and timezones from files,
# none of which this build has, and saying so is what stops their configure
# looking for a parser at all.
foreach(off WITHOUT_FREERDP_3x_DEPRECATED WITHOUT_WINPR_3x_DEPRECATED WITH_INTERNAL_RC4
        WITH_INTERNAL_MD4 WITH_INTERNAL_MD5 WITH_SMARTCARD_EMULATE WITH_JSON_DISABLED)
    set(${off} ON CACHE BOOL "" FORCE)
endforeach()
# Which the first of those implies: their include/CMakeLists.txt caches it
# when it sees WITHOUT_FREERDP_3x_DEPRECATED, but only after their root has
# written every WITH_* variable it can see into the build-configuration
# string the library carries (`freerdp_get_build_config`), so the string — and
# with it the library's bytes — gained it on the second configure. Cached
# here, it is in the string from the first.
set(WITH_OPAQUE_SETTINGS ON CACHE INTERNAL "")
# Opus and uriparser each fall back to pkg-config when the package search comes
# up empty, and that can find the host's copy; off by name is the only answer
# that does not depend on what the build machine has installed.
foreach(off WITH_CLIENT_SDL WITH_CLIENT WITH_SERVER WITH_SAMPLE WITH_SHADOW WITH_PROXY
        WITH_MANPAGES WITH_AAD WITH_CUPS WITH_PCSC WITH_OSS WITH_KRB5 WITH_WINPR_TOOLS WITH_X11
        WITH_WAYLAND WITH_FUSE WITH_OPUS WITH_URIPARSER)
    set(${off} OFF CACHE BOOL "" FORCE)
endforeach()
set(WITH_CLIENT_COMMON ON CACHE BOOL "" FORCE)
set(WITH_CLIENT_CHANNELS ON CACHE BOOL "" FORCE)

# The library's H.264 is decoded by the platform's decoders, which the session
# is handed (vento/video.h), through a subsystem of this module's: the slot
# patches/freerdp adds for one the application links in. None of the
# library's own is built, so there is one H.264 decoder to reason about —
# the one RustDesk's and TigerVNC's pictures go through, with the same
# hardware decoding and fallback — and the library's code above the decoder
# (AVC444's combining of two pictures, the conversion into the surface) is
# upstream's, unchanged.
set(WITH_H264_EXTERNAL ON CACHE BOOL "" FORCE)

# The phone's sound is the NDK's, which FreeRDP has switches for, so the
# host's codec and sound libraries are off by name; and its H.264 is
# MediaCodec's through vento's decoders, so FreeRDP's own MediaCodec
# subsystem is off too.
foreach(off WITH_OPENH264 WITH_FFMPEG WITH_SWSCALE WITH_ALSA WITH_PULSE WITH_DSP_FFMPEG
        WITH_MEDIACODEC)
    set(${off} OFF CACHE BOOL "" FORCE)
endforeach()

# The device behind Sound = "This phone": rdpsnd tries OpenSL ES first on
# Android, and without it the only device left is one that discards every
# sample. It is upstream's default here already; saying so makes the
# libOpenSLES.so it links a decision rather than an accident.
set(WITH_OPENSLES ON CACHE BOOL "" FORCE)

# The version from the hard-coded string in their cmake/GetProjectVersion.cmake
# rather than from `git describe`, which also stamps the build with a short
# commit hash whose length is whatever the clone it ran in decided. The string
# is the release's own, so the version the library reports is the same either
# way; only the revision becomes "n/a".
set(USE_VERSION_FROM_GIT_TAG OFF CACHE BOOL "" FORCE)

# Their asserts print what failed and then abort. Keep that in a build somebody
# is debugging, where an abort with a message is the finding; a release build
# gets neither, which is the same trade every other borrowed library here makes.
if(CMAKE_BUILD_TYPE STREQUAL "Debug")
    set(WITH_VERBOSE_WINPR_ASSERT ON CACHE BOOL "" FORCE)
else()
    set(WITH_VERBOSE_WINPR_ASSERT OFF CACHE BOOL "" FORCE)
endif()

# Two libraries their configure looks for and warns about not finding, both
# behind an option this build has already answered. The search is what is
# switched off rather than the warning: the answer is then the same on a machine
# that happens to have them installed, which is what the NDK toolchain's
# find-root rules only mostly guarantee. The JSON libraries need no such line,
# since WITH_JSON_DISABLED skips their detection altogether.
foreach(pkg Opus uriparser)
    set(CMAKE_DISABLE_FIND_PACKAGE_${pkg} ON)
endforeach()

# No link-time optimisation, so that the library is the same however many
# times its tree has been configured. Their cmake/CommonConfigOptions.cmake
# makes CMAKE_INTERPROCEDURAL_OPTIMIZATION an option() defaulting to whether
# the compiler can do it, and an option() is a cache entry: on a tree's first
# configure it does not yet exist when the targets made before theirs read it
# (the OpenSSL port here), on every later one it does, so which targets were
# built with -flto=thin — and the bytes of the library they make — came down to
# the tree's history. A forced cache entry answers it before their option() is
# reached, which then does nothing; and the answer is off, which is also what a
# target made before this line sees on a first configure, so every target in
# every tree is built the same way whatever the cache held.
set(CMAKE_INTERPROCEDURAL_OPTIMIZATION OFF CACHE BOOL "" FORCE)

add_subdirectory("${THIRD_PARTY}/freerdp" freerdp EXCLUDE_FROM_ALL)
remotedesktop_vendored_warnings("${THIRD_PARTY}/freerdp")

# Their targets carry no usable include directories under add_subdirectory —
# their own build puts them on the path globally — so the four are named here:
# the tree's and, beside each, the one their configure wrote.
set(FREERDP_INCLUDES
        "${THIRD_PARTY}/freerdp/include" "${CMAKE_CURRENT_BINARY_DIR}/freerdp/include"
        "${THIRD_PARTY}/freerdp/winpr/include" "${CMAKE_CURRENT_BINARY_DIR}/freerdp/winpr/include")
# The H.264 subsystem is built against the library's own declaration of the
# interface, which it keeps beside its subsystems rather than in its public
# headers; the one file that needs it is the one given it.
set_source_files_properties(h264_vento.c PROPERTIES INCLUDE_DIRECTORIES
        "${THIRD_PARTY}/freerdp/libfreerdp/codec")

# The session, which is the shim over FreeRDP's client and names no
# platform, and the Rust crate beside it in `src/main/rust`, which is the
# backend over the session and the JNI layer every backend shares. OpenSSL's
# headers are borrowed from the port for the TLS version constants, and as
# SYSTEM like the rest, because linking the port would put them on the path
# as ours.
add_library(remotedesktop_freerdp SHARED freerdp_session.c h264_vento.c)
vento_android_library(remotedesktop_freerdp remotedesktop_freerdp)
remotedesktop_warnings(remotedesktop_freerdp)
target_include_directories(remotedesktop_freerdp SYSTEM PRIVATE ${FREERDP_INCLUDES}
        "$<TARGET_PROPERTY:openssl,INTERFACE_INCLUDE_DIRECTORIES>")
target_link_libraries(remotedesktop_freerdp PRIVATE freerdp-client freerdp winpr)

# A library has one `JNI_OnLoad`, and WinPR brings one: it keeps the
# `JavaVM` it reads the time zone through. Theirs is compiled under
# another name, and the crate's — the one that registers the natives —
# calls it first (`natives!`'s `on_load`, in `src/main/rust/src/lib.rs`).
# A definition on that one source file rather than a patch: jni.h's
# declaration is renamed with it, so the two still agree.
set_property(SOURCE "${THIRD_PARTY}/freerdp/winpr/libwinpr/utils/android.c"
        TARGET_DIRECTORY winpr APPEND PROPERTY COMPILE_DEFINITIONS JNI_OnLoad=winpr_JNI_OnLoad)
