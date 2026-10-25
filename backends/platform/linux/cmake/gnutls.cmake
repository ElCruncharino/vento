# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# GnuTLS and nettle for the host build, from the release tarballs in
# `third_party/gnutls` and `third_party/nettle` that the phone's ports
# (`platform/android/ports/{gnutls,nettle}`) build: the same versions, nettle
# with mini-gmp as there, as static position-independent archives that the
# libvnc and TigerVNC libraries link in (`vento_linux.cmake` keeps their
# symbols out of the exports). The system's would tie a built tree to its
# distribution's sonames: nettle's changes with nearly every release
# (libnettle.so.6 on Ubuntu 18.04, .so.8 on current ones, .so.9 from 4.0),
# and an older distribution has no GnuTLS that a newer one's tree can run
# against.
#
# The phone's ports compile the sources from CMake with configure's answers
# committed in their gen/, since an app build must not run configure. A
# host build may, so this runs upstream's own build, autotools and all,
# through ExternalProject: configure, make and install into the build tree,
# once, at build time, with nothing committed. It is redone when its command
# line changes (a compiler, a flag) and when a tarball's version does, since
# the version names the directory everything is built in. Unlike the phone's,
# the assembly is on: nettle's (m4 input, so where `m4` is installed; without
# it, nettle is built in C with a note saying so) and GnuTLS's x86-64 AES,
# GHASH and SHA, which is assembler source in the tarball.
#
# Neither reads anything of the build machine's at run time but two paths:
# the CA bundle GnuTLS loads as the system's trust store, which the
# backends hand every certificate past anyway (both libraries are patched to
# ask the client about one that verified too, so the known hosts decide),
# at the path Debian, Ubuntu and Fedora all have it at; and the system
# priority file, at the path Debian's GnuTLS reads, rather than the
# crypto-policies file Fedora's reads, whose contents follow Fedora's own
# GnuTLS version.
#
# The tarballs are downloaded and unpacked at configure time when a tree is
# missing or is not the pinned release (`vento_crypto_source` below), so
# that ExternalProject finds the sources and a host build needs no Gradle.
#
# The result is three imported targets named as the phone's ports name
# theirs, `gnutls`, `hogweed` and `nettle`, each carrying its include
# directory and what it links after it, and VENTO_GNUTLS_VERSION for
# libvncserver's version check.

# The hash in `<tree>/.pin`, or nothing.
function(vento_crypto_source_pin tree out)
    set(pin "")
    if(EXISTS "${tree}/.pin")
        file(READ "${tree}/.pin" pin)
        string(STRIP "${pin}" pin)
    endif()
    set(${out} "${pin}" PARENT_SCOPE)
endfunction()

