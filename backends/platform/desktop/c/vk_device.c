// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// The Vulkan device to decode on, chosen before FFmpeg opens one.
//
// FFmpeg's Vulkan device, asked for with no device string, is the first
// physical device the loader lists, whatever it is. That is lavapipe
// (Mesa's software Vulkan, `VK_PHYSICAL_DEVICE_TYPE_CPU`) on a machine whose
// GPU driver cannot open the GPU — `/dev/dri` hidden by a sandbox, say —
// which decodes no video and costs an instance and a device of its own; and
// it is a GPU without a video decode queue where the driver keeps decoding
// behind a switch, as Mesa's ANV does behind `ANV_DEBUG=video-decode`. So
// the devices are listed here first, through an instance of our own, and
// FFmpeg is handed the index of the first that is a GPU with a queue family
// that decodes video, or not asked at all.
//
// **Loaded for the look, then unloaded.** libvulkan is opened here only when
// a decoder asks for Vulkan (off loads nothing, and `auto` only once VAAPI
// could not take the stream), and creating the instance loads every driver
// the loader knows of, lavapipe's among them. Destroying it unloads them
// again and the handle is closed, so a machine without a device to decode
// on is left with nothing of Vulkan in it; FFmpeg loads libvulkan again
// itself for the device it opens, which is the same library.
//
// **The loader's order is FFmpeg's.** Both instances are the same loader's
// in the same process, which sorts its devices the same way each time, so
// the index is the same device; `vento_vk_is` checks that it was by UUID
// once FFmpeg has opened it.

#include "vk_device.h"

#include <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define VK_NO_PROTOTYPES
#include <vulkan/vulkan.h>

#include "vento_video.h"

// The codecs a decode queue's operations decode, as `vento_video_codec` bits.
static uint32_t codecs_of(VkVideoCodecOperationFlagsKHR operations) {
    uint32_t codecs = 0;
    if (operations & VK_VIDEO_CODEC_OPERATION_DECODE_H264_BIT_KHR)
        codecs |= 1u << VENTO_VIDEO_H264;
    if (operations & VK_VIDEO_CODEC_OPERATION_DECODE_H265_BIT_KHR)
        codecs |= 1u << VENTO_VIDEO_H265;
    if (operations & VK_VIDEO_CODEC_OPERATION_DECODE_VP9_BIT_KHR)
        codecs |= 1u << VENTO_VIDEO_VP9;
    if (operations & VK_VIDEO_CODEC_OPERATION_DECODE_AV1_BIT_KHR)
        codecs |= 1u << VENTO_VIDEO_AV1;
    return codecs;
}

// Whether the device has `VK_KHR_video_decode_queue`, without which its
// queue families' video properties cannot be asked for.
static int decodes(PFN_vkEnumerateDeviceExtensionProperties enumerate, VkPhysicalDevice device) {
    uint32_t count = 0;
    if (enumerate(device, NULL, &count, NULL) != VK_SUCCESS || count == 0)
        return 0;
    VkExtensionProperties *extensions = calloc(count, sizeof(*extensions));
    if (!extensions)
        return 0;
    int found = 0;
    if (enumerate(device, NULL, &count, extensions) >= VK_SUCCESS) {
        for (uint32_t i = 0; i < count && !found; i++)
            found = !strcmp(extensions[i].extensionName, VK_KHR_VIDEO_DECODE_QUEUE_EXTENSION_NAME);
    }
    free(extensions);
    return found;
}

