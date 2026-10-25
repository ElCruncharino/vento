# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Two builds from one file. Under Gradle this is a root, configured by the NDK
# toolchain, and makes `libremotedesktop_freerdp.so`: the session, with the
# module's Rust crate linked in as the JNI layer over it, and the OpenSSL port.
# Under `backends/CMakeLists.txt` it is a subdirectory on a Linux host and
# makes `vento_freerdp`, the session alone, against the host's OpenSSL, with
# libswscale for the graphics pipeline's scaling, and a probe in front of it.
# Its H.264 is vento's decoders on both, the phone's MediaCodec and FFmpeg on
# Linux, through the subsystem in h264_vento.c; and its sound is vento's output
# and vento's AAC and Opus decoders on both, AAudio and MediaCodec on the phone,
# a PipeWire stream (or ALSA's default device) and FFmpeg on Linux, through the
# rdpsnd device in rdpsnd_vento.c. What is shared is the middle — how FreeRDP
# itself is configured — and the two halves around it are `if(ANDROID)` and
# `else()`, so that the Android build is the same sequence of statements it was.

cmake_minimum_required(VERSION 3.30)
project(remotedesktop_freerdp C)

get_filename_component(THIRD_PARTY "${CMAKE_CURRENT_SOURCE_DIR}/../../../../third_party" ABSOLUTE)
get_filename_component(VENTO_BACKENDS "${CMAKE_CURRENT_SOURCE_DIR}/../../.." ABSOLUTE)

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

# We import this project for its static libraries only; neutering install/export
# stops it warning about targets nobody will install.
macro(install)
endmacro()
macro(export)
endmacro()

if(ANDROID)
    string(APPEND CMAKE_C_FLAGS_DEBUG " -O2 -DNDEBUG")

    # Answers find_package(OpenSSL) with the port in
    # backends/platform/android/ports/openssl instead of the copy CMake ships,
    # which would either fail against a sysroot that has no OpenSSL or find the
    # host's. An override of the command rather than a module on
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

# The sound is played on the platform's output, which the session is handed
# (vento/audio.h), through a device of this module's that the channel finds
# by name (rdpsnd_vento.h) — the same queue and policy TigerVNC's sound goes
# through. So none of the library's own devices is built but the one that
# discards every sample, which it builds whatever it is told and which is what
# a session with its sound off or played over there gets; and none of the
# sound libraries they would link (PulseAudio and the fifteen libraries behind
# it, ALSA, OSS, sndio, OpenSL ES) is looked for. The microphone channel's
# devices go with them, which is no loss: it is built and never loaded.
#
# The AAC and Opus a server sends are decoded by the platform's decoders, in
# the same table, so the library's DSP decodes nothing it is not given as
# PCM and is built without FFmpeg (`WITH_DSP_FFMPEG`) on both: the device
# claims those formats itself and gets the packets as they came.
foreach(off WITH_PULSE WITH_ALSA WITH_OSS WITH_SNDIO WITH_OPENSLES WITH_DSP_FFMPEG)
    set(${off} OFF CACHE BOOL "" FORCE)
endforeach()

if(ANDROID)
    # No FFmpeg on the phone, so the host's codec libraries are off by name;
    # and its H.264 is MediaCodec's through vento's decoders, so FreeRDP's own
    # MediaCodec subsystem is off too.
    foreach(off WITH_OPENH264 WITH_FFMPEG WITH_SWSCALE WITH_MEDIACODEC)
        set(${off} OFF CACHE BOOL "" FORCE)
    endforeach()
else()
    # libswscale, and of FFmpeg nothing else: the library has nothing left
    # to decode (its H.264 and its sound are vento's decoders), but its
    # graphics pipeline scales a surface the server maps onto a scaled
    # output with it (`freerdp_image_scale`), and offers the server the
    # capability versions that may ask for that — 10.5 and 10.6, which
    # Windows 10 confirms — only where it has a scaler (rdpgfx_main.c), so
    # without it a session is EGFX 10.4, as the phone's is. libavcodec is
    # not linked (`WITH_FFMPEG` off, which their swscale branch would also
    # set for the camera channel's MJPEG where it finds libavcodec).
    set(WITH_SWSCALE ON CACHE BOOL "" FORCE)
    foreach(off WITH_FFMPEG WITH_VIDEO_FFMPEG WITH_FFMPEG_HWACCEL WITH_OPENH264)
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
    # FFMPEG_LIBRARIES is cached. Where a build gives a static FFmpeg
    # (VENTO_FFMPEG_PREFIX, vento_linux.cmake) the two the scaler needs are
    # cached here, as the results that module would have come to, and the other
    # six as not found, so that nothing of the host's — libavcodec,
    # libavformat, libavfilter and libavdevice included, which it would
    # otherwise find and link — is looked at.
    if(TARGET vento_ffmpeg::swscale)
        set(_ffmpeg "")
        foreach(lib SWSCALE AVUTIL)
            string(TOLOWER ${lib} name)
            set(${lib}_INCLUDE_DIRS "${VENTO_FFMPEG_PREFIX}/include" CACHE PATH "" FORCE)
            set(${lib}_LIBRARIES vento_ffmpeg::${name} CACHE STRING "" FORCE)
            list(APPEND _ffmpeg vento_ffmpeg::${name})
        endforeach()
        set(FFMPEG_LIBRARIES "${_ffmpeg}" CACHE STRING "" FORCE)
        set(FFMPEG_INCLUDE_DIRS "${VENTO_FFMPEG_PREFIX}/include" CACHE STRING "" FORCE)
        foreach(lib AVCODEC SWRESAMPLE AVFORMAT AVFILTER AVDEVICE POSTPROCESS)
            unset(${lib}_LIBRARIES CACHE)
            unset(${lib}_INCLUDE_DIRS CACHE)
        endforeach()
        unset(AVCODEC_VERSION CACHE)
    endif()
