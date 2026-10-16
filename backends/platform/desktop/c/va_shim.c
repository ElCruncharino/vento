// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// libva, for FFmpeg's VAAPI code, loaded the first time it is called rather
// than linked.
//
// FFmpeg's VAAPI device and its hardware decoders call the libva functions
// defined below (and its Vulkan device one of them, deriving itself from a
// VAAPI one), and nothing else of libva or libva-drm; `vento_video.c` calls
// two of the same, to wait for a held picture's surface and export it. Linked against the
// real libraries, every vento would need libva installed just to start, and
// would load it into every process, though hardware decoding is off unless it
// is asked for and most sessions never touch it. So each function is defined
// here instead, forwarding to the real one, and the real ones are looked up
// the first time any of them is called: `libva.so.2` and `libva-drm.so.2`, the
// sonames every libva 2 has had. With hardware decoding off nothing calls
// them, and neither library is ever opened (`/proc/PID/maps` shows no libva).
//
// **Without libva everything fails the way a machine without a device does.**
// FFmpeg's first call is `vaGetDisplayDRM`, which then returns NULL, so
// `av_hwdevice_ctx_create` fails and the decoder is software; every other
// function here returns an error, though nothing gets as far as calling one.
// A libva older than 2.21 has no `vaMapBuffer2`, which FFmpeg built against
// these headers calls, and that is `vaMapBuffer` for it, which is what FFmpeg
// itself calls against older headers.
//
// **Hidden.** These are definitions of libva's own names, and visible they
// would be in the executable's dynamic symbol table, where the real libva-drm
// (and a driver that links libva) would bind to them instead of to libva: a
// loop. Hidden, they are only what FFmpeg's objects in the same link reach,
// and the real libva's are what everything it loads reaches.
//
// **A library of its own.** It is built before FFmpeg is configured, as
// `libvento_va.a` in FFmpeg's prefix, and `build.rs` writes a `libva.pc` and a
// `libva-drm.pc` there that name it, which is what FFmpeg's configure checks
// for libva with: a test program linked against what pkg-config says. That is
// the least to keep working across FFmpeg releases, since it is the check
// configure makes of a real libva, rather than a patch to configure or a
// cache of its answers. Named `vento_va` and not `va`, so that no linker's
// search can find the system's `libva.so` in its place, which would make
// libva a dependency of the binary after all.

#pragma GCC visibility push(hidden)
#include <va/va.h>
#include <va/va_drm.h>
#pragma GCC visibility pop

#include <dlfcn.h>
#include <pthread.h>
#include <stdio.h>

#include "va_shim.h"

