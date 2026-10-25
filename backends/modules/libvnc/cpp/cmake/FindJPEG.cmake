# find_package(JPEG), resolved to the libjpeg-turbo added as a subdirectory
# next to it rather than to the host's library. LibVNCServer expects
# JPEG_INCLUDE_DIR to be a single directory (it greps jpeglib.h for JCS_EXT_RGB
# to tell libjpeg-turbo from libjpeg), so this names the source tree; the
# generated jconfig.h is already on the include path.

set(JPEG_FOUND TRUE)
set(JPEG_INCLUDE_DIR "${THIRD_PARTY}/libjpeg-turbo/src")
set(JPEG_INCLUDE_DIRS "${JPEG_INCLUDE_DIR}")
set(JPEG_LIBRARY jpeg-static)
set(JPEG_LIBRARIES jpeg-static)
