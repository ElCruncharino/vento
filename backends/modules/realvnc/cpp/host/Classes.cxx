// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// The Android and `java.*` classes the core resolves by name on its way to a
// connected session, each as a C++ struct with the members it calls hooked.
// The core needs little of them (a bitmap to draw a cursor into, a map to
// read its options from, a proxy selector that says "direct", a locale, a
// build fingerprint, an empty asset list), and each answer here is the one a
// stock phone would give, or the simplest one that satisfies the call.
// Nothing here is RealVNC's: it implements the Android SDK's surface.
//
// Two jnivm rules shape the code. A constructor is hooked as a *static*
// `"<init>"` returning the object, not as an instance function. And a hook
// with a typed `T*` self parameter is only given objects of C++ type `T`,
// with the derived JNI signature keyed on the C++ types of the parameters.
// So a class that several interface names resolve to (a map, a set, an
// iterator) is hooked under each name with an `Object*` self and a
// `dynamic_cast`, and each name gets a lambda of its own type, since one
// lambda type installed under several names is silently installed once.

#include <algorithm>
#include <memory>
#include <string>
#include <vector>

#include "Host.h"

namespace vento_realvnc {

namespace {

/* runtime::MAX_CURSOR_SIDE: the widest and tallest cursor any backend takes
   from a server. */
constexpr jint MAX_CURSOR_SIDE = 1024;

// ---- the stand-ins ----------------------------------------------------------------

struct BitmapConfig final : jnivm::Object {
    static std::shared_ptr<BitmapConfig> ARGB_8888;
};
std::shared_ptr<BitmapConfig> BitmapConfig::ARGB_8888 = std::make_shared<BitmapConfig>();

struct Entry final : jnivm::Object {
    std::shared_ptr<jnivm::String> key, value;
};
struct Iterator final : jnivm::Object {
    std::vector<std::shared_ptr<jnivm::Object>> items;
    size_t at = 0;
};
struct List final : jnivm::Object {
    std::vector<std::shared_ptr<jnivm::Object>> items;
};
struct Set final : jnivm::Object {
    std::vector<std::shared_ptr<Entry>> items;
};
/* `java.util.Map<String, String>`, the type of both `initApp`'s parameters
   and `createSession`'s options; the core walks `entrySet`. */
struct Map final : jnivm::Object {
    std::vector<std::pair<std::string, std::string>> entries;
};

struct Uri final : jnivm::Object {
    std::string uri;
};
struct ProxyType final : jnivm::Object {
    jint ordinal = 0;
    std::string name;
    static std::shared_ptr<ProxyType> DIRECT, HTTP, SOCKS;
};
std::shared_ptr<ProxyType> ProxyType::DIRECT = std::make_shared<ProxyType>();
std::shared_ptr<ProxyType> ProxyType::HTTP = std::make_shared<ProxyType>();
std::shared_ptr<ProxyType> ProxyType::SOCKS = std::make_shared<ProxyType>();
struct Proxy final : jnivm::Object {};
struct ProxySelector final : jnivm::Object {};
struct SocketAddress final : jnivm::Object {};

/* `Bindings$LogMode`, an enum whose ordinal the core reads. */
struct LogMode final : jnivm::Object {
    jint ordinal = 1;
};

struct Locale final : jnivm::Object {};
struct Thread final : jnivm::Object {};
struct ClassLoader final : jnivm::Object {};
struct Resources final : jnivm::Object {};
struct ContentResolver final : jnivm::Object {};
struct AssetManager final : jnivm::Object {};
struct PrintStream final : jnivm::Object {
    static std::shared_ptr<PrintStream> err;
};
std::shared_ptr<PrintStream> PrintStream::err = std::make_shared<PrintStream>();

/* Device details the core reads: nothing it does depends on them beyond a
   line in its log, so they describe a Linux desktop. */
struct DisplayMetrics final : jnivm::Object {
    jfloat density = 1.0f, scaledDensity = 1.0f, xdpi = 96.0f, ydpi = 96.0f;
    jint densityDpi = 160, widthPixels = 1920, heightPixels = 1080;
};
struct BuildVersion final : jnivm::Object {
    static std::shared_ptr<jnivm::String> RELEASE, INCREMENTAL, CODENAME;
    static jint SDK_INT;
};
std::shared_ptr<jnivm::String> BuildVersion::RELEASE = jstr("14");
std::shared_ptr<jnivm::String> BuildVersion::INCREMENTAL = jstr("1");
std::shared_ptr<jnivm::String> BuildVersion::CODENAME = jstr("REL");
jint BuildVersion::SDK_INT = 34;
struct Build final : jnivm::Object {
    static std::shared_ptr<jnivm::String> MODEL, MANUFACTURER, BRAND, DEVICE, PRODUCT, HARDWARE,
            BOARD, FINGERPRINT, SERIAL, ID, DISPLAY, HOST, TAGS, TYPE, USER;
};
std::shared_ptr<jnivm::String> Build::MODEL = jstr("desktop");
std::shared_ptr<jnivm::String> Build::MANUFACTURER = jstr("linux");
std::shared_ptr<jnivm::String> Build::BRAND = jstr("linux");
std::shared_ptr<jnivm::String> Build::DEVICE = jstr("desktop");
std::shared_ptr<jnivm::String> Build::PRODUCT = jstr("desktop");
#if defined(__x86_64__)
std::shared_ptr<jnivm::String> Build::HARDWARE = jstr("x86_64");
#else
std::shared_ptr<jnivm::String> Build::HARDWARE = jstr("arm64");
#endif
std::shared_ptr<jnivm::String> Build::BOARD = jstr("pc");
std::shared_ptr<jnivm::String> Build::FINGERPRINT = jstr("linux/desktop:14/REL/1:user/release-keys");
std::shared_ptr<jnivm::String> Build::SERIAL = jstr("unknown");
std::shared_ptr<jnivm::String> Build::ID = jstr("REL");
std::shared_ptr<jnivm::String> Build::DISPLAY = jstr("REL");
std::shared_ptr<jnivm::String> Build::HOST = jstr("localhost");
std::shared_ptr<jnivm::String> Build::TAGS = jstr("release-keys");
std::shared_ptr<jnivm::String> Build::TYPE = jstr("user");
std::shared_ptr<jnivm::String> Build::USER = jstr("user");

/* The four cloud callbacks `initVwr` takes, which `Bindings.NoCloud` answers
   with nothing on the phone; here the core has never called one. */
struct SignInUi final : jnivm::Object {};
struct SyncCallback final : jnivm::Object {};
struct DirectoryCallback final : jnivm::Object {};
struct LabelCallback final : jnivm::Object {};

template <class T>
std::shared_ptr<Iterator> iteratorOver(const std::vector<std::shared_ptr<T>> &items) {
    auto it = std::make_shared<Iterator>();
    for (const auto &x : items) {
        it->items.push_back(x);
    }
    return it;
}

// ---- hooks under several names ------------------------------------------------------

/* `N` gives each name its own lambda types. */
template <int N>
void mapHooks(jnivm::ENV *E, const std::shared_ptr<jnivm::Class> &c) {
    c->HookInstanceFunction(E, "entrySet", [](jnivm::ENV *, jnivm::Object *self) {
        auto *m = dynamic_cast<Map *>(self);
        auto s = std::make_shared<Set>();
        if (m) {
            for (const auto &kv : m->entries) {
                auto e = std::make_shared<Entry>();
                e->key = jstr(kv.first);
                e->value = jstr(kv.second);
                s->items.push_back(e);
            }
        }
        return s;
    });
    c->HookInstanceFunction(E, "size", [](jnivm::ENV *, jnivm::Object *self) {
        auto *m = dynamic_cast<Map *>(self);
        return (jint) (m ? m->entries.size() : 0);
    });
    c->HookInstanceFunction(E, "isEmpty", [](jnivm::ENV *, jnivm::Object *self) {
        auto *m = dynamic_cast<Map *>(self);
        return (jboolean) (!m || m->entries.empty());
    });
    c->HookInstanceFunction(E, "get",
                            [](jnivm::ENV *, jnivm::Object *self,
                               std::shared_ptr<jnivm::Object> key) -> std::shared_ptr<jnivm::Object> {
                                auto *m = dynamic_cast<Map *>(self);
                                auto k = std::dynamic_pointer_cast<jnivm::String>(key);
                                if (m && k) {
                                    for (const auto &kv : m->entries) {
                                        if (kv.first == *k) {
                                            return jstr(kv.second);
                                        }
                                    }
                                }
                                return nullptr;
                            });
}

template <int N>
void setHooks(jnivm::ENV *E, const std::shared_ptr<jnivm::Class> &c) {
    c->HookInstanceFunction(E, "iterator", [](jnivm::ENV *, jnivm::Object *self) {
        auto *s = dynamic_cast<Set *>(self);
        return iteratorOver(s ? s->items : std::vector<std::shared_ptr<Entry>>{});
    });
    c->HookInstanceFunction(E, "size", [](jnivm::ENV *, jnivm::Object *self) {
        auto *s = dynamic_cast<Set *>(self);
        return (jint) (s ? s->items.size() : 0);
    });
}

} // namespace

/* Used by `DesktopCore.cxx` to make a `Map` for the two calls that take one. */
std::shared_ptr<jnivm::Object> makeMap(const std::vector<std::pair<std::string, std::string>> &entries) {
    auto m = std::make_shared<Map>();
    m->entries = entries;
    return m;
}

std::shared_ptr<jnivm::Object> makeLogMode(int ordinal) {
    auto m = std::make_shared<LogMode>();
    m->ordinal = ordinal;
    return m;
}

std::shared_ptr<jnivm::Object> makeNoCloud(int which) {
    switch (which) {
        case 0: return std::make_shared<SignInUi>();
        case 1: return std::make_shared<SyncCallback>();
        case 2: return std::make_shared<DirectoryCallback>();
        default: return std::make_shared<LabelCallback>();
    }
}

void registerClasses(jnivm::VM &vm, const std::shared_ptr<Context> &context) {
    auto env = vm.GetEnv();
    auto *E = env.get();

    env->GetClass<Bitmap>("android/graphics/Bitmap");
    env->GetClass<BitmapConfig>("android/graphics/Bitmap$Config");
    env->GetClass<Entry>("java/util/Map$Entry");
    env->GetClass<Iterator>("java/util/Iterator");
    env->GetClass<Set>("java/util/Set");
    env->GetClass<Map>("java/util/Map");
    env->GetClass<List>("java/util/List");
    env->GetClass<Context>("android/content/Context");
    env->GetClass<LogMode>("com/realvnc/vncviewer/jni/Bindings$LogMode");
    env->GetClass<SignInUi>("com/realvnc/vncviewer/jni/SignInMgrBindings$SignInUi");
    env->GetClass<SyncCallback>("com/realvnc/vncviewer/jni/SyncMgrBindings$Callback");
    env->GetClass<DirectoryCallback>("com/realvnc/vncviewer/jni/DirectoryBrowserBindings$Callback");
    env->GetClass<LabelCallback>("com/realvnc/vncviewer/jni/LabelBindings$Callback");
    env->GetClass<Uri>("java/net/URI");
    env->GetClass<ProxyType>("java/net/Proxy$Type");
    env->GetClass<Proxy>("java/net/Proxy");
    env->GetClass<ProxySelector>("java/net/ProxySelector");
    env->GetClass<SocketAddress>("java/net/SocketAddress");
    env->GetClass<Locale>("java/util/Locale");
    env->GetClass<Thread>("java/lang/Thread");
    env->GetClass<ClassLoader>("java/lang/ClassLoader");
    env->GetClass<Resources>("android/content/res/Resources");
    env->GetClass<ContentResolver>("android/content/ContentResolver");
    env->GetClass<DisplayMetrics>("android/util/DisplayMetrics");
    env->GetClass<PrintStream>("java/io/PrintStream");
    env->GetClass<AssetManager>("android/content/res/AssetManager");
    env->GetClass<BuildVersion>("android/os/Build$VERSION");
    env->GetClass<Build>("android/os/Build");

    // ---- the bitmap the cursor is drawn into ----
    env->GetClass("android/graphics/Bitmap")
            ->Hook(E, "createBitmap",
                   [](jnivm::ENV *, jnivm::Class *, jint w, jint h, std::shared_ptr<BitmapConfig>) {
                       auto b = std::make_shared<Bitmap>();
                       // The server sets the cursor's size, and this allocates
                       // its pixels: past runtime::MAX_CURSOR_SIDE (the other
                       // backends' limit) the bitmap is left empty, which the
                       // pixel lock (Loader.cxx) refuses, so the core draws no
                       // cursor rather than allocating gigabytes.
                       if (w < 0 || h < 0 || w > MAX_CURSOR_SIDE || h > MAX_CURSOR_SIDE) {
                           LOGW("a cursor of %dx%d is past %d a side; not drawn", w, h, MAX_CURSOR_SIDE);
                           return b;
                       }
                       b->width = w;
                       b->height = h;
                       b->pixels.resize((size_t) w * (size_t) h);
                       return b;
                   });
    env->GetClass("android/graphics/Bitmap$Config")->Hook(E, "ARGB_8888", &BitmapConfig::ARGB_8888);
    env->GetClass("android/graphics/Bitmap")
            ->HookInstanceFunction(E, "setHasAlpha", [](jnivm::ENV *, Bitmap *, jboolean) {});

    // ---- the collections, under every name the core resolves them by ----
    mapHooks<0>(E, env->GetClass("java/util/Map"));
    mapHooks<1>(E, env->GetClass("java/util/HashMap"));
    setHooks<0>(E, env->GetClass("java/util/Set"));
    setHooks<1>(E, env->GetClass("java/util/Collection"));
    setHooks<2>(E, env->GetClass("java/lang/Iterable"));
    {
        auto c = env->GetClass("java/util/Iterator");
        c->HookInstanceFunction(E, "hasNext", [](jnivm::ENV *, jnivm::Object *self) {
            auto *it = dynamic_cast<Iterator *>(self);
            return (jboolean) (it && it->at < it->items.size());
        });
        c->HookInstanceFunction(E, "next",
                                [](jnivm::ENV *, jnivm::Object *self) -> std::shared_ptr<jnivm::Object> {
                                    auto *it = dynamic_cast<Iterator *>(self);
                                    if (!it || it->at >= it->items.size()) {
                                        return nullptr;
                                    }
                                    return it->items[it->at++];
                                });
    }
    {
        auto c = env->GetClass("java/util/Map$Entry");
        c->HookInstanceFunction(E, "getKey",
                                [](jnivm::ENV *, jnivm::Object *self) -> std::shared_ptr<jnivm::Object> {
                                    auto *e = dynamic_cast<Entry *>(self);
                                    return e ? e->key : nullptr;
                                });
        c->HookInstanceFunction(E, "getValue",
                                [](jnivm::ENV *, jnivm::Object *self) -> std::shared_ptr<jnivm::Object> {
                                    auto *e = dynamic_cast<Entry *>(self);
                                    return e ? e->value : nullptr;
                                });
    }
    {
        auto l = env->GetClass("java/util/List");
        l->HookInstanceFunction(E, "iterator",
                                [](jnivm::ENV *, List *self) { return iteratorOver(self->items); });
        l->HookInstanceFunction(E, "size",
                                [](jnivm::ENV *, List *self) { return (jint) self->items.size(); });
        l->HookInstanceFunction(E, "get",
                                [](jnivm::ENV *, List *self, jint i) -> std::shared_ptr<jnivm::Object> {
                                    return i >= 0 && (size_t) i < self->items.size() ? self->items[i]
                                                                                     : nullptr;
                                });
        l->HookInstanceFunction(E, "add", [](jnivm::ENV *, List *self, std::shared_ptr<jnivm::Object> o) {
            self->items.push_back(o);
            return (jboolean) true;
        });
    }

    // ---- java.net: the core asks the system's proxy selector about the
    // server and is told to go direct ----
    ProxyType::DIRECT->ordinal = 0;
    ProxyType::DIRECT->name = "DIRECT";
    ProxyType::HTTP->ordinal = 1;
    ProxyType::HTTP->name = "HTTP";
    ProxyType::SOCKS->ordinal = 2;
    ProxyType::SOCKS->name = "SOCKS";
    env->GetClass("java/net/URI")->Hook(E, "<init>", [](jnivm::ENV *, jnivm::Class *,
                                                         std::shared_ptr<jnivm::String> u) {
        auto x = std::make_shared<Uri>();
        x->uri = fromJstr(u);
        return x;
    });
    {
        auto pt = env->GetClass("java/net/Proxy$Type");
        pt->Hook(E, "DIRECT", &ProxyType::DIRECT);
        pt->Hook(E, "HTTP", &ProxyType::HTTP);
        pt->Hook(E, "SOCKS", &ProxyType::SOCKS);
        pt->HookInstanceFunction(E, "ordinal", [](jnivm::ENV *, ProxyType *t) { return t->ordinal; });
        pt->HookInstanceFunction(E, "name", [](jnivm::ENV *, ProxyType *t) { return jstr(t->name); });
        pt->Hook(E, "values", [](jnivm::ENV *, jnivm::Class *) {
            auto a = std::make_shared<jnivm::Array<ProxyType>>((jsize) 3);
            (*a)[0] = ProxyType::DIRECT;
            (*a)[1] = ProxyType::HTTP;
            (*a)[2] = ProxyType::SOCKS;
            return a;
        });
        auto px = env->GetClass("java/net/Proxy");
        px->HookInstanceFunction(E, "type", [](jnivm::ENV *, Proxy *) { return ProxyType::DIRECT; });
        px->HookInstanceFunction(E, "address",
                                 [](jnivm::ENV *, Proxy *) -> std::shared_ptr<SocketAddress> {
                                     return nullptr;
                                 });
        auto ps = env->GetClass("java/net/ProxySelector");
        ps->Hook(E, "getDefault",
                 [](jnivm::ENV *, jnivm::Class *) { return std::make_shared<ProxySelector>(); });
        ps->HookInstanceFunction(E, "select", [](jnivm::ENV *, ProxySelector *, std::shared_ptr<Uri>) {
            auto l = std::make_shared<List>();
            l->items.push_back(std::make_shared<Proxy>());
            return l;
        });
    }

    // ---- enums, by ordinal ----
    env->GetClass("com/realvnc/vncviewer/jni/Bindings$LogMode")
            ->HookInstanceFunction(E, "ordinal", [](jnivm::ENV *, LogMode *m) { return m->ordinal; });
    env->GetClass("java/lang/Enum")->HookInstanceFunction(E, "ordinal", [](jnivm::ENV *, jnivm::Object *self) {
        if (auto *m = dynamic_cast<LogMode *>(self)) {
            return m->ordinal;
        }
        if (auto *t = dynamic_cast<ProxyType *>(self)) {
            return t->ordinal;
        }
        return (jint) 0;
    });
    env->GetClass("java/lang/Enum")->HookInstanceFunction(E, "name", [](jnivm::ENV *, jnivm::Object *self) {
        if (auto *t = dynamic_cast<ProxyType *>(self)) {
            return jstr(t->name);
        }
        return jstr("LOG_DEBUG");
    });

    // ---- the context and what it hands out ----
    {
        auto ctx = env->GetClass("android/content/Context");
        ctx->HookInstanceFunction(E, "getApplicationContext", [context](jnivm::ENV *, jnivm::Object *) {
            return context;
        });
        ctx->HookInstanceFunction(E, "getResources",
                                  [](jnivm::ENV *, jnivm::Object *) { return std::make_shared<Resources>(); });
        ctx->HookInstanceFunction(E, "getContentResolver", [](jnivm::ENV *, jnivm::Object *) {
            return std::make_shared<ContentResolver>();
        });
        ctx->HookInstanceFunction(E, "getAssets",
                                  [](jnivm::ENV *, jnivm::Object *) { return std::make_shared<AssetManager>(); });
    }
    env->GetClass("android/content/res/Resources")
            ->HookInstanceFunction(E, "getAssets",
                                   [](jnivm::ENV *, jnivm::Object *) { return std::make_shared<AssetManager>(); });
    env->GetClass("android/content/res/Resources")
            ->HookInstanceFunction(E, "getDisplayMetrics", [](jnivm::ENV *, jnivm::Object *) {
                return std::make_shared<DisplayMetrics>();
            });
    // The core looks for its translations (`strings.en_US.po`) among the
    // assets; there are none, so it stays in English, as on the phone.
    env->GetClass("android/content/res/AssetManager")
            ->HookInstanceFunction(E, "list", [](jnivm::ENV *, jnivm::Object *, std::shared_ptr<jnivm::String>) {
                return std::make_shared<jnivm::Array<jnivm::String>>((jsize) 0);
            });
    {
        auto dm = env->GetClass("android/util/DisplayMetrics");
        dm->Hook(E, "density", &DisplayMetrics::density);
        dm->Hook(E, "scaledDensity", &DisplayMetrics::scaledDensity);
        dm->Hook(E, "xdpi", &DisplayMetrics::xdpi);
        dm->Hook(E, "ydpi", &DisplayMetrics::ydpi);
        dm->Hook(E, "densityDpi", &DisplayMetrics::densityDpi);
        dm->Hook(E, "widthPixels", &DisplayMetrics::widthPixels);
        dm->Hook(E, "heightPixels", &DisplayMetrics::heightPixels);
    }
    // The device id, which the core uses as a salt; any fixed string works.
    env->GetClass("android/provider/Settings$Secure")
            ->Hook(E, "getString",
                   [](jnivm::ENV *, jnivm::Class *, std::shared_ptr<jnivm::Object>, std::shared_ptr<jnivm::String>) {
                       return jstr("0123456789abcdef");
                   });
    {
        auto c = env->GetClass("android/os/Build$VERSION");
        c->Hook(E, "RELEASE", &BuildVersion::RELEASE);
        c->Hook(E, "INCREMENTAL", &BuildVersion::INCREMENTAL);
        c->Hook(E, "CODENAME", &BuildVersion::CODENAME);
        c->Hook(E, "SDK_INT", &BuildVersion::SDK_INT);
    }
    {
        auto c = env->GetClass("android/os/Build");
        c->Hook(E, "MODEL", &Build::MODEL);
        c->Hook(E, "MANUFACTURER", &Build::MANUFACTURER);
        c->Hook(E, "BRAND", &Build::BRAND);
        c->Hook(E, "DEVICE", &Build::DEVICE);
        c->Hook(E, "PRODUCT", &Build::PRODUCT);
        c->Hook(E, "HARDWARE", &Build::HARDWARE);
        c->Hook(E, "BOARD", &Build::BOARD);
        c->Hook(E, "FINGERPRINT", &Build::FINGERPRINT);
        c->Hook(E, "SERIAL", &Build::SERIAL);
        c->Hook(E, "ID", &Build::ID);
        c->Hook(E, "DISPLAY", &Build::DISPLAY);
        c->Hook(E, "HOST", &Build::HOST);
        c->Hook(E, "TAGS", &Build::TAGS);
        c->Hook(E, "TYPE", &Build::TYPE);
        c->Hook(E, "USER", &Build::USER);
    }

    // ---- java.lang ----
    {
        auto loc = env->GetClass("java/util/Locale");
        loc->Hook(E, "getDefault", [](jnivm::ENV *, jnivm::Class *) { return std::make_shared<Locale>(); });
        loc->HookInstanceFunction(E, "getLanguage", [](jnivm::ENV *, jnivm::Object *) { return jstr("en"); });
        loc->HookInstanceFunction(E, "getCountry", [](jnivm::ENV *, jnivm::Object *) { return jstr("US"); });
        loc->HookInstanceFunction(E, "toString", [](jnivm::ENV *, jnivm::Object *) { return jstr("en_US"); });
    }
    {
        // The core loads its callback classes through the context class
        // loader rather than `FindClass`, so one is provided.
        auto thr = env->GetClass("java/lang/Thread");
        thr->Hook(E, "currentThread", [](jnivm::ENV *, jnivm::Class *) { return std::make_shared<Thread>(); });
        thr->HookInstanceFunction(E, "getContextClassLoader",
                                  [](jnivm::ENV *, jnivm::Object *) { return std::make_shared<ClassLoader>(); });
        env->GetClass("java/lang/ClassLoader")
                ->HookInstanceFunction(E, "loadClass",
                                       [](jnivm::ENV *e, jnivm::Object *, std::shared_ptr<jnivm::String> name) {
                                           std::string n = fromJstr(name);
                                           std::replace(n.begin(), n.end(), '.', '/');
                                           return e->GetClass(n.c_str());
                                       });
        env->GetClass("java/lang/Class")->HookInstanceFunction(E, "getName", [](jnivm::ENV *, jnivm::Object *self) {
            auto *c = dynamic_cast<jnivm::Class *>(self);
            std::string n = c ? c->getName() : "?";
            std::replace(n.begin(), n.end(), '/', '.');
            return jstr(n);
        });
    }
    {
        auto sys = env->GetClass("java/lang/System");
        // `http.proxyHost` and the like: none set.
        sys->Hook(E, "getProperty",
                  [](jnivm::ENV *, jnivm::Class *, std::shared_ptr<jnivm::String>) -> std::shared_ptr<jnivm::String> {
                      return nullptr;
                  });
        sys->Hook(E, "err", &PrintStream::err);
        env->GetClass("java/io/PrintStream")
                ->HookInstanceFunction(E, "println", [](jnivm::ENV *, jnivm::Object *, std::shared_ptr<jnivm::String> s) {
                    LOGW("System.err: %s", fromJstr(s).c_str());
                });
    }
    {
        // Used when the core prints a caught exception, which under jnivm is
        // a class rather than a thrown object.
        env->GetClass("java/lang/Throwable")->HookInstanceFunction(E, "toString", [](jnivm::ENV *, jnivm::Object *self) {
            auto *c = dynamic_cast<jnivm::Class *>(self);
            return jstr(c ? c->getName() : "java.lang.Throwable");
        });
        env->GetClass("java/lang/Throwable")
                ->HookInstanceFunction(E, "getMessage",
                                       [](jnivm::ENV *, jnivm::Object *) -> std::shared_ptr<jnivm::String> {
                                           return nullptr;
                                       });
        env->GetClass("java/lang/NullPointerException")
                ->HookInstanceFunction(E, "toString",
                                       [](jnivm::ENV *, jnivm::Object *) { return jstr("java.lang.NullPointerException"); });
    }

    // ---- AuthkeyStoreAndroid: no client identities enrolled, as
    // `RealVncCore` configures it, so identity authentication has nothing to
    // choose from and falls back to a password ----
    env->GetClass("com/realvnc/vncviewer/jni/AuthkeyStoreAndroid")
            ->Hook(E, "getAliases", [](jnivm::ENV *, jnivm::Class *) {
                return std::make_shared<jnivm::Array<jnivm::Object>>((jsize) 0);
            });
}

} // namespace vento_realvnc
