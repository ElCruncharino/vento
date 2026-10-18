// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// `java.io` and `androidx.security.crypto`, which is how the core persists
// what it keeps: its RSA key, the identities it has accepted, its password
// store and its caches, each an `EncryptedFile` under the data directory it
// was given.
//
// **The files are plain here.** `EncryptedFile` on a phone is AES-GCM under
// a key in the hardware keystore; this stand-in opens the same path as an
// ordinary file, so `rsaKeys/vncviewer.privatekey.enc` and
// `passwords.json.enc` under `state_dir` are plaintext, protected by the
// directory's mode and nothing else. A desktop client that wants better
// wraps these two streams in whatever its platform keeps secrets with; what
// the core reads and writes through them does not change.

#include <stdint.h>
#include <sys/stat.h>
#include <unistd.h>

#include <algorithm>
#include <fstream>
#include <iterator>
#include <memory>
#include <string>
#include <vector>

#include "Host.h"

namespace vento_realvnc {

namespace {

struct File final : jnivm::Object {
    std::string path;
};
struct KeySpec final : jnivm::Object {};
struct MasterKeys final : jnivm::Object {
    static std::shared_ptr<KeySpec> AES256_GCM_SPEC;
};
std::shared_ptr<KeySpec> MasterKeys::AES256_GCM_SPEC = std::make_shared<KeySpec>();
struct Scheme final : jnivm::Object {
    static std::shared_ptr<Scheme> AES256_GCM_HKDF_4KB;
};
std::shared_ptr<Scheme> Scheme::AES256_GCM_HKDF_4KB = std::make_shared<Scheme>();
struct EncryptedFile final : jnivm::Object {
    std::string path;
};
struct EncryptedFileBuilder final : jnivm::Object {
    std::string path;
};
/* A whole file read at open, and a whole file written at close: the core
   reads and writes each of its files in one go. */
struct InputStream final : jnivm::Object {
    std::vector<uint8_t> data;
    size_t at = 0;
};
struct OutputStream final : jnivm::Object {
    std::string path;
    std::vector<uint8_t> buffer;
};
struct ByteArrayOutputStream final : jnivm::Object {
    std::vector<uint8_t> buffer;
};

using Bytes = std::shared_ptr<jnivm::Array<jbyte>>;

std::vector<uint8_t> *bufferOf(jnivm::Object *o) {
    if (auto *s = dynamic_cast<OutputStream *>(o)) {
        return &s->buffer;
    }
    if (auto *s = dynamic_cast<ByteArrayOutputStream *>(o)) {
        return &s->buffer;
    }
    return nullptr;
}

template <int N>
void inputHooks(jnivm::ENV *E, const std::shared_ptr<jnivm::Class> &c) {
    c->HookInstanceFunction(E, "read", [](jnivm::ENV *, jnivm::Object *o, Bytes buf, jint off, jint len) -> jint {
        auto *s = dynamic_cast<InputStream *>(o);
        if (!s || !buf || s->at >= s->data.size() || off < 0 || len < 0
                || (size_t) off + (size_t) len > (size_t) buf->getSize()) {
            return -1;
        }
        const jint n = (jint) std::min<size_t>((size_t) len, s->data.size() - s->at);
        memcpy((uint8_t *) buf->getArray() + off, s->data.data() + s->at, (size_t) n);
        s->at += (size_t) n;
        return n;
    });
    c->HookInstanceFunction(E, "read", [](jnivm::ENV *, jnivm::Object *o) -> jint {
        auto *s = dynamic_cast<InputStream *>(o);
        if (!s || s->at >= s->data.size()) {
            return -1;
        }
        return s->data[s->at++];
    });
    c->HookInstanceFunction(E, "close", [](jnivm::ENV *, jnivm::Object *) {});
}

template <int N>
void outputHooks(jnivm::ENV *E, const std::shared_ptr<jnivm::Class> &c) {
    c->HookInstanceFunction(E, "write", [](jnivm::ENV *, jnivm::Object *o, Bytes buf, jint off, jint len) {
        auto *b = bufferOf(o);
        if (!b || !buf || off < 0 || len < 0 || (size_t) off + (size_t) len > (size_t) buf->getSize()) {
            return;
        }
        const auto *p = (const uint8_t *) buf->getArray();
        b->insert(b->end(), p + off, p + off + len);
    });
    c->HookInstanceFunction(E, "write", [](jnivm::ENV *, jnivm::Object *o, Bytes buf) {
        auto *b = bufferOf(o);
        if (!b || !buf) {
            return;
        }
        const auto *p = (const uint8_t *) buf->getArray();
        b->insert(b->end(), p, p + buf->getSize());
    });
    c->HookInstanceFunction(E, "write", [](jnivm::ENV *, jnivm::Object *o, jint v) {
        if (auto *b = bufferOf(o)) {
            b->push_back((uint8_t) v);
        }
    });
    c->HookInstanceFunction(E, "flush", [](jnivm::ENV *, jnivm::Object *) {});
    c->HookInstanceFunction(E, "close", [](jnivm::ENV *, jnivm::Object *o) {
        if (auto *s = dynamic_cast<OutputStream *>(o)) {
            std::ofstream out(s->path, std::ios::binary | std::ios::trunc);
            out.write((const char *) s->buffer.data(), (std::streamsize) s->buffer.size());
            if (!out) {
                LOGW("%s: could not be written", s->path.c_str());
            }
            chmod(s->path.c_str(), 0600);
        }
    });
}

} // namespace

void registerFiles(jnivm::VM &vm) {
    auto env = vm.GetEnv();
    auto *E = env.get();
    env->GetClass<File>("java/io/File");
    env->GetClass<KeySpec>("android/security/keystore/KeyGenParameterSpec");
    env->GetClass<MasterKeys>("androidx/security/crypto/MasterKeys");
    env->GetClass<Scheme>("androidx/security/crypto/EncryptedFile$FileEncryptionScheme");
    env->GetClass<EncryptedFile>("androidx/security/crypto/EncryptedFile");
    env->GetClass<EncryptedFileBuilder>("androidx/security/crypto/EncryptedFile$Builder");
    env->GetClass<InputStream>("java/io/FileInputStream");
    env->GetClass<OutputStream>("java/io/FileOutputStream");
    env->GetClass<ByteArrayOutputStream>("java/io/ByteArrayOutputStream");

    {
        auto file = env->GetClass("java/io/File");
        file->Hook(E, "<init>",
                   [](jnivm::ENV *, jnivm::Class *, std::shared_ptr<jnivm::String> dir,
                      std::shared_ptr<jnivm::String> name) {
                       auto f = std::make_shared<File>();
                       f->path = (dir ? std::string(*dir) : ".") + "/" + fromJstr(name);
                       return f;
                   });
        file->HookInstanceFunction(E, "exists", [](jnivm::ENV *, File *f) {
            struct stat st;
            return (jboolean) (stat(f->path.c_str(), &st) == 0);
        });
        file->HookInstanceFunction(E, "delete",
                                   [](jnivm::ENV *, File *f) { return (jboolean) (unlink(f->path.c_str()) == 0); });
        file->HookInstanceFunction(E, "getAbsolutePath", [](jnivm::ENV *, File *f) { return jstr(f->path); });
    }

    {
        auto mk = env->GetClass("androidx/security/crypto/MasterKeys");
        mk->Hook(E, "AES256_GCM_SPEC", &MasterKeys::AES256_GCM_SPEC);
        mk->Hook(E, "getOrCreate", [](jnivm::ENV *, jnivm::Class *, std::shared_ptr<KeySpec>) {
            return jstr("_androidx_security_master_key_");
        });
        env->GetClass("androidx/security/crypto/EncryptedFile$FileEncryptionScheme")
                ->Hook(E, "AES256_GCM_HKDF_4KB", &Scheme::AES256_GCM_HKDF_4KB);
        auto b = env->GetClass("androidx/security/crypto/EncryptedFile$Builder");
        b->Hook(E, "<init>",
                [](jnivm::ENV *, jnivm::Class *, std::shared_ptr<File> f, std::shared_ptr<Context>,
                   std::shared_ptr<jnivm::String>, std::shared_ptr<Scheme>) {
                    auto x = std::make_shared<EncryptedFileBuilder>();
                    x->path = f ? f->path : "";
                    return x;
                });
        b->HookInstanceFunction(E, "build", [](jnivm::ENV *, EncryptedFileBuilder *self) {
            auto x = std::make_shared<EncryptedFile>();
            x->path = self->path;
            return x;
        });
        auto ef = env->GetClass("androidx/security/crypto/EncryptedFile");
        ef->HookInstanceFunction(E, "openFileInput", [](jnivm::ENV *, EncryptedFile *self) {
            auto s = std::make_shared<InputStream>();
            std::ifstream in(self->path, std::ios::binary);
            s->data.assign(std::istreambuf_iterator<char>(in), {});
            return s;
        });
        ef->HookInstanceFunction(E, "openFileOutput", [](jnivm::ENV *, EncryptedFile *self) {
            auto s = std::make_shared<OutputStream>();
            s->path = self->path;
            return s;
        });
    }

    inputHooks<0>(E, env->GetClass("java/io/FileInputStream"));
    inputHooks<1>(E, env->GetClass("java/io/InputStream"));
    outputHooks<0>(E, env->GetClass("java/io/FileOutputStream"));
    outputHooks<1>(E, env->GetClass("java/io/OutputStream"));
    outputHooks<2>(E, env->GetClass("java/io/ByteArrayOutputStream"));
    {
        auto baos = env->GetClass("java/io/ByteArrayOutputStream");
        baos->Hook(E, "<init>", [](jnivm::ENV *, jnivm::Class *) { return std::make_shared<ByteArrayOutputStream>(); });
        baos->HookInstanceFunction(E, "toByteArray", [](jnivm::ENV *, ByteArrayOutputStream *s) {
            return std::make_shared<jnivm::Array<jbyte>>((jbyte *) s->buffer.data(), (jsize) s->buffer.size(), true);
        });
    }
}

} // namespace vento_realvnc