endif()

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
# (the OpenSSL port here, libvncserver's and TigerVNC's under the host root),
# on every later one it does, so which targets were built with -flto=thin —
# and the bytes of the library they make — came down to the tree's history. A
# forced cache entry answers it before their option() is reached, which then
# does nothing; and the answer is off, which is also what a target made
# before this line sees on a first configure, so every target in every tree
# is built the same way whatever the cache held — the four trees under the
# host root, which share this one cache, included.
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

if(ANDROID)
    # The session, which is the shim over FreeRDP's client and names no
    # platform, and the Rust crate at the module's root, which is the
    # backend over the session and the JNI layer every backend shares. OpenSSL's
    # headers are borrowed from the port for the TLS version constants, and as
    # SYSTEM like the rest, because linking the port would put them on the path
    # as ours.
    add_library(remotedesktop_freerdp SHARED freerdp_session.c h264_vento.c rdpsnd_vento.c)
    vento_android_library(remotedesktop_freerdp remotedesktop_freerdp)
    remotedesktop_warnings(remotedesktop_freerdp)
    target_include_directories(remotedesktop_freerdp SYSTEM PRIVATE ${FREERDP_INCLUDES}
            "$<TARGET_PROPERTY:openssl,INTERFACE_INCLUDE_DIRECTORIES>")
    target_link_libraries(remotedesktop_freerdp PRIVATE freerdp-client freerdp winpr)

    # A library has one `JNI_OnLoad`, and WinPR brings one: it keeps the
    # `JavaVM` it reads the time zone through. Theirs is compiled under
    # another name, and the crate's — the one that registers the natives —
    # calls it first (`natives!`'s `on_load`, in `src/lib.rs`).
    # A definition on that one source file rather than a patch: jni.h's
    # declaration is renamed with it, so the two still agree.
    set_property(SOURCE "${THIRD_PARTY}/freerdp/winpr/libwinpr/utils/android.c"
            TARGET_DIRECTORY winpr APPEND PROPERTY COMPILE_DEFINITIONS JNI_OnLoad=winpr_JNI_OnLoad)
else()
    # The session alone, for whatever desktop process links it: the neutral
    # include directory is its interface, and `vento_log` is left to the
    # platform — the probe's `vento_linux`, or the library's own copy of it
    # under VENTO_SHARED (vento_linux.cmake). OpenSSL's headers are the ones
    # their configure found, for the same constants.
    vento_backend_library(vento_freerdp freerdp_session.c h264_vento.c rdpsnd_vento.c)
    target_include_directories(vento_freerdp SYSTEM PRIVATE ${FREERDP_INCLUDES}
            ${OPENSSL_INCLUDE_DIR})
    target_link_libraries(vento_freerdp ${VENTO_BACKEND_LINK} freerdp-client freerdp winpr)

    vento_backend_probe(vento_freerdp freerdp_probe.c)

    # And beside the probe, the tests of the session file's pure parts
    # (freerdp_test.c), which include the three sources rather than link the
    # library, and wrap the two input calls the keys go through.
    if(VENTO_PROBES)
        add_executable(vento_freerdp_test freerdp_test.c)
        remotedesktop_warnings(vento_freerdp_test)
        target_include_directories(vento_freerdp_test PRIVATE "${_VENTO_BACKENDS}/api/include"
                "${CMAKE_CURRENT_SOURCE_DIR}/include")
        target_include_directories(vento_freerdp_test SYSTEM PRIVATE ${FREERDP_INCLUDES}
                ${OPENSSL_INCLUDE_DIR} "${THIRD_PARTY}/freerdp/libfreerdp/codec")
        target_link_libraries(vento_freerdp_test PRIVATE freerdp-client freerdp winpr vento_linux)
        target_link_options(vento_freerdp_test PRIVATE
                "LINKER:--wrap=freerdp_input_send_keyboard_event"
                "LINKER:--wrap=freerdp_input_send_unicode_keyboard_event")
        add_test(NAME vento_freerdp_test COMMAND vento_freerdp_test)
    endif()
endif()
