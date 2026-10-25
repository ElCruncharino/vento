# Provides the port in backends/platform/android/ports/openssl as the result of
# find_package(OpenSSL): FreeRDP asks for OpenSSL through its own find_feature,
# and CMake's FindOpenSSL would either fail against a sysroot without it or,
# worse, find the host's.
#
# It sets only what FreeRDP reads; the imported targets are defined because a
# find module is expected to define them, not because this build uses them.
if(NOT TARGET openssl)
    add_subdirectory("${CMAKE_CURRENT_LIST_DIR}/../../../../platform/android/ports/openssl" openssl)
endif()

set(OPENSSL_FOUND TRUE)
# Both spellings: this file is include()d rather than found, so nothing maps
# the upper-case variable onto the name FreeRDP checks.
set(OpenSSL_FOUND TRUE)
set(OPENSSL_INCLUDE_DIR "")
set(OPENSSL_SSL_LIBRARY openssl_ssl)
set(OPENSSL_CRYPTO_LIBRARIES openssl_crypto)
set(OPENSSL_LIBRARIES openssl)
# Read from the submodule rather than repeated: FreeRDP prints it, and the
# submodule's pin is meant to be the only place the version is recorded.
file(STRINGS "${CMAKE_CURRENT_LIST_DIR}/../../../../../third_party/openssl/VERSION.dat"
        _openssl_version_dat)
foreach(line ${_openssl_version_dat})
    if(line MATCHES "^(MAJOR|MINOR|PATCH)=(.*)$")
        list(APPEND _openssl_version_parts "${CMAKE_MATCH_2}")
    endif()
endforeach()
list(JOIN _openssl_version_parts "." OPENSSL_VERSION)

if(NOT TARGET OpenSSL::Crypto)
    add_library(OpenSSL::Crypto ALIAS openssl_crypto)
    add_library(OpenSSL::SSL ALIAS openssl_ssl)
endif()
