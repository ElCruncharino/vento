#!/bin/bash
# Does every `native` method we declare exist in the library we ship?
#
# `realvnc/check-jni-abi.sh` asks this of somebody else's binary; this
# asks it of ours, and the failure mode is the same one: a native that is not
# where Java looks for it does not fail to compile. It fails the first time
# that method is called, as an UnsatisfiedLinkError — which for something like
# a thumbnail read might be the first time somebody looks at the home screen
# after a session.
#
# A library binds its natives one of two ways, and each is asked its own
# question:
#
#   exported    by mangled name, `Java_<class>_<method>`: the C backends. Every
#               native the class declares must be a dynamic symbol.
#   registered  by `RegisterNatives` from the library's `JNI_OnLoad`: the Rust
#               backends, which are one JNI layer (backends/platform/android)
#               registered on each module's generated natives class. There is
#               no symbol to look for, so what is checked is what registration
#               is made of — the library exports `JNI_OnLoad`, names the class
#               it registers on, and carries each native's name and JNI
#               descriptor, as the compiled class declares them, as strings;
#               and every `on…` callback `NativeBackend` declares has its name
#               and descriptor there too, since those are looked up by string.
#               A descriptor that drifted on either side is a string the other
#               does not have. (That each Rust function matches its descriptor
#               is checked when it compiles, by `jni::native_method!`.)
#
# One library per protocol module, which is what the loop below is: each is
# asked about its own declarations, and a module deleted is a line deleted.
#
#   scripts/check-jni-symbols.sh            # builds if needed
#   NOBUILD=1 scripts/check-jni-symbols.sh  # against whatever is already there
set -euo pipefail
cd "$(dirname "$0")/.."   # the build is at the repository root

# module : the class declaring the natives : the library it loads : how it is
# built : how it binds
MODULES=(
    "rfb:net/pgaskin/remotedesktop/backend/rfb/RfbNatives:remotedesktop_rfb:cargo:registered"
    "libvnc:net/pgaskin/remotedesktop/backend/libvnc/LibVncNative:remotedesktop_libvnc:cmake:exported"
    "tigervnc:net/pgaskin/remotedesktop/backend/tigervnc/TigerVncNative:remotedesktop_tigervnc:cmake:exported"
    "ironrdp:net/pgaskin/remotedesktop/backend/ironrdp/IronRdpNative:remotedesktop_ironrdp:cargo:exported"
    "freerdp:net/pgaskin/remotedesktop/backend/freerdp/FreeRdpNative:remotedesktop_freerdp:cmake:exported"
    "realvnc:net/pgaskin/remotedesktop/backend/realvnc/RealVncTraffic:remotedesktop_realvnc:cmake:exported"
    "rustdesk:net/pgaskin/remotedesktop/backend/rustdesk/RustDeskNative:remotedesktop_rustdesk:cargo:exported"
    "spice:net/pgaskin/remotedesktop/backend/spice/SpiceNatives:remotedesktop_spice:cargo:registered"
)

# The class every registered library calls back, compiled once in :backends.
EVENTS=net/pgaskin/remotedesktop/backend/NativeBackend

if [ "${NOBUILD:-0}" != 1 ]; then
    tasks=(":backends:compileDebugJavaWithJavac")
    for entry in "${MODULES[@]}"; do
        IFS=: read -r module _ _ built bound <<<"$entry"
        case "$built" in
            cargo) tasks+=(":backends:${module}:cargoBuildDebugNative") ;;
            cmake) tasks+=(":backends:${module}:assembleDebug") ;;
        esac
        # A generated class is only on disk once it has been compiled.
        [ "$bound" = registered ] && tasks+=(":backends:${module}:compileDebugJavaWithJavac")
    done
    ./gradlew --quiet "${tasks[@]}"
fi

# A compiled class, newest first for the reason the library is (below).
class_file() {
    find "$1" -path '*/javac/*' -path "*/classes/$2.class" -printf '%T@ %p\n' 2>/dev/null \
        | sort -rn | head -1 | cut -d' ' -f2-
}

