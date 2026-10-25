# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Under Gradle this is a root, configured by the NDK toolchain, and makes
# `libremotedesktop_realvnc.so`: the one native thing this module has, the
# estimator read in `realvnc_traffic.c`, which links against nothing — the
# viewer core is already in the process, and what it reads through is a pointer
# their own Java handed us rather than a symbol of theirs.

cmake_minimum_required(VERSION 3.30)
project(remotedesktop_realvnc C CXX)

get_filename_component(THIRD_PARTY "${CMAKE_CURRENT_SOURCE_DIR}/../../../../third_party" ABSOLUTE)
get_filename_component(VENTO_BACKENDS "${CMAKE_CURRENT_SOURCE_DIR}/../../.." ABSOLUTE)
get_filename_component(REPO "${CMAKE_CURRENT_SOURCE_DIR}/../../../.." ABSOLUTE)

include("${VENTO_BACKENDS}/cmake/reproducible.cmake")
include("${VENTO_BACKENDS}/cmake/warnings.cmake")

add_library(remotedesktop_realvnc SHARED realvnc_traffic.c)
remotedesktop_warnings(remotedesktop_realvnc)
