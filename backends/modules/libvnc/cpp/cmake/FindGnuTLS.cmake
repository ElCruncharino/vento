# find_package(GnuTLS), resolved to the port added as a subdirectory next to
# it rather than to the host's library. LibVNCServer asks for a minimum
# version, which in module mode is checked against this variable.

set(GNUTLS_FOUND TRUE)
# Read rather than hardcoded: the version is pinned in
# backends/crypto-sources.txt, and a number here could keep satisfying a
# minimum the port no longer meets. (TigerVNC's cmake/pixman.cmake reads
# meson's version for the same reason.)
file(READ "${CMAKE_CURRENT_LIST_DIR}/../../../../platform/android/ports/gnutls/gen/arm64-v8a/config.h" _gnutls_config)
if(NOT _gnutls_config MATCHES "#define VERSION \"([0-9]+\\.[0-9]+\\.[0-9]+)\"")
    message(FATAL_ERROR "no VERSION in the generated gnutls config.h")
endif()
set(GnuTLS_VERSION "${CMAKE_MATCH_1}")
set(GNUTLS_VERSION "${GnuTLS_VERSION}")
set(GNUTLS_INCLUDE_DIR "")
set(GNUTLS_LIBRARIES gnutls)
set(GNUTLS_DEFINITIONS "")