// Each function: its return type, name, parameters, the arguments as they are
// passed on, which library it is in (0 libva, 1 libva-drm) and what it returns
// without that library.
#define VA_FAILED VA_STATUS_ERROR_OPERATION_FAILED
#define VA_FUNCTIONS(X)                                                                            \
    X(VAStatus, vaAcquireBufferHandle, (VADisplay dpy, VABufferID buf, VABufferInfo * info),       \
      (dpy, buf, info), 0, VA_FAILED)                                                              \
    X(VAStatus, vaBeginPicture, (VADisplay dpy, VAContextID context, VASurfaceID target),          \
      (dpy, context, target), 0, VA_FAILED)                                                        \
    X(VAStatus, vaCreateBuffer,                                                                    \
      (VADisplay dpy, VAContextID context, VABufferType type, unsigned int size,                   \
       unsigned int num_elements, void *data, VABufferID *buf),                                    \
      (dpy, context, type, size, num_elements, data, buf), 0, VA_FAILED)                          \
    X(VAStatus, vaCreateConfig,                                                                    \
      (VADisplay dpy, VAProfile profile, VAEntrypoint entrypoint, VAConfigAttrib * attribs,        \
       int num_attribs, VAConfigID *config),                                                       \
      (dpy, profile, entrypoint, attribs, num_attribs, config), 0, VA_FAILED)                      \
    X(VAStatus, vaCreateContext,                                                                   \
      (VADisplay dpy, VAConfigID config, int width, int height, int flag, VASurfaceID *targets,    \
       int num_targets, VAContextID *context),                                                     \
      (dpy, config, width, height, flag, targets, num_targets, context), 0, VA_FAILED)             \
    X(VAStatus, vaCreateImage,                                                                     \
      (VADisplay dpy, VAImageFormat * format, int width, int height, VAImage *image),              \
      (dpy, format, width, height, image), 0, VA_FAILED)                                           \
    X(VAStatus, vaCreateSurfaces,                                                                  \
      (VADisplay dpy, unsigned int format, unsigned int width, unsigned int height,                \
       VASurfaceID *surfaces, unsigned int num_surfaces, VASurfaceAttrib *attribs,                 \
       unsigned int num_attribs),                                                                  \
      (dpy, format, width, height, surfaces, num_surfaces, attribs, num_attribs), 0, VA_FAILED)    \
    X(VAStatus, vaDeriveImage, (VADisplay dpy, VASurfaceID surface, VAImage * image),              \
      (dpy, surface, image), 0, VA_FAILED)                                                         \
    X(VAStatus, vaDestroyBuffer, (VADisplay dpy, VABufferID buf), (dpy, buf), 0, VA_FAILED)        \
    X(VAStatus, vaDestroyConfig, (VADisplay dpy, VAConfigID config), (dpy, config), 0, VA_FAILED)  \
    X(VAStatus, vaDestroyContext, (VADisplay dpy, VAContextID context), (dpy, context), 0,         \
      VA_FAILED)                                                                                   \
    X(VAStatus, vaDestroyImage, (VADisplay dpy, VAImageID image), (dpy, image), 0, VA_FAILED)      \
    X(VAStatus, vaDestroySurfaces, (VADisplay dpy, VASurfaceID * surfaces, int num_surfaces),      \
      (dpy, surfaces, num_surfaces), 0, VA_FAILED)                                                 \
    X(VAStatus, vaEndPicture, (VADisplay dpy, VAContextID context), (dpy, context), 0, VA_FAILED)  \
    X(const char *, vaErrorStr, (VAStatus status), (status), 0, "libva is not loaded")             \
    X(VAStatus, vaExportSurfaceHandle,                                                             \
      (VADisplay dpy, VASurfaceID surface, uint32_t mem_type, uint32_t flags, void *descriptor),   \
      (dpy, surface, mem_type, flags, descriptor), 0, VA_FAILED)                                   \
    X(VAStatus, vaGetDisplayAttributes,                                                            \
      (VADisplay dpy, VADisplayAttribute * attribs, int num_attribs), (dpy, attribs, num_attribs), \
      0, VA_FAILED)                                                                                \
    X(VAStatus, vaGetImage,                                                                        \
      (VADisplay dpy, VASurfaceID surface, int x, int y, unsigned int width, unsigned int height,  \
       VAImageID image),                                                                           \
      (dpy, surface, x, y, width, height, image), 0, VA_FAILED)                                    \
    X(VAStatus, vaInitialize, (VADisplay dpy, int *major, int *minor), (dpy, major, minor), 0,     \
      VA_FAILED)                                                                                   \
    X(VAStatus, vaMapBuffer, (VADisplay dpy, VABufferID buf, void **pbuf), (dpy, buf, pbuf), 0,    \
      VA_FAILED)                                                                                   \
    X(int, vaMaxNumImageFormats, (VADisplay dpy), (dpy), 0, 0)                                     \
    X(int, vaMaxNumProfiles, (VADisplay dpy), (dpy), 0, 0)                                         \
    X(VAStatus, vaPutImage,                                                                        \
      (VADisplay dpy, VASurfaceID surface, VAImageID image, int src_x, int src_y,                  \
       unsigned int src_width, unsigned int src_height, int dest_x, int dest_y,                    \
       unsigned int dest_width, unsigned int dest_height),                                         \
      (dpy, surface, image, src_x, src_y, src_width, src_height, dest_x, dest_y, dest_width,       \
       dest_height),                                                                               \
      0, VA_FAILED)                                                                                \
    X(VAStatus, vaQueryConfigProfiles, (VADisplay dpy, VAProfile * profiles, int *num_profiles),   \
      (dpy, profiles, num_profiles), 0, VA_FAILED)                                                 \
    X(VAStatus, vaQueryImageFormats, (VADisplay dpy, VAImageFormat * formats, int *num_formats),   \
      (dpy, formats, num_formats), 0, VA_FAILED)                                                   \
    X(VAStatus, vaQuerySurfaceAttributes,                                                          \
      (VADisplay dpy, VAConfigID config, VASurfaceAttrib * attribs, unsigned int *num_attribs),    \
      (dpy, config, attribs, num_attribs), 0, VA_FAILED)                                           \
    X(const char *, vaQueryVendorString, (VADisplay dpy), (dpy), 0, NULL)                          \
    X(VAStatus, vaReleaseBufferHandle, (VADisplay dpy, VABufferID buf), (dpy, buf), 0, VA_FAILED)  \
    X(VAStatus, vaRenderPicture,                                                                   \
      (VADisplay dpy, VAContextID context, VABufferID * buffers, int num_buffers),                 \
      (dpy, context, buffers, num_buffers), 0, VA_FAILED)                                          \
    X(VAStatus, vaSetDriverName, (VADisplay dpy, char *name), (dpy, name), 0, VA_FAILED)           \
    X(VAMessageCallback, vaSetErrorCallback,                                                       \
      (VADisplay dpy, VAMessageCallback callback, void *user_context),                             \
      (dpy, callback, user_context), 0, NULL)                                                      \
    X(VAMessageCallback, vaSetInfoCallback,                                                        \
      (VADisplay dpy, VAMessageCallback callback, void *user_context),                             \
      (dpy, callback, user_context), 0, NULL)                                                      \
    X(VAStatus, vaSyncSurface, (VADisplay dpy, VASurfaceID surface), (dpy, surface), 0,            \
      VA_FAILED)                                                                                   \
    X(VAStatus, vaTerminate, (VADisplay dpy), (dpy), 0, VA_FAILED)                                 \
    X(VAStatus, vaUnmapBuffer, (VADisplay dpy, VABufferID buf), (dpy, buf), 0, VA_FAILED)          \
    X(VADisplay, vaGetDisplayDRM, (int fd), (fd), 1, NULL)

