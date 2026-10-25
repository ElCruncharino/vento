# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The Android adapter for a C backend, as an object library each module links
# into its own shared library: `backends/platform/android/jni`, compiled once
# per module against `backends/common/include`. Linking the object library
# carries both include directories to the module, so its `*_session.c` finds
# `<vento/backend.h>` and its `*_jni.c` finds `"vento_jni_natives.h"`. The
# module still names `jnigraphics` and `log` itself, which is where its own
# link line says what it needs of the NDK; they are repeated here only so that
# the adapter is correct on its own.

get_filename_component(_VENTO_BACKENDS "${CMAKE_CURRENT_LIST_DIR}/.." ABSOLUTE)

add_library(vento_jni OBJECT "${_VENTO_BACKENDS}/platform/android/jni/vento_jni.c")
remotedesktop_warnings(vento_jni)
target_include_directories(vento_jni PUBLIC
        "${_VENTO_BACKENDS}/common/include"
        "${_VENTO_BACKENDS}/platform/android/jni")
target_link_libraries(vento_jni PUBLIC jnigraphics log)