int vento_vk_choose(struct vento_vk_choice *choice, char *why, size_t why_len) {
    memset(choice, 0, sizeof(*choice));
    void *library = dlopen("libvulkan.so.1", RTLD_NOW | RTLD_LOCAL);
    if (!library) {
        snprintf(why, why_len, "no libvulkan (%s)", dlerror());
        return 0;
    }
    PFN_vkGetInstanceProcAddr get = (PFN_vkGetInstanceProcAddr)dlsym(library,
                                                                     "vkGetInstanceProcAddr");
    PFN_vkCreateInstance create_instance =
        get ? (PFN_vkCreateInstance)get(NULL, "vkCreateInstance") : NULL;
    // 1.1, for the queue families' and the device's properties `2`.
    VkApplicationInfo app = {
        .sType = VK_STRUCTURE_TYPE_APPLICATION_INFO,
        .pApplicationName = "vento",
        .apiVersion = VK_API_VERSION_1_1,
    };
    VkInstanceCreateInfo info = {
        .sType = VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO,
        .pApplicationInfo = &app,
    };
    VkInstance instance = VK_NULL_HANDLE;
    if (!create_instance || create_instance(&info, NULL, &instance) != VK_SUCCESS) {
        snprintf(why, why_len, "no device (no Vulkan instance)");
        dlclose(library);
        return 0;
    }
#define LOOK_UP(name) PFN_##name name = (PFN_##name)get(instance, #name)
    LOOK_UP(vkDestroyInstance);
    LOOK_UP(vkEnumeratePhysicalDevices);
    LOOK_UP(vkGetPhysicalDeviceProperties2);
    LOOK_UP(vkGetPhysicalDeviceQueueFamilyProperties2);
    LOOK_UP(vkEnumerateDeviceExtensionProperties);
#undef LOOK_UP

    int found = 0;
    uint32_t count = 0;
    VkPhysicalDevice *devices = NULL;
    if (vkEnumeratePhysicalDevices && vkGetPhysicalDeviceProperties2 &&
        vkGetPhysicalDeviceQueueFamilyProperties2 && vkEnumerateDeviceExtensionProperties &&
        vkEnumeratePhysicalDevices(instance, &count, NULL) == VK_SUCCESS && count > 0 &&
        (devices = calloc(count, sizeof(*devices))) &&
        vkEnumeratePhysicalDevices(instance, &count, devices) >= VK_SUCCESS) {
        for (uint32_t i = 0; i < count && !found; i++) {
            VkPhysicalDeviceIDProperties id = {
                .sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_ID_PROPERTIES,
            };
            VkPhysicalDeviceProperties2 properties = {
                .sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_PROPERTIES_2,
                .pNext = &id,
            };
            vkGetPhysicalDeviceProperties2(devices[i], &properties);
            if (properties.properties.deviceType == VK_PHYSICAL_DEVICE_TYPE_CPU ||
                properties.properties.apiVersion < VK_API_VERSION_1_1 ||
                !decodes(vkEnumerateDeviceExtensionProperties, devices[i]))
                continue;
            uint32_t families = 0;
            vkGetPhysicalDeviceQueueFamilyProperties2(devices[i], &families, NULL);
            VkQueueFamilyProperties2 *family = calloc(families, sizeof(*family));
            VkQueueFamilyVideoPropertiesKHR *video = calloc(families, sizeof(*video));
            uint32_t codecs = 0;
            if (family && video) {
                for (uint32_t f = 0; f < families; f++) {
                    video[f].sType = VK_STRUCTURE_TYPE_QUEUE_FAMILY_VIDEO_PROPERTIES_KHR;
                    family[f].sType = VK_STRUCTURE_TYPE_QUEUE_FAMILY_PROPERTIES_2;
                    family[f].pNext = &video[f];
                }
                vkGetPhysicalDeviceQueueFamilyProperties2(devices[i], &families, family);
                for (uint32_t f = 0; f < families; f++) {
                    if (family[f].queueFamilyProperties.queueFlags & VK_QUEUE_VIDEO_DECODE_BIT_KHR)
                        codecs |= codecs_of(video[f].videoCodecOperations);
                }
            }
            free(video);
            free(family);
            if (!codecs)
                continue;
            found = 1;
            choice->index = (int)i;
            choice->codecs = codecs;
            memcpy(choice->uuid, id.deviceUUID, sizeof(choice->uuid));
            snprintf(choice->name, sizeof(choice->name), "%s", properties.properties.deviceName);
        }
    }
    free(devices);
    if (!found)
        snprintf(why, why_len, "no video decode device");
    if (vkDestroyInstance)
        vkDestroyInstance(instance, NULL);
    dlclose(library);
    return found;
}

int vento_vk_is(const struct vento_vk_choice *choice, void *get_instance_proc_addr,
                void *instance, void *device) {
    PFN_vkGetInstanceProcAddr get = (PFN_vkGetInstanceProcAddr)get_instance_proc_addr;
    PFN_vkGetPhysicalDeviceProperties2 properties2 =
        get ? (PFN_vkGetPhysicalDeviceProperties2)get((VkInstance)instance,
                                                      "vkGetPhysicalDeviceProperties2")
            : NULL;
    if (!properties2)
        return 0;
    VkPhysicalDeviceIDProperties id = {
        .sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_ID_PROPERTIES,
    };
    VkPhysicalDeviceProperties2 properties = {
        .sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_PROPERTIES_2,
        .pNext = &id,
    };
    properties2((VkPhysicalDevice)device, &properties);
    return !memcmp(id.deviceUUID, choice->uuid, sizeof(choice->uuid));
}
