// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// What vento_video.c asks of va_shim.c beyond libva's own functions.
#ifndef VENTO_VA_SHIM_H
#define VENTO_VA_SHIM_H

// Whether libva and libva-drm could be loaded, loading them if nobody has
// yet; when not, `*reason` (if given) is the loader's words for why. Hidden,
// as the rest of the shim is.
__attribute__((visibility("hidden"))) int vento_va_loaded(const char **reason);

#endif
