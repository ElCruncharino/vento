# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Two builds from one file. Under Gradle this is a root, configured by the NDK
# toolchain, and makes `libremotedesktop_freerdp.so`: the session, with the
# module's Rust crate linked in as the JNI layer over it and the OpenSSL port and FreeRDP's
# own MediaCodec and OpenSL ES behind its H.264 and sound. Under
# `backends/CMakeLists.txt` it is a subdirectory on a Linux host and makes
# `vento_freerdp`, the session alone, against the host's OpenSSL, with FFmpeg
# and PulseAudio behind the same two, and a probe in front of it. What is
# shared is the middle — how FreeRDP itself is configured — and the two halves
# around it are `if(ANDROID)` and `else()`, so that the Android build is the
# same sequence of statements it was.

cmake_minimum_required(VERSION 3.30)
project(remotedesktop_freerdp C)

get_filename_component(THIRD_PARTY "${CMAKE_CURRENT_SOURCE_DIR}/../../../../../../third_party" ABSOLUTE)
get_filename_component(VENTO_BACKENDS "${CMAKE_CURRENT_SOURCE_DIR}/../../../../.." ABSOLUTE)

if(NOT EXISTS "${THIRD_PARTY}/freerdp/CMakeLists.txt")
    message(FATAL_ERROR "third_party/freerdp is empty — run `git submodule update --init`")
endif()

# What is baked into the phone's library is pinned so that a build elsewhere
# matches; a host build is for running, and keeps its real paths for gdb.
if(ANDROID)
    include("${VENTO_BACKENDS}/cmake/reproducible.cmake")
endif()
include("${VENTO_BACKENDS}/cmake/warnings.cmake")
if(ANDROID)
    include("${VENTO_BACKENDS}/cmake/vento_android.cmake")
else()
    include("${VENTO_BACKENDS}/cmake/vento_desktop.cmake")
endif()

# One shared library per protocol, as every other backend module does, so a
# build that drops this protocol drops the whole of it. Everything below is
# linked into it statically — and into the host's static library the same way.
set(BUILD_SHARED_LIBS OFF CACHE BOOL "" FORCE)
set(BUILD_TESTING OFF CACHE BOOL "" FORCE)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)

# We import this project for its static libraries only; neutering install/export
# stops it warning about targets nobody will install.
macro(install)
endmacro()
macro(export)
endmacro()

if(ANDROID)
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
endif()

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

if(ANDROID)
    # The phone's H.264 and sound are the NDK's, which FreeRDP has switches for,
    # so the host's codec and sound libraries are off by name.
    foreach(off WITH_OPENH264 WITH_FFMPEG WITH_SWSCALE WITH_ALSA WITH_PULSE WITH_DSP_FFMPEG)
        set(${off} OFF CACHE BOOL "" FORCE)
    endforeach()

    # The device behind Sound = "This phone": rdpsnd tries OpenSL ES first on
    # Android, and without it the only device left is one that discards every
    # sample. It is upstream's default here already; saying so makes the
    # libOpenSLES.so it links a decision rather than an accident.
    set(WITH_OPENSLES ON CACHE BOOL "" FORCE)