# Makes `third_party/<name>` the release `crypto-sources.txt` pins for it,
# as Gradle's `fetchCryptoSources` does, so that a host build needs no
# Gradle: downloaded from the first mirror that answers, checked against
# the pinned SHA-256 before anything is unpacked, unpacked without the
# tarball's top directory, with a `.pin` file holding the hash. A tree
# whose `.pin` holds the pinned hash is kept as it is; any other is
# replaced. The tree is unpacked under `third_party/.fetch/` and renamed
# into place with its `.pin`, so an interrupted download or unpack leaves
# the previous tree or none, never part of one. A lock there serialises
# the configures of several build trees.
function(vento_crypto_source third_party name)
    unset(version)
    unset(sha256)
    set(pins "${CMAKE_CURRENT_FUNCTION_LIST_DIR}/../../../crypto-sources.txt")
    set_property(DIRECTORY "${CMAKE_SOURCE_DIR}" APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS "${pins}")
    file(STRINGS "${pins}" lines REGEX "^[ \t]*${name}\\.")
    set(urls "")
    foreach(line IN LISTS lines)
        if(NOT line MATCHES "^[ \t]*${name}\\.(version|sha256|url)[ \t]*=[ \t]*([^ \t]+)[ \t]*$")
            message(FATAL_ERROR "backends/crypto-sources.txt: not a `<name>.<key> = <value>` line: ${line}")
        endif()
        if(CMAKE_MATCH_1 STREQUAL "url")
            list(APPEND urls "${CMAKE_MATCH_2}")
        else()
            set(${CMAKE_MATCH_1} "${CMAKE_MATCH_2}")
        endif()
    endforeach()
    if(NOT DEFINED version OR NOT DEFINED sha256 OR NOT urls)
        message(FATAL_ERROR "backends/crypto-sources.txt has no version, sha256 and url for ${name}")
    endif()

    set(tree "${third_party}/${name}")
    # A rebuild notices a deleted or replaced tree and configures again.
    set_property(DIRECTORY "${CMAKE_SOURCE_DIR}" APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS "${tree}/.pin")
    vento_crypto_source_pin("${tree}" pin)
    if(pin STREQUAL sha256)
        return()
    endif()
    file(MAKE_DIRECTORY "${third_party}/.fetch")
    file(LOCK "${third_party}/.fetch/${name}.lock" GUARD FUNCTION)
    vento_crypto_source_pin("${tree}" pin)
    if(pin STREQUAL sha256)
        return()
    endif()
    if(NOT EXISTS "${tree}")
        set(why "is missing")
    elseif(pin STREQUAL "")
        set(why "has no .pin file, so it is not known to be ${name} ${version}")
    else()
        set(why "holds another release (sha256 ${pin})")
    endif()
    message(STATUS "${name}: third_party/${name} ${why}; fetching ${name} ${version}")

    set(work "${third_party}/.fetch/${name}")
    file(REMOVE_RECURSE "${work}")
    file(MAKE_DIRECTORY "${work}")
    set(failures "")
    foreach(url IN LISTS urls)
        string(REGEX REPLACE "^.*/" "" tarball "${url}")
        set(tarball "${work}/${tarball}")
        message(STATUS "${name}: downloading ${url}")
        file(DOWNLOAD "${url}" "${tarball}" STATUS status INACTIVITY_TIMEOUT 60)
        list(GET status 0 code)
        if(NOT code EQUAL 0)
            list(GET status 1 error)
            list(APPEND failures "${url}: ${error}")
            file(REMOVE "${tarball}")
            continue()
        endif()
        file(SHA256 "${tarball}" got)
        if(NOT got STREQUAL sha256)
            list(APPEND failures "${url}: sha256 ${got}, wanted ${sha256}")
            file(REMOVE "${tarball}")
            continue()
        endif()
        file(ARCHIVE_EXTRACT INPUT "${tarball}" DESTINATION "${work}/unpacked")
        file(GLOB top LIST_DIRECTORIES true "${work}/unpacked/*")
        list(LENGTH top count)
        if(NOT count EQUAL 1 OR NOT IS_DIRECTORY "${top}")
            file(REMOVE_RECURSE "${work}")
            message(FATAL_ERROR "${url} does not hold a single top directory, which "
                    "third_party/${name} is unpacked from; check backends/crypto-sources.txt")
        endif()
        file(WRITE "${top}/.pin" "${sha256}\n")
        if(EXISTS "${tree}")
            file(RENAME "${tree}" "${work}/previous")
        endif()
        file(RENAME "${top}" "${tree}")
        file(REMOVE_RECURSE "${work}")
        message(STATUS "${name}: ${version} unpacked into third_party/${name}")
        return()
    endforeach()
    file(REMOVE_RECURSE "${work}")
    list(JOIN failures "\n  " failures)
    list(GET urls 0 tarball)
    string(REGEX REPLACE "^.*/" "" tarball "${tarball}")
    message(FATAL_ERROR "third_party/${name} ${why}, and no mirror gave ${name} ${version}:\n"
            "  ${failures}\n"
            "Configure again once a mirror serves it, or run `./gradlew :backends:fetchCryptoSources`, "
            "or unpack the tarball by hand: download it from one of those URLs, check that its "
            "SHA-256 is ${sha256}, then, from the repository root, `mkdir third_party/${name} && "
            "tar -xf ${tarball} --strip-components=1 -C third_party/${name} && "
            "echo ${sha256} > third_party/${name}/.pin`. The pins are in backends/crypto-sources.txt.")
endfunction()