# `name descriptor` for each method of a class file that matches `$2` (a
# grep -E on javap's declaration line): javap prints the descriptor on the
# line after each declaration, which is the same string RegisterNatives and
# GetMethodID are called with, so the two sides compare as text.
methods() {
    javap -p -s "$1" | awk -v want="$2" '
        /^ +descriptor: / { if (name != "") print name, $2; name = ""; next }
        /\(/ && $0 ~ want {
            line = $0; sub(/\(.*/, "", line); n = split(line, w, " "); name = w[n]; next
        }
        { name = "" }
    '
}

fail=0
for entry in "${MODULES[@]}"; do
    IFS=: read -r module class lib built bound <<<"$entry"

    case "$built" in
        cargo) root="backends/modules/$module/build/generated/rustNativeLibs" ;;
        cmake) root="backends/modules/$module/build/intermediates/cxx" ;;
    esac
    # The newest, not the first: a CMake module's build tree keeps a directory
    # per configuration, so `find | head -1` can hand back a library from an
    # older one — which is a check that answers about a binary nobody is
    # shipping, in both directions. Seen: a MISSING for a method that had been
    # there for a stage.
    so="$(find "$root" -name "lib${lib}.so" -printf '%T@ %p\n' 2>/dev/null \
            | sort -rn | head -1 | cut -d' ' -f2-)"
    if [ -z "$so" ]; then
        echo "no lib${lib}.so — run ./gradlew :backends:${module}:assembleDebug" >&2
        exit 1
    fi
    echo "== ${module}: ${so#*/build/} (${bound})"

    # The host's readelf, as check-page-alignment.sh uses: these are ordinary
    # ELF dynamic symbols and no cross toolchain is needed to read them.
    symbols="$(readelf -W --dyn-syms "$so" | awk '$4=="FUNC"{print $NF}')"

    if [ "$bound" = exported ]; then
        java="backends/modules/$module/src/main/java/$class.java"
        prefix="Java_$(dirname "$class" | tr / _)_$(basename "$class")"
        while read -r method; do
            [ -n "$method" ] || continue
            if grep -qx "${prefix}_${method}" <<<"$symbols"; then
                printf 'ok       %s.%s\n' "$(basename "$class")" "$method"
            else
                printf 'MISSING  %s.%s (%s)\n' "$(basename "$class")" "$method" "${prefix}_${method}"
                fail=1
            fi
        done < <(grep -E 'static +native' "$java" | grep -oE '\bnative[A-Z][A-Za-z0-9]*')
        continue
    fi

    # ---- registered -----------------------------------------------------------
    natives_class="$(class_file "backends/modules/$module/build" "$class")"
    events_class="$(class_file backends/build "$EVENTS")"
    if [ -z "$natives_class" ] || [ -z "$events_class" ]; then
        echo "no compiled $(basename "$class") or NativeBackend — run without NOBUILD=1" >&2
        exit 1
    fi
    strings="$(strings -a -n 1 "$so" | sort -u)"
    has() { grep -Fxq -- "$1" <<<"$strings"; }

    if grep -qx JNI_OnLoad <<<"$symbols"; then
        printf 'ok       JNI_OnLoad\n'
    else
        printf 'MISSING  JNI_OnLoad: nothing would register the natives\n'
        fail=1
    fi
    if has "$class"; then
        printf 'ok       registers on %s\n' "$class"
    else
        printf 'MISSING  registers on another class than %s\n' "$class"
        fail=1
    fi
    count=0
    while read -r name desc; do
        count=$((count + 1))
        if has "$name" && has "$desc"; then
            printf 'ok       native %s %s\n' "$name" "$desc"
        else
            printf 'MISSING  native %s %s is not what %s registers\n' "$name" "$desc" "$(basename "$so")"
            fail=1
        fi
    done < <(methods "$natives_class" ' native ')
    # A table that came out empty is a check that passes by asking nothing.
    if [ "$count" = 0 ]; then
        printf 'MISSING  %s declares no natives\n' "$(basename "$class")"
        fail=1
    fi
    callbacks=0
    while read -r name desc; do
        callbacks=$((callbacks + 1))
        if has "$name" && has "$desc"; then
            printf 'ok       callback NativeBackend.%s %s\n' "$name" "$desc"
        else
            printf 'MISSING  callback NativeBackend.%s %s is not looked up by %s\n' \
                "$name" "$desc" "$(basename "$so")"
            fail=1
        fi
    done < <(methods "$events_class" ' on[A-Z][A-Za-z]*[(]')
    if [ "$callbacks" = 0 ]; then
        printf 'MISSING  NativeBackend declares no callbacks\n'
        fail=1
    fi
done

if [ "$fail" = 0 ]; then
    echo "PASS: every declared native is bound in its own library"
else
    echo "FAIL: see above" >&2
    exit 1
fi
