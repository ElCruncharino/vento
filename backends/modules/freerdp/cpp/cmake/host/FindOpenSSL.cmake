# The host build's find_package(OpenSSL): the OpenSSL the host root builds
# from the submodule (platform/linux/cmake/openssl.cmake), rather than the
# host's. It sets what FreeRDP reads, as the phone's shim one directory up
# does for the port, with the include directory named as well: the archives
# and headers are only there once the root's build of them has run, which
# FreeRDP's targets wait for through the ones named here.

set(OPENSSL_FOUND TRUE)
set(OpenSSL_FOUND TRUE)
set(OPENSSL_VERSION "${VENTO_OPENSSL_VERSION}")
get_target_property(OPENSSL_INCLUDE_DIR openssl INTERFACE_INCLUDE_DIRECTORIES)
set(OPENSSL_SSL_LIBRARY openssl_ssl)
set(OPENSSL_CRYPTO_LIBRARY openssl_crypto)
set(OPENSSL_CRYPTO_LIBRARIES openssl_crypto)
set(OPENSSL_LIBRARIES openssl)

if(NOT TARGET OpenSSL::Crypto)
    add_library(OpenSSL::Crypto ALIAS openssl_crypto)
    add_library(OpenSSL::SSL ALIAS openssl_ssl)
endif()
