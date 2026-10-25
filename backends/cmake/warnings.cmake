# SPDX-License-Identifier: GPL-3.0-or-later
#
# The warning policy for the native builds, in one place because every C
# backend module's CMake project and the host build share it.
#
# **Vendored code is silenced and ours is not.** A warning in a vendored tree
# is not a finding: fixing it would be a change not sent upstream and a patch
# this repository would carry indefinitely. A warning in our own sources is a
# finding, because they are JNI and C API glue with hand-managed lifetimes and
# there is no upstream to report it to.
#
# Two rules follow, and the second is easy to get wrong:
#
# 1. Silence at the *target*, never globally, and never on a target of ours.
#    `-w` on a directory reaches ours too; the vendored trees are separate
#    targets, and `remotedesktop_warnings` never touches them.
# 2. Vendored headers are included by our code but not fixed here. `-Wall` on
#    our target diagnoses everything it compiles, including several thousand
#    lines of vendored templates, so a vendored include directory goes on the
#    path as SYSTEM, which attributes the warning to the file rather than the
#    target.
#
# `-Werror` is deliberately *not* here: a build that fails on a warning cannot
# be made by a third party on another toolchain, and that includes the
# reproducible-build check, which exists so that a third party can rebuild it.
# It is enabled by `scripts/check-warnings.sh` (through Gradle's
# `strictWarnings` property, which sets STRICT_WARNINGS) and by nothing in the
# path of building the app.

option(STRICT_WARNINGS "Fail the build on a warning in our own sources" OFF)

# Every native target of ours, and no others.
function(remotedesktop_warnings target)
    target_compile_options(${target} PRIVATE -Wall -Wextra)
    if(STRICT_WARNINGS)
        target_compile_options(${target} PRIVATE -Werror)
    endif()
endfunction()

# Every compiled target a vendored tree defines under `dir`, subdirectories
# included: rule 1 for a project too big to name its targets one by one.
# `-w` on each of them rather than on the directory, so that it cannot reach a
# target of ours however the scopes are nested; and a compile option rather than
# CMAKE_C_FLAGS, which FreeRDP writes into a string in the library.
function(remotedesktop_vendored_warnings dir)
    get_property(targets DIRECTORY "${dir}" PROPERTY BUILDSYSTEM_TARGETS)
    foreach(target IN LISTS targets)
        get_target_property(type ${target} TYPE)
        if(type MATCHES "^(STATIC_LIBRARY|SHARED_LIBRARY|MODULE_LIBRARY|OBJECT_LIBRARY|EXECUTABLE)$")
            target_compile_options(${target} PRIVATE -w)
        endif()
    endforeach()
    get_property(subdirs DIRECTORY "${dir}" PROPERTY SUBDIRECTORIES)
    foreach(sub IN LISTS subdirs)
        remotedesktop_vendored_warnings("${sub}")
    endforeach()
endfunction()