else()
    # The host's H.264 is FFmpeg's, through the same `WITH_FFMPEG` their Linux
    # client builds with: libavcodec for the video and for the AAC the sound
    # channel can carry (`WITH_DSP_FFMPEG`), libswscale for the scaling the
    # graphics pipeline does. OpenH264 would do for the video alone, but is a
    # library a distribution does not ship and FFmpeg is one every one does.
    # The device behind Sound = "This computer" is PulseAudio's, which on a
    # desktop of this decade is PipeWire answering to the same name.
    foreach(on WITH_FFMPEG WITH_DSP_FFMPEG WITH_VIDEO_FFMPEG WITH_SWSCALE WITH_PULSE)
        set(${on} ON CACHE BOOL "" FORCE)
    endforeach()
    foreach(off WITH_OPENH264 WITH_ALSA WITH_OSS)
        set(${off} OFF CACHE BOOL "" FORCE)
    endforeach()
    # Under the host root the three vendored trees share one cache, and
    # libvncserver has a WITH_OPENSSL, a WITH_JPEG and a WITH_PNG of its own
    # that it answers the other way; FreeRDP's answers are forced here,
    # immediately before its tree reads them, as that module forces its.
    set(WITH_OPENSSL ON CACHE BOOL "" FORCE)
    set(WITH_JPEG OFF CACHE BOOL "" FORCE)
    set(WITH_PNG OFF CACHE BOOL "" FORCE)
    # The rest of what their Linux configure looks for and this build has no
    # use for, off by name for the same reason as Opus and uriparser above:
    # a journal to log to, a PKCS#11 module, USB redirection, and a
    # clang-format to register a target for.
    foreach(off WITH_LIBSYSTEMD WITH_PKCS11 CHANNEL_URBDRC WITH_CLANG_FORMAT WITH_WEBVIEW)
        set(${off} OFF CACHE BOOL "" FORCE)
    endforeach()
    # Their FindFFmpeg looks for eight libraries and skips the search when
    # FFMPEG_LIBRARIES is cached. Where the Linux client's build gives a static
    # FFmpeg (VENTO_FFMPEG_PREFIX, vento_desktop.cmake) its four are cached
    # here, as the results that module would have come to, so that nothing of
    # the host's — libavformat, libavfilter, libavdevice included, which it
    # would otherwise find and link — is looked at.
    if(TARGET vento_ffmpeg::avcodec)
        set(_ffmpeg "")
        foreach(lib AVCODEC AVUTIL SWSCALE SWRESAMPLE)
            string(TOLOWER ${lib} name)
            set(${lib}_INCLUDE_DIRS "${VENTO_FFMPEG_PREFIX}/include" CACHE PATH "" FORCE)
            set(${lib}_LIBRARIES vento_ffmpeg::${name} CACHE STRING "" FORCE)
            list(APPEND _ffmpeg vento_ffmpeg::${name})
        endforeach()
        set(AVCODEC_VERSION "${VENTO_FFMPEG_AVCODEC_VERSION}" CACHE STRING "" FORCE)
        set(FFMPEG_LIBRARIES "${_ffmpeg}" CACHE STRING "" FORCE)
        set(FFMPEG_INCLUDE_DIRS "${VENTO_FFMPEG_PREFIX}/include" CACHE STRING "" FORCE)
        foreach(lib AVFORMAT AVFILTER AVDEVICE POSTPROCESS)
            unset(${lib}_LIBRARIES CACHE)
            unset(${lib}_INCLUDE_DIRS CACHE)
        endforeach()
    endif()
endif()

# The version from the hard-coded string in their cmake/GetProjectVersion.cmake
# rather than from `git describe`, which also stamps the build with a short
# commit hash whose length is whatever the clone it ran in decided. The string
# is the release's own, so the version the library reports is the same either
# way; only the revision becomes "n/a".
set(USE_VERSION_FROM_GIT_TAG OFF CACHE BOOL "" FORCE)

if(ANDROID)
    # H.264 on the phone's own decoder, through the NDK's MediaCodec — which is a
    # build switch here and was eighteen hand-written NDK declarations the last time
    # this repository wanted the same thing. Nothing is vendored and nothing is
    # added to the APK for it.
    set(WITH_MEDIACODEC ON CACHE BOOL "" FORCE)
endif()

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

add_subdirectory("${THIRD_PARTY}/freerdp" freerdp EXCLUDE_FROM_ALL)
remotedesktop_vendored_warnings("${THIRD_PARTY}/freerdp")

# Their targets carry no usable include directories under add_subdirectory —
# their own build puts them on the path globally — so the four are named here:
# the tree's and, beside each, the one their configure wrote.
set(FREERDP_INCLUDES
        "${THIRD_PARTY}/freerdp/include" "${CMAKE_CURRENT_BINARY_DIR}/freerdp/include"
        "${THIRD_PARTY}/freerdp/winpr/include" "${CMAKE_CURRENT_BINARY_DIR}/freerdp/winpr/include")

if(ANDROID)
    # The session, which is the shim over FreeRDP's client and names no
    # platform, and the Rust crate beside it in `src/main/rust`, which is the
    # backend over the session and the JNI layer every backend shares. OpenSSL's
    # headers are borrowed from the port for the TLS version constants, and as
    # SYSTEM like the rest, because linking the port would put them on the path
    # as ours.
    add_library(remotedesktop_freerdp SHARED freerdp_session.c)
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
else()
    # The session alone, for whatever desktop process links it: the neutral
    # include directory is its interface, and `vento_log` is left to the
    # platform — the probe's `vento_desktop`, or the library's own copy of it
    # under VENTO_SHARED (vento_desktop.cmake). OpenSSL's headers are the ones
    # their configure found, for the same constants.
    vento_backend_library(vento_freerdp freerdp_session.c)
    target_include_directories(vento_freerdp SYSTEM PRIVATE ${FREERDP_INCLUDES}
            ${OPENSSL_INCLUDE_DIR})
    target_link_libraries(vento_freerdp ${VENTO_BACKEND_LINK} freerdp-client freerdp winpr)

    vento_backend_probe(vento_freerdp freerdp_probe.c)
endif()
