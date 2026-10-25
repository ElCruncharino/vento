# syntax=docker/dockerfile:1
# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The Linux client's release build. It runs the whole build in a container
# and outputs only the installed tree: the `vento` binary and, beside it,
# `lib/` with the backends' `libvento_*.so` (the binary's rpath is
# `$ORIGIN/lib`). The tree can be moved anywhere as a whole: /opt/vento, a
# home directory, a USB stick. From the repository root, with the submodules
# initialised (`git submodule update --init --recursive`):
#
#   docker build -f linux/Dockerfile --output type=local,dest=build/dist/vento .
#
# The build copies the checkout and patches the copy: it applies `patches/`
# to submodules at their pinned commits and accepts submodules `./patch.sh`
# has already patched. It writes nothing to the checkout.
# `linux/Dockerfile.dockerignore` limits the context to the sources the build
# reads. GnuTLS and nettle are not in it: the CMake build downloads their
# release tarballs and checks them against the SHA-256 hashes in
# `backends/crypto-sources.txt`, as on a machine without them, so the result
# does not depend on whatever is unpacked in the checkout.
#
# The binary needs the glibc symbol versions it was linked against, so it runs
# on the build's glibc and later ones. The build therefore runs on Ubuntu
# 18.04 (glibc 2.27), with a newer clang, CMake, Meson and NASM fetched at
# pinned versions and checked. This baseline lives here and not in the build
# scripts, which build against the machine's own libraries. The C++ runtime is
# GCC 8's libstdc++, the one 18.04 ships.
#
# At run time the tree needs:
# - Linux on x86_64 with glibc 2.27 or later, libstdc++ (GLIBCXX_3.4.22) and
#   zlib (libz.so.1);
# - for the window, a Vulkan or GL driver and a Wayland or X11 session;
# - for RealVNC, the two libraries from RealVNC's viewer (linux/claude-notes.md
#   says how to extract them from its APK).
# Sound (PipeWire, ALSA) and hardware decoding (libva) are loaded at run time
# when installed. libjpeg-turbo, pixman, GnuTLS, nettle, OpenSSL, libatomic,
# FFmpeg and dav1d are linked in.

FROM ubuntu:18.04@sha256:152dc042452c496007f07ca9127571cb9c29697f42acbfad72324b2bb2e43c98 AS build
SHELL ["/bin/bash", "-eo", "pipefail", "-c"]
ENV DEBIAN_FRONTEND=noninteractive

