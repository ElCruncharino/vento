# The host build's find_package(JPEG): the shim beside the phone's, on a
# module path of its own beside the host's FindGnuTLS.cmake, so that the
# phone's, which answers the phone's GnuTLS port, is not on it (see the
# module's CMakeLists).
include("${CMAKE_CURRENT_LIST_DIR}/../FindJPEG.cmake")