// The real functions, or NULL for one that is not there.
static struct {
#define FIELD(ret, name, params, args, lib, fail) ret(*name) params;
    VA_FUNCTIONS(FIELD)
#undef FIELD
    // libva 2.21's, looked up beside the rest and standing in for nothing:
    // without it, `vaMapBuffer2` is `vaMapBuffer`.
    VAStatus (*vaMapBuffer2)(VADisplay dpy, VABufferID buf, void **pbuf, uint32_t flags);
} real;

static pthread_once_t once = PTHREAD_ONCE_INIT;
static char why[256];
static int loaded;

static void load(void) {
    // Local: the symbols are only ever reached through `real`, and global
    // they would be there for whatever the process dlopens next to bind to.
    // libva-drm needs libva, and its own dependency on it brings in the same
    // copy rather than a second.
    void *libs[2] = {dlopen("libva.so.2", RTLD_NOW | RTLD_LOCAL), NULL};
    if (!libs[0]) {
        snprintf(why, sizeof(why), "%s", dlerror());
        return;
    }
    libs[1] = dlopen("libva-drm.so.2", RTLD_NOW | RTLD_LOCAL);
    if (!libs[1]) {
        snprintf(why, sizeof(why), "%s", dlerror());
        return;
    }
#define RESOLVE(ret, name, params, args, lib, fail)                                                \
    *(void **)(&real.name) = dlsym(libs[lib], #name);
    VA_FUNCTIONS(RESOLVE)
#undef RESOLVE
    *(void **)(&real.vaMapBuffer2) = dlsym(libs[0], "vaMapBuffer2");
    if (!real.vaGetDisplayDRM || !real.vaInitialize) {
        snprintf(why, sizeof(why), "libva has no vaGetDisplayDRM or vaInitialize");
        return;
    }
    loaded = 1;
}

int vento_va_loaded(const char **reason) {
    pthread_once(&once, load);
    if (reason)
        *reason = loaded ? NULL : why;
    return loaded;
}

#pragma GCC visibility push(hidden)

#define FORWARD(ret, name, params, args, lib, fail)                                                \
    ret name params {                                                                              \
        pthread_once(&once, load);                                                                 \
        return real.name ? real.name args : (fail);                                                \
    }
VA_FUNCTIONS(FORWARD)
#undef FORWARD

VAStatus vaMapBuffer2(VADisplay dpy, VABufferID buf, void **pbuf, uint32_t flags) {
    pthread_once(&once, load);
    if (real.vaMapBuffer2)
        return real.vaMapBuffer2(dpy, buf, pbuf, flags);
    return real.vaMapBuffer ? real.vaMapBuffer(dpy, buf, pbuf) : VA_FAILED;
}

#pragma GCC visibility pop