# From Ubuntu: GCC 8 for its libstdc++ headers (clang uses the newest GCC
# installed), make, m4 for nettle's assembly, perl for OpenSSL's Configure
# (the base image has only perl-base, without the modules it needs),
# pkg-config, Python 3.8 for Meson, Ninja 1.8.2, git for `git apply`, and
# zlib's development files.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates curl gnupg xz-utils binutils make m4 perl g++-8 pkg-config \
        ninja-build python3.8 git \
        zlib1g-dev \
    && rm -rf /var/lib/apt/lists/*

# clang 18 and LLVM's binutils from apt.llvm.org, the key checked by its
# fingerprint. RealVNC's hosting layer builds only with clang, so clang is
# the C and C++ compiler of the whole build and Rust's linker (there is no
# `cc`; GCC 8 provides headers and libraries only). bindgen needs libclang.
RUN curl -fsSL https://apt.llvm.org/llvm-snapshot.gpg.key -o /tmp/llvm.asc \
    && gpg --batch --with-colons --with-fingerprint /tmp/llvm.asc 2> /dev/null \
        | grep -q '^fpr:::::::::6084F3CF814B57C1CF12EFD515CF4D18AF4F7421:' \
    && gpg --batch --dearmor < /tmp/llvm.asc > /usr/share/keyrings/llvm.gpg \
    && rm /tmp/llvm.asc \
    && echo 'deb [signed-by=/usr/share/keyrings/llvm.gpg] http://apt.llvm.org/bionic/ llvm-toolchain-bionic-18 main' \
        > /etc/apt/sources.list.d/llvm.list \
    && apt-get update \
    && apt-get install -y --no-install-recommends clang-18 llvm-18 libclang-18-dev \
    && rm -rf /var/lib/apt/lists/*
ENV PATH=/usr/lib/llvm-18/bin:$PATH \
    CC=clang CXX=clang++ \
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=clang \
    LIBCLANG_PATH=/usr/lib/llvm-18/lib

# Newer build tools than 18.04 ships, each checked against its SHA-256:
# - CMake 3.31 (the tree needs 3.30; 18.04 has 3.10), Kitware's binary,
#   built for glibc 2.17;
# - NASM 2.16 (dav1d needs 2.14; 18.04 has 2.13), built from source;
# - Meson 1.5 (dav1d needs 0.49; 18.04 has 0.45), run from its release
#   tarball with Python 3.8.
RUN curl -fsSL -o /tmp/cmake.tar.gz https://github.com/Kitware/CMake/releases/download/v3.31.6/cmake-3.31.6-linux-x86_64.tar.gz \
    && echo '5a1133ff103c71eb5120e2cc3de922733e7d8a26a98ae716397e8676adb367bf  /tmp/cmake.tar.gz' | sha256sum -c - \
    && mkdir /opt/cmake && tar -xzf /tmp/cmake.tar.gz -C /opt/cmake --strip-components=1 \
    && ln -s /opt/cmake/bin/cmake /opt/cmake/bin/ctest /usr/local/bin/ \
    && curl -fsSL -o /tmp/nasm.tar.xz https://www.nasm.us/pub/nasm/releasebuilds/2.16.03/nasm-2.16.03.tar.xz \
    && echo '1412a1c760bbd05db026b6c0d1657affd6631cd0a63cddb6f73cc6d4aa616148  /tmp/nasm.tar.xz' | sha256sum -c - \
    && mkdir /tmp/nasm && tar -xJf /tmp/nasm.tar.xz -C /tmp/nasm --strip-components=1 \
    && (cd /tmp/nasm && ./configure --prefix=/usr/local > /dev/null && make -j"$(nproc)" > /dev/null && make install > /dev/null) \
    && curl -fsSL -o /tmp/meson.tar.gz https://github.com/mesonbuild/meson/releases/download/1.5.2/meson-1.5.2.tar.gz \
    && echo 'f955e09ab0d71ef180ae85df65991d58ed8430323de7d77a37e11c9ea630910b  /tmp/meson.tar.gz' | sha256sum -c - \
    && mkdir /opt/meson && tar -xzf /tmp/meson.tar.gz -C /opt/meson --strip-components=1 \
    && printf '#!/bin/sh\nexec python3.8 /opt/meson/meson.py "$@"\n' > /usr/local/bin/meson \
    && chmod +x /usr/local/bin/meson \
    && rm -rf /tmp/cmake.tar.gz /tmp/nasm.tar.xz /tmp/nasm /tmp/meson.tar.gz \
    && cmake --version | head -1 && nasm -v && meson --version && clang --version | head -1

# Rust through rustup, at the channel in rust-toolchain.toml, minimal
# profile. The file also lists Android targets and lint components for the
# phone's build; RUSTUP_TOOLCHAIN at the build step overrides the file so
# rustup does not fetch them.
COPY rust-toolchain.toml /tmp/rust-toolchain.toml
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH
RUN curl -fsSL -o /tmp/rustup-init https://static.rust-lang.org/rustup/archive/1.28.2/x86_64-unknown-linux-gnu/rustup-init \
    && echo '20a06e644b0d9bd2fbdbfd52d42540bdde820ea7df86e92e533c073da0cdd43c  /tmp/rustup-init' | sha256sum -c - \
    && chmod +x /tmp/rustup-init \
    && channel=$(sed -n 's/^channel *= *"\(.*\)"/\1/p' /tmp/rust-toolchain.toml) \
    && /tmp/rustup-init -y --no-modify-path --profile minimal --default-toolchain "$channel" \
    && rm /tmp/rustup-init \
    && rustc --version

COPY . /src
WORKDIR /src

# Apply each submodule's patch series in order if its first patch applies
# (the submodule is at its pinned commit). If its last patch reverses
# cleanly, `./patch.sh` already applied the series in the checkout. Anything
# else is an error.
RUN for dir in patches/*/; do \
        name=$(basename "$dir"); tree=third_party/$name; \
        patches=("$dir"*.patch); \
        [ -e "${patches[0]}" ] || continue; \
        [ -d "$tree" ] || { echo "error: $tree is not in the build context" >&2; exit 1; }; \
        if (cd "$tree" && git apply --check "../../${patches[0]}" 2> /dev/null); then \
            for patch in "${patches[@]}"; do (cd "$tree" && git apply "../../$patch"); done; \
            echo "$name: ${#patches[@]} patches applied"; \
        elif (cd "$tree" && git apply -R --check "../../${patches[-1]}" 2> /dev/null); then \
            echo "$name: patched already"; \
        else \
            echo "error: $tree is neither at its pinned commit nor patched" >&2; exit 1; \
        fi; \
    done

# Build, then copy the tree out of target/release/ (linux/build.rs lays it
# out there) and strip the libraries' debug information (the C backends are
# built RelWithDebInfo). Check the copy: the relative rpath, no library
# missing with LD_LIBRARY_PATH unset, and every backend available in
# `vento --list`, which opens each backend's library from the tree's own lib/
# (the binary does not link them). Print the newest glibc and libstdc++
# symbol versions each file needs.
# The tools are LLVM's, since 18.04's binutils cannot read clang 18's DWARF 5.
RUN cd linux \
    && RUSTUP_TOOLCHAIN=$(sed -n 's/^channel *= *"\(.*\)"/\1/p' /src/rust-toolchain.toml) \
        cargo build --release --locked \
    && mkdir -p /out/lib \
    && cp target/release/vento /out/ \
    && cp target/release/lib/libvento_*.so /out/lib/ \
    && llvm-strip --strip-debug /out/lib/libvento_*.so \
    && llvm-readelf -d /out/vento | grep -q 'RUNPATH.*\[\$ORIGIN/lib' \
    && for file in /out/vento /out/lib/*.so; do \
           ! ldd "$file" | grep 'not found' || { echo "error: $file" >&2; exit 1; }; \
       done \
    && /out/vento --list > /tmp/backends \
    && grep -E '^[a-z]+  [A-Z]' /tmp/backends \
    && ! grep 'not available' /tmp/backends \
    && for file in /out/vento /out/lib/*.so; do \
           echo "$file: $(llvm-objdump -T "$file" | grep -oE 'GLIBC_[0-9.]+' | sort -uV | tail -1)" \
                "$(llvm-objdump -T "$file" | grep -oE 'GLIBCXX_[0-9.]+' | sort -uV | tail -1)"; \
       done \
    && ls -l /out /out/lib

FROM scratch
COPY --from=build /out/ /
