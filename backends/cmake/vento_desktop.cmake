# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The desktop platform for a C backend, as `vento_jni.cmake` is the Android
# one: `backends/platform/desktop/c`, compiled once as two object libraries.
# `vento_desktop` is the platform proper — `vento_log` onto stderr, which is
# all a session file needs of one to link — and carries the neutral include
# directory, so a target that links it finds `<vento/backend.h>`.
# `vento_probe` is the probe body, which each module's host branch puts a
# `main` of its own in front of. Object libraries do not pass their objects on
# through one another, so a probe links both by name.
#
# Guarded, because the host root includes it once and each module includes it
# again for the case where a module is configured on its own; the TigerVNC
# audio sink is not here but under `platform/desktop/tigervnc`, compiled by
# that module, since it speaks TigerVNC's headers.

get_filename_component(_VENTO_BACKENDS "${CMAKE_CURRENT_LIST_DIR}/.." ABSOLUTE)

if(NOT TARGET vento_desktop)
    add_library(vento_desktop OBJECT "${_VENTO_BACKENDS}/platform/desktop/c/vento_log.c")
    remotedesktop_warnings(vento_desktop)
    target_include_directories(vento_desktop PUBLIC
            "${_VENTO_BACKENDS}/common/include"
            "${_VENTO_BACKENDS}/platform/desktop/c")

    add_library(vento_probe OBJECT "${_VENTO_BACKENDS}/platform/desktop/c/probe.c")
    remotedesktop_warnings(vento_probe)
    target_link_libraries(vento_probe PUBLIC vento_desktop)
endif()
