// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// Loads the two bionic libraries into the process: the symbol tables standing
// in for the Android system libraries they import, and the two `dlopen`s
// through mcpelauncher-linker.

#ifndef VENTO_REALVNC_LOADER_H
#define VENTO_REALVNC_LOADER_H

#include <string>

namespace vento_realvnc {

/* Once per process: `linker::init`, the five system libraries by name, then
   `libc++_shared.so` and `libvncviewer.so` out of `dir`. Returns the viewer
   core's handle, or nullptr with `*why` set. */
void *loadCore(const std::string &dir, std::string *why);

/* A symbol of the viewer core's. */
void *coreSymbol(void *core, const char *name);

} // namespace vento_realvnc

#endif
