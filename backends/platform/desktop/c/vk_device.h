// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// What vento_video.c asks of vk_device.c: which Vulkan device to decode on,
// chosen before FFmpeg is asked to open one.
#ifndef VENTO_VK_DEVICE_H
#define VENTO_VK_DEVICE_H

#include <stddef.h>
#include <stdint.h>

// A physical device that can decode video, as the Vulkan loader lists it.
struct vento_vk_choice {
    // Its place in the loader's list, which is what FFmpeg's device string
    // takes a number for.
    int index;
    // The `vento_video_codec`s its decode queue has operations for, a bit
    // each (`1 << codec`).
    uint32_t codecs;
    // `VkPhysicalDeviceIDProperties::deviceUUID`, to tell that the device
    // FFmpeg opened is this one.
    uint8_t uuid[16];
    char name[256];
};

// The first device the loader lists that is not a CPU implementation (such
// as lavapipe) and has a queue family that decodes video, into `choice`; or
// false, with why in `why` ("no libvulkan (...)", "no video decode device").
// Loads libvulkan and every driver for the time it takes, and unloads them
// again, so that what is left in the process afterwards is only what FFmpeg
// opens on the device chosen. Hidden.
__attribute__((visibility("hidden"))) int vento_vk_choose(struct vento_vk_choice *choice,
                                                          char *why, size_t why_len);

// Whether the physical device of FFmpeg's instance (`instance` and `device`,
// with its `vkGetInstanceProcAddr`) is the one chosen. Hidden.
__attribute__((visibility("hidden"))) int vento_vk_is(const struct vento_vk_choice *choice,
                                                      void *get_instance_proc_addr,
                                                      void *instance, void *device);

#endif
