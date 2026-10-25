# The host build's find_package(JPEG): reuses the phone's FindJPEG.cmake from
# a separate module path, next to the host's FindGnuTLS.cmake, so that the
# phone's FindGnuTLS.cmake (which expects the GnuTLS port) is not on the path
# (see the module's CMakeLists.txt).
include("${CMAKE_CURRENT_LIST_DIR}/../FindJPEG.cmake")
