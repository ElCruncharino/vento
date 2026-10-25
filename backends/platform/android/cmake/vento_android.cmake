# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The Android half of a C backend, linked into the module's own shared
# library: the module's Rust crate (the backend as an `api::Backend` over the
# vtable in `vento/backend.h`, plus the JNI layer in
# `backends/platform/android`, whose `JNI_OnLoad` registers the natives on the
# module's generated natives class), and `cpp/vento_log.c`, which sends
# `vento_log` to logcat. The crate is a static library that Gradle has
# cargo-ndk build before CMake runs (`rustStaticLibrary` in
# `backends/build.gradle`), one per ABI under VENTO_RUST_LIBRARY_DIR.
#
#   vento_android_library(<target> <crate library name>)
#
# Nothing in the C calls into the Rust, so the linker would leave the archive
# out: `JNI_OnLoad` is named as undefined, which pulls in what it reaches, and
# it is the only symbol the library exports (`exports.map` beside
# `vento_log.c` says why). The NDK libraries after it are the ones in the
# crate's `native-static-libs`.

get_filename_component(_VENTO_ANDROID "${CMAKE_CURRENT_LIST_DIR}/.." ABSOLUTE)
get_filename_component(_VENTO_BACKENDS "${_VENTO_ANDROID}/../.." ABSOLUTE)

if(NOT VENTO_RUST_LIBRARY_DIR)
    message(FATAL_ERROR "VENTO_RUST_LIBRARY_DIR is not set: the module's build.gradle passes it")
endif()

function(vento_android_library target crate)
    add_library(${target}_rust STATIC IMPORTED)
    set_target_properties(${target}_rust PROPERTIES
            IMPORTED_LOCATION "${VENTO_RUST_LIBRARY_DIR}/${ANDROID_ABI}/lib${crate}.a")
    target_sources(${target} PRIVATE "${_VENTO_ANDROID}/cpp/vento_log.c")
    # `api/include`, and the module's own `include/` with its
    # `vento/<name>.h`. Called from the module's CMakeLists,
    # CMAKE_CURRENT_SOURCE_DIR is that module's directory.
    target_include_directories(${target} PRIVATE "${_VENTO_BACKENDS}/api/include"
            "${CMAKE_CURRENT_SOURCE_DIR}/include")
    target_link_options(${target} PRIVATE "LINKER:--undefined=JNI_OnLoad"
            "LINKER:--version-script=${_VENTO_ANDROID}/cpp/exports.map")
    set_property(TARGET ${target} APPEND PROPERTY LINK_DEPENDS
            "${_VENTO_ANDROID}/cpp/exports.map")
    target_link_libraries(${target} PRIVATE ${target}_rust jnigraphics mediandk aaudio log dl m)
endfunction()
