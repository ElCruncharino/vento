# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# OpenSSL for the host build, from the submodule in `third_party/openssl`
# that the phone's port (`platform/android/ports/openssl`) builds: the same
# version, as two static position-independent archives that the FreeRDP
# library links in (`vento_linux.cmake` keeps their symbols out of its
# exports). The system's would tie a built tree to its distribution's
# soname: libssl.so.1.1 on Ubuntu 18.04 and libssl.so.3 on current
# distributions, neither of which the other has.
#
# The phone's port compiles the sources from CMake with Configure's output
# committed in its gen/, since an app build must not run Configure. A host
# build may, so this runs upstream's own build through ExternalProject: perl
# Configure, make and install into the build tree, once, at build time, with
# nothing committed and the submodule's tree only read (the build is out of
# tree). It is redone when its command line changes and when the version
# does, since the version names the directory it is built in. FreeRDP's
# configure runs no checks against OpenSSL (it reads the variables the
# module's find_package shim gives it), so the headers and archives need
# not exist until its sources are compiled, which waits for them.
#
# The configuration is the port's (its gen.sh says why each switch), with
# the assembly on as there, and three differences:
# - The legacy provider is built (no `no-legacy`). FreeRDP compiles its own
#   RC4, MD4 and MD5 and uses none of it, but WinPR loads it by name when
#   it initialises OpenSSL and, failing that, prints a warning that MD4 is
#   unavailable on stderr in every process. `no-dso` also turns off
#   loadable modules, so the provider is compiled into libcrypto as a
#   built-in one: loading it reads no file from MODULESDIR, which is a
#   path in this build tree. It adds about 70 kB to the FreeRDP library.
# - OPENSSLDIR is /etc/ssl, where Debian and Ubuntu keep the CA certificates
#   as a hashed directory (`certs/`) and Fedora links `cert.pem` to its
#   bundle, the two default locations OpenSSL looks in under it (`cert.pem`
#   and `certs/`). It decides nothing here:
#   FreeRDP is told the application manages certificates, so every one goes
#   to the client's known hosts and none to a CA store.
# - no-autoload-config: the library does not read `openssl.cnf` from there
#   (or from OPENSSL_CONF) on its own. That file is the distribution's,
#   written for its own OpenSSL build: it names providers and engines this
#   one does not have, and on Fedora includes the crypto-policies file,
#   whose contents follow Fedora's OpenSSL version and patches. This build
#   behaves the same everywhere instead.
#
# The result is three imported targets named as the phone's port names its
# own, `openssl_crypto`, `openssl_ssl` and `openssl` (both, with the include
# directory), and VENTO_OPENSSL_VERSION.

block(SCOPE_FOR VARIABLES PROPAGATE VENTO_OPENSSL_VERSION)
    include(ExternalProject)
    find_package(Threads REQUIRED)

    get_filename_component(source "${CMAKE_CURRENT_LIST_DIR}/../../../../third_party/openssl" ABSOLUTE)
    if(NOT EXISTS "${source}/Configure")
        message(FATAL_ERROR "third_party/openssl is empty — run `git submodule update --init`")
    endif()
    # Configure refuses to build out of tree from a source tree that was
    # configured in place, and would build that configuration instead.
    if(EXISTS "${source}/configdata.pm")
        message(FATAL_ERROR "third_party/openssl has been configured in place "
                "(configdata.pm); `git -C third_party/openssl clean -fdx` restores it")
    endif()
    file(STRINGS "${source}/VERSION.dat" lines REGEX "^(MAJOR|MINOR|PATCH)=")
    set(parts "")
    foreach(line ${lines})
        string(REGEX REPLACE "^[A-Z]+=" "" part "${line}")
        list(APPEND parts "${part}")
    endforeach()
    list(JOIN parts "." VENTO_OPENSSL_VERSION)

    # Configure is perl with a few modules a minimal perl lacks; said here,
    # rather than as a missing module halfway through the build.
    find_program(VENTO_PERL perl REQUIRED)
    execute_process(COMMAND "${VENTO_PERL}" -e "use FindBin; use IPC::Cmd; use File::Compare; use Time::Piece;"
            RESULT_VARIABLE missing OUTPUT_QUIET ERROR_QUIET)
    if(missing)
        message(FATAL_ERROR "OpenSSL's Configure needs perl with its core modules "
                "(Fedora: perl-core; Debian and Ubuntu: perl)")
    endif()
    find_program(VENTO_MAKE NAMES gmake make REQUIRED)
    cmake_host_system_information(RESULT jobs QUERY NUMBER_OF_LOGICAL_CORES)

    set(root "${CMAKE_BINARY_DIR}/tls/openssl-${VENTO_OPENSSL_VERSION}")
    set(prefix "${root}/prefix")
    # Configure takes the compiler from CC and its own optimisation flags
    # unless CFLAGS is set, which the environment of whatever runs the build
    # (cargo's, a shell's) may have done for some other compiler.
    set(env ${CMAKE_COMMAND} -E env --unset=CFLAGS --unset=CPPFLAGS --unset=LDFLAGS --unset=LIBS
            "CC=${CMAKE_C_COMPILER}")
    ExternalProject_Add(vento_openssl_build
            PREFIX "${root}"
            SOURCE_DIR "${source}"
            BINARY_DIR "${root}/build"
            CONFIGURE_COMMAND ${env} "${VENTO_PERL}" "${source}/Configure"
                --prefix=${prefix} --libdir=lib --openssldir=/etc/ssl
                no-shared no-tests no-apps no-docs no-engine no-dso
                no-comp no-dtls no-srp no-psk no-ssl3 no-weak-ssl-ciphers
                no-autoload-config -fPIC
            BUILD_COMMAND ${VENTO_MAKE} -j${jobs} build_libs
            INSTALL_COMMAND ${VENTO_MAKE} install_dev
            INSTALL_BYPRODUCTS "${prefix}/lib/libssl.a" "${prefix}/lib/libcrypto.a"
            LOG_CONFIGURE ON LOG_BUILD ON LOG_INSTALL ON LOG_OUTPUT_ON_FAILURE ON)

    # An imported target's include directory has to exist when the build is
    # generated, which is before anything is installed into it.
    file(MAKE_DIRECTORY "${prefix}/include")
    foreach(lib crypto ssl)
        add_library(openssl_${lib} STATIC IMPORTED GLOBAL)
        set_target_properties(openssl_${lib} PROPERTIES
                IMPORTED_LOCATION "${prefix}/lib/lib${lib}.a"
                INTERFACE_INCLUDE_DIRECTORIES "${prefix}/include")
        add_dependencies(openssl_${lib} vento_openssl_build)
    endforeach()
    target_link_libraries(openssl_crypto INTERFACE Threads::Threads)
    target_link_libraries(openssl_ssl INTERFACE openssl_crypto)
    add_library(openssl INTERFACE IMPORTED GLOBAL)
    set_target_properties(openssl PROPERTIES INTERFACE_INCLUDE_DIRECTORIES "${prefix}/include")
    target_link_libraries(openssl INTERFACE openssl_ssl openssl_crypto)
endblock()