block(SCOPE_FOR VARIABLES PROPAGATE VENTO_GNUTLS_VERSION)
    include(ExternalProject)
    find_package(Threads REQUIRED)

    get_filename_component(third_party "${CMAKE_CURRENT_LIST_DIR}/../../../../third_party" ABSOLUTE)
    foreach(name nettle gnutls)
        vento_crypto_source("${third_party}" ${name})
        file(STRINGS "${third_party}/${name}/configure" version REGEX "^PACKAGE_VERSION='[0-9.]+'$")
        if(NOT version MATCHES "'([0-9.]+)'")
            message(FATAL_ERROR "no PACKAGE_VERSION in third_party/${name}/configure")
        endif()
        set(${name}_version "${CMAKE_MATCH_1}")
    endforeach()
    set(VENTO_GNUTLS_VERSION "${gnutls_version}")

    find_program(VENTO_MAKE NAMES gmake make REQUIRED)
    find_program(VENTO_M4 m4)
    cmake_host_system_information(RESULT jobs QUERY NUMBER_OF_LOGICAL_CORES)

    set(root "${CMAKE_BINARY_DIR}/tls/nettle-${nettle_version}-gnutls-${gnutls_version}")
    set(prefix "${root}/prefix")
    string(TOUPPER "${CMAKE_BUILD_TYPE}" type)
    set(cflags "${CMAKE_C_FLAGS} ${CMAKE_C_FLAGS_${type}} -fPIC")
    # The environment of whatever runs the build (cargo's, a shell's) is not
    # the one either configure is meant to see.
    set(env ${CMAKE_COMMAND} -E env
            "CC=${CMAKE_C_COMPILER}" "CFLAGS=${cflags}" "LDFLAGS=" "LIBS="
            "PKG_CONFIG_LIBDIR=${prefix}/lib/pkgconfig" "PKG_CONFIG_PATH=")

    # mini-gmp, as on the phone: GMP would be a third autotools upstream, and
    # nettle's own bignums are a file in its tarball.
    set(nettle_options --enable-mini-gmp)
    if(NOT VENTO_M4)
        message(STATUS "nettle: no m4, so built without its assembly (install m4 for it)")
        list(APPEND nettle_options --disable-assembler --disable-fat)
    endif()
    ExternalProject_Add(vento_nettle_build
            PREFIX "${root}"
            SOURCE_DIR "${third_party}/nettle"
            BINARY_DIR "${root}/nettle"
            CONFIGURE_COMMAND ${env} "CC_FOR_BUILD=${CMAKE_C_COMPILER}" "CPPFLAGS="
                "${third_party}/nettle/configure" --prefix=${prefix} --libdir=${prefix}/lib
                --disable-shared --enable-static --disable-documentation --disable-openssl
                ${nettle_options}
            BUILD_COMMAND ${VENTO_MAKE} -j${jobs}
            INSTALL_COMMAND ${VENTO_MAKE} install
            INSTALL_BYPRODUCTS "${prefix}/lib/libnettle.a" "${prefix}/lib/libhogweed.a"
            LOG_CONFIGURE ON LOG_BUILD ON LOG_INSTALL ON LOG_OUTPUT_ON_FAILURE ON)

    # The phone port's configuration (its gen.sh says why each), but for the
    # trust store, the priority file and the hardware acceleration above.
    ExternalProject_Add(vento_gnutls_build
            DEPENDS vento_nettle_build
            PREFIX "${root}"
            SOURCE_DIR "${third_party}/gnutls"
            BINARY_DIR "${root}/gnutls"
            CONFIGURE_COMMAND ${env} "CPPFLAGS=-I${prefix}/include"
                "NETTLE_CFLAGS=-I${prefix}/include" "NETTLE_LIBS=-L${prefix}/lib -lnettle"
                "HOGWEED_CFLAGS=-I${prefix}/include" "HOGWEED_LIBS=-L${prefix}/lib -lhogweed -lnettle"
                "${third_party}/gnutls/configure" --prefix=${prefix} --libdir=${prefix}/lib
                --disable-shared --enable-static
                --with-nettle-mini --with-included-libtasn1 --with-included-unistring
                --without-p11-kit --without-idn --without-tpm --without-tpm2
                --without-zlib --without-brotli --without-zstd
                --with-default-trust-store-file=/etc/ssl/certs/ca-certificates.crt
                --with-system-priority-file=/etc/gnutls/config
                --disable-doc --disable-tools --disable-tests --disable-cxx --disable-nls
                --disable-libdane --disable-full-test-suite
            BUILD_COMMAND ${VENTO_MAKE} -j${jobs}
            INSTALL_COMMAND ${VENTO_MAKE} install
            INSTALL_BYPRODUCTS "${prefix}/lib/libgnutls.a"
            LOG_CONFIGURE ON LOG_BUILD ON LOG_INSTALL ON LOG_OUTPUT_ON_FAILURE ON)

    # An imported target's include directory has to exist when the build is
    # generated, which is before anything is installed into it.
    file(MAKE_DIRECTORY "${prefix}/include")
    foreach(lib nettle hogweed gnutls)
        add_library(${lib} STATIC IMPORTED GLOBAL)
        set_target_properties(${lib} PROPERTIES
                IMPORTED_LOCATION "${prefix}/lib/lib${lib}.a"
                INTERFACE_INCLUDE_DIRECTORIES "${prefix}/include")
    endforeach()
    add_dependencies(nettle vento_nettle_build)
    add_dependencies(hogweed vento_nettle_build)
    add_dependencies(gnutls vento_gnutls_build)
    target_link_libraries(hogweed INTERFACE nettle)
    target_link_libraries(gnutls INTERFACE hogweed nettle Threads::Threads)
endblock()
