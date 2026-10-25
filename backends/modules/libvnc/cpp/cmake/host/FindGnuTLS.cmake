# The host build's find_package(GnuTLS): the GnuTLS the host root builds from
# the phone's tarball (platform/linux/cmake/gnutls.cmake), rather than the
# host's. LibVNCServer asks for a minimum version, which in module mode is
# checked against this variable; the root reads it from the tarball.

set(GNUTLS_FOUND TRUE)
set(GnuTLS_VERSION "${VENTO_GNUTLS_VERSION}")
set(GNUTLS_VERSION "${GnuTLS_VERSION}")
set(GNUTLS_INCLUDE_DIR "")
set(GNUTLS_LIBRARIES gnutls)
set(GNUTLS_DEFINITIONS "")
