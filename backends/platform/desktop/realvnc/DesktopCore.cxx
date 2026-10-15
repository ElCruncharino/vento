// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// `HostedCore` and `HostedSession` for a Linux host: the viewer core loaded
// by `Loader.cxx`, a jnivm VM with the stand-ins of `Classes.cxx` and
// `Files.cxx` in it, and this file's own classes — the `com.realvnc.vncviewer.
// jni` callbacks and dialog factories the core resolves by name, each
// bridged to the session's `Listener`. The natives are the core's exported
// `Java_com_realvnc_vncviewer_jni_*` functions, called directly with the
// VM's `JNIEnv`, which is all a JVM would do.
//
// Three rules found by running the core here, and kept:
//
//   * A deleted global reference stays alive. In `sessionClosed` the core
//     deletes its reference to the `Session` object and then calls
//     `getToken` through the same handle. A JVM hands out indirect handles
//     and tolerates that; jnivm hands out the wrapper's own pointer and frees
//     it, so the late call was a use-after-free. `DeleteGlobalRef` is
//     replaced in the VM's function table with one that keeps every wrapper
//     it is given — a handful of objects a session, for the process's life.
//   * Nothing is destroyed or closed after `sessionClosed`; the session file
//     keeps that rule and this file never calls `destroySession` at all.
//   * The JNI env is per thread. The core's own thread has one from the
//     start; the drawing thread, which calls `setScaleBufferSize` and
//     `copyScaledRegion`, attaches itself on first use, as a JVM would ask.
//   * Every call into the core runs in a local frame of its own. A JVM drops
//     a native's local references when it returns, and the core relies on
//     that: it never deletes one. Its natives here are called from C++, not
//     from a JVM, and the looper's callbacks are not natives at all, so
//     without a frame popped around each every local the core or a hook
//     made stayed in the thread's top frame, which grew for the life of the
//     process: strings, map entries and arrays, hundreds to thousands of
//     them a minute.

#include "HostedCore.h"

#include <pthread.h>
#include <sys/stat.h>

#include <memory>
#include <mutex>
#include <string>
#include <unordered_map>
#include <vector>

#include "Host.h"
#include "Loader.h"
#include "Looper.h"

namespace vento_realvnc {

/* `Classes.cxx`'s. */
std::shared_ptr<jnivm::Object> makeMap(const std::vector<std::pair<std::string, std::string>> &entries);
std::shared_ptr<jnivm::Object> makeLogMode(int ordinal);
std::shared_ptr<jnivm::Object> makeNoCloud(int which);

namespace {

// ---- the global-reference graveyard ------------------------------------------

std::vector<std::shared_ptr<jnivm::Object>> &graveyard() {
    static auto *the = new std::vector<std::shared_ptr<jnivm::Object>>;
    return *the;
}
std::mutex graveyardLock;
void (*origDeleteGlobalRef)(JNIEnv *, jobject);

void keepDeleteGlobalRef(JNIEnv *env, jobject obj) {
    // The cast must target `Global` itself: a cast to `Object` would unwrap
    // the wrapper, and the wrapper is what the stale handle points at.
    if (auto *g = dynamic_cast<jnivm::Global *>((jnivm::Object *) obj)) {
        const std::lock_guard<std::mutex> guard(graveyardLock);
        graveyard().push_back(std::static_pointer_cast<jnivm::Object>(g->shared_from_this()));
    }
    origDeleteGlobalRef(env, obj);
}

// ---- the core's own classes ----------------------------------------------------------

/* The listener each of these answers to is found through the `Session`
   object the core hands the factory, or — for a prompt that arrives during
   `createSession`, before there is one — through `pending`, the listener
   whose `createSession` is in flight; there is at most one, since every
   `createSession` happens on the one thread. */
struct SessionCallback final : jnivm::Object {
    Listener *listener = nullptr;
};
struct DesktopCallback final : jnivm::Object {
    Listener *listener = nullptr;
};
struct ReconnectFactory final : jnivm::Object {};
struct ReconnectDlg final : jnivm::Object {
    Listener *listener = nullptr;
};
struct SecurityFactory final : jnivm::Object {};
struct SecurityDlg final : jnivm::Object {
    Listener *listener = nullptr;
    jlong cookie = 0;
};
struct MsgBoxFactory final : jnivm::Object {};
struct MsgBox final : jnivm::Object {};
struct PasswdFactory final : jnivm::Object {};
struct PasswdDlg final : jnivm::Object {
    Listener *listener = nullptr;
    jlong cookie = 0;
};
struct InteractiveFactory final : jnivm::Object {};
struct InteractiveDlg final : jnivm::Object {
    jlong cookie = 0;
};
struct AuthkeyChoiceFactory final : jnivm::Object {};
struct AuthkeyChoiceDlg final : jnivm::Object {
    jlong cookie = 0;
};
struct StatusNotifier final : jnivm::Object {};
struct SecurityNotifier final : jnivm::Object {};
struct CredentialsStore final : jnivm::Object {};
struct IdentityStore final : jnivm::Object {};

/* The core's exported natives this file calls. */
struct Natives {
    void (*initApp)(JNIEnv *, jclass, jstring, jstring, jstring, jstring, jstring, jobject, jobject,
                    jboolean, jobject);
    void (*initVwr)(JNIEnv *, jclass, jobject, jobject, jobject, jobject);
    jobject (*createSession)(JNIEnv *, jclass, jobject, jobject, jstring, jstring, jobject);
    void (*focusEvent)(JNIEnv *, jclass, jobject, jboolean);
    void (*pointerEvent)(JNIEnv *, jclass, jobject, jint, jint, jint);
    void (*keyDownEvent)(JNIEnv *, jclass, jobject, jlong, jint);
    void (*keyUpEvent)(JNIEnv *, jclass, jobject, jint);
    void (*setOption)(JNIEnv *, jclass, jobject, jstring, jstring);
    void (*applyOptions)(JNIEnv *, jclass, jobject);
    void (*closeSession)(JNIEnv *, jclass, jobject);
    jobject (*setScaleBufferSize)(JNIEnv *, jclass, jobject, jint, jint);
    void (*copyScaledRegion)(JNIEnv *, jclass, jobject, jint, jint, jint, jint, jfloat);
    jstring (*info[8])(JNIEnv *, jclass, jobject);
    void (*passwdResult)(JNIEnv *, jclass, jlong, jboolean, jstring, jstring);
    void (*securityResult)(JNIEnv *, jclass, jlong, jboolean, jboolean, jboolean);
    void (*msgBoxResult)(JNIEnv *, jclass, jlong, jboolean);
    void (*interactiveResult)(JNIEnv *, jclass, jlong, jboolean, jobject);
    void (*identityChosen)(JNIEnv *, jclass, jlong, jboolean, jstring);
};

class DesktopCore;
DesktopCore *theCore = nullptr;

/* A local frame for the length of a scope, which is what a JVM gives every
   native call. Whatever has to outlive it is held by a `shared_ptr` of our
   own (the session's handle and buffer) or copied out (a string). */
class LocalFrame {
public:
    explicit LocalFrame(JNIEnv *env) : env_(env) { env_->PushLocalFrame(16); }
    ~LocalFrame() { env_->PopLocalFrame(nullptr); }
    LocalFrame(const LocalFrame &) = delete;
    LocalFrame &operator=(const LocalFrame &) = delete;

private:
    JNIEnv *env_;
};

class DesktopSession : public HostedSession {
public:
    DesktopSession(DesktopCore *core, jobject handle, std::shared_ptr<SessionHandle> object)
            : core_(core), handle_(handle), object_(std::move(object)) {}

    uintptr_t token() override { return (uintptr_t) object_->token; }
    void focusEvent(bool focused) override;
    void pointerEvent(int x, int y, int buttonMask) override;
    void keyDownEvent(int64_t keysym, int keycode) override;
    void keyUpEvent(int keycode) override;
    void setOption(const std::string &name, const std::string &value) override;
    void applyOptions() override;
    void closeSession() override;
    std::string info(Info which) override;
    uint8_t *setScaleBufferSize(int width, int height) override;
    bool copyScaledRegion(int x, int y, int width, int height, float scale) override;

private:
    DesktopCore *core_;
    jobject handle_;                        // what the natives take
    std::shared_ptr<SessionHandle> object_; // kept, so the token stays readable
    std::shared_ptr<jnivm::Object> buffer_; // the last ByteBuffer, so its memory stays
};

class DesktopCore : public HostedCore {
public:
    bool open(const std::string &libraryDir, const Bootstrap &bootstrap, std::string *why);

    void pollOnce(int timeoutMs) override {
        // The looper's callbacks are the core's, and call back into the hooks.
        const LocalFrame frame(jenv_);
        looper_->pollOnce(timeoutMs, nullptr, nullptr, nullptr);
    }

    void wake() override { looper_->wake(); }

    HostedSession *createSession(Listener *listener, const std::string &address,
                                 const char *password, const Options &options) override {
        JNIEnv *env = jenv_;
        const LocalFrame frame(env);
        auto scb = std::make_shared<SessionCallback>();
        scb->listener = listener;
        auto dcb = std::make_shared<DesktopCallback>();
        dcb->listener = listener;
        // Kept, as the core keeps them: a session's callbacks are its for life.
        keep_.push_back(scb);
        keep_.push_back(dcb);
        pending_ = listener;
        jobject handle = natives_.createSession(env, nullptr, toJni(env_, scb), toJni(env_, dcb),
                                                env->NewStringUTF(address.c_str()),
                                                password ? env->NewStringUTF(password) : nullptr,
                                                toJni(env_, makeMap(options)));
        pending_ = nullptr;
        if (env->ExceptionCheck()) {
            env->ExceptionDescribe();
            env->ExceptionClear();
        }
        auto *object = unwrap<SessionHandle>(handle);
        if (!handle || !object) {
            LOGW("createSession returned nothing");
            return nullptr;
        }
        auto shared = std::static_pointer_cast<SessionHandle>(object->shared_from_this());
        owners_[object->token] = listener;
        // The object itself rather than `handle`, which is a local reference
        // and gone with the frame; `shared` keeps the object alive.
        auto *s = new DesktopSession(this, (jobject) static_cast<jnivm::Object *>(object), shared);
        sessions_.push_back(s);
        return s;
    }

    void passwdResult(int64_t cookie, bool ok, const char *userName, const char *password) override {
        const LocalFrame frame(jenv_);
        natives_.passwdResult(jenv_, nullptr, cookie, ok, userName ? jenv_->NewStringUTF(userName) : nullptr,
                              password ? jenv_->NewStringUTF(password) : nullptr);
    }

    void securityResult(int64_t cookie, bool accept, bool saveIdentity, bool saveEncryption) override {
        const LocalFrame frame(jenv_);
        natives_.securityResult(jenv_, nullptr, cookie, accept, saveIdentity, saveEncryption);
    }

    void msgBoxResult(int64_t cookie, bool ok) override {
        const LocalFrame frame(jenv_);
        natives_.msgBoxResult(jenv_, nullptr, cookie, ok);
    }

    // ---- for the session ----

    const Natives &natives() const { return natives_; }
    JNIEnv *env() const { return jenv_; }
    jnivm::ENV *envObject() const { return env_; }

    /* The core thread's env, or this thread's own, attached on first use. */
    JNIEnv *envForThisThread() {
        if (pthread_equal(pthread_self(), coreThread_)) {
            return jenv_;
        }
        thread_local JNIEnv *mine = nullptr;
        if (!mine) {
            vm_->GetJavaVM()->AttachCurrentThread(&mine, nullptr);
        }
        return mine;
    }

    /* `RealVncPrompts.owner`. */
    Listener *ownerOf(jobject session) {
        if (auto *s = unwrap<SessionHandle>(session)) {
            const auto it = owners_.find(s->token);
            if (it != owners_.end()) {
                return it->second;
            }
        }
        return pending_;
    }

private:
    void registerBindings();

    jnivm::VM *vm_ = nullptr;      // leaked: the core holds pointers into it for life
    jnivm::ENV *env_ = nullptr;
    JNIEnv *jenv_ = nullptr;
    void *core_ = nullptr;
    Natives natives_{};
    Looper *looper_ = nullptr;
    pthread_t coreThread_{};
    std::shared_ptr<Context> context_;
    std::vector<std::shared_ptr<jnivm::Object>> keep_;
    std::unordered_map<jlong, Listener *> owners_;
    Listener *pending_ = nullptr;
    std::vector<DesktopSession *> sessions_; // never freed, as the core never lets one go
};

// ---- the session's natives -------------------------------------------------------------

void DesktopSession::focusEvent(bool focused) {
    const LocalFrame frame(core_->env());
    core_->natives().focusEvent(core_->env(), nullptr, handle_, focused);
}

void DesktopSession::pointerEvent(int x, int y, int buttonMask) {
    const LocalFrame frame(core_->env());
    core_->natives().pointerEvent(core_->env(), nullptr, handle_, x, y, buttonMask);
}

void DesktopSession::keyDownEvent(int64_t keysym, int keycode) {
    const LocalFrame frame(core_->env());
    core_->natives().keyDownEvent(core_->env(), nullptr, handle_, keysym, keycode);
    if (core_->env()->ExceptionCheck()) {
        // A keysym that does not fit in 31 bits, which the core throws about.
        core_->env()->ExceptionClear();
        LOGW("keyDownEvent refused keysym 0x%llx", (unsigned long long) keysym);
    }
}

void DesktopSession::keyUpEvent(int keycode) {
    const LocalFrame frame(core_->env());
    core_->natives().keyUpEvent(core_->env(), nullptr, handle_, keycode);
}

void DesktopSession::setOption(const std::string &name, const std::string &value) {
    JNIEnv *env = core_->env();
    const LocalFrame frame(env);
    core_->natives().setOption(env, nullptr, handle_, env->NewStringUTF(name.c_str()),
                               env->NewStringUTF(value.c_str()));
}

void DesktopSession::applyOptions() {
    const LocalFrame frame(core_->env());
    core_->natives().applyOptions(core_->env(), nullptr, handle_);
}

void DesktopSession::closeSession() {
    const LocalFrame frame(core_->env());
    core_->natives().closeSession(core_->env(), nullptr, handle_);
}

std::string DesktopSession::info(Info which) {
    // Copied out before the frame goes, which takes the string with it.
    const LocalFrame frame(core_->env());
    jstring s = core_->natives().info[(int) which](core_->env(), nullptr, handle_);
    auto *str = unwrap<jnivm::String>(s);
    return str ? std::string(*str) : std::string();
}

uint8_t *DesktopSession::setScaleBufferSize(int width, int height) {
    JNIEnv *env = core_->envForThisThread();
    const LocalFrame frame(env);
    jobject bb = core_->natives().setScaleBufferSize(env, nullptr, handle_, width, height);
    auto *buffer = unwrap<jnivm::ByteBuffer>(bb);
    if (!buffer || !buffer->buffer || buffer->capacity < (jlong) width * height * 4) {
        return nullptr;
    }
    buffer_ = std::static_pointer_cast<jnivm::Object>(buffer->shared_from_this());
    return (uint8_t *) buffer->buffer;
}

bool DesktopSession::copyScaledRegion(int x, int y, int width, int height, float scale) {
    JNIEnv *env = core_->envForThisThread();
    const LocalFrame frame(env);
    core_->natives().copyScaledRegion(env, nullptr, handle_, x, y, width, height, scale);
    if (env->ExceptionCheck()) {
        env->ExceptionClear();
        return false;
    }
    return true;
}

// ---- bringing the core up -------------------------------------------------------------

template <class F>
bool symbol(void *core, const char *name, F *out, std::string *why) {
    *out = (F) coreSymbol(core, name);
    if (!*out) {
        *why = std::string("missing ") + name;
        return false;
    }
    return true;
}

bool DesktopCore::open(const std::string &libraryDir, const Bootstrap &b, std::string *why) {
    core_ = loadCore(libraryDir, why);
    if (!core_) {
        return false;
    }
    coreThread_ = pthread_self();
    looper_ = Looper::forThread();

    vm_ = new jnivm::VM;
    context_ = std::make_shared<Context>();
    registerClasses(*vm_, context_);
    registerFiles(*vm_);
    registerBindings();
    env_ = vm_->GetEnv().get();
    jenv_ = vm_->GetJNIEnv();
    {
        auto *table = const_cast<JNINativeInterface *>(jenv_->functions);
        origDeleteGlobalRef = table->DeleteGlobalRef;
        table->DeleteGlobalRef = keepDeleteGlobalRef;
    }

    auto onLoad = (jint(*)(JavaVM *, void *)) coreSymbol(core_, "JNI_OnLoad");
    if (!onLoad) {
        *why = "no JNI_OnLoad";
        return false;
    }
    onLoad(vm_->GetJavaVM(), nullptr);

#define SYM(field, name) \
    if (!symbol(core_, "Java_com_realvnc_vncviewer_jni_" name, &natives_.field, why)) return false;
    SYM(initApp, "Bindings_initApp");
    SYM(initVwr, "Bindings_initVwr");
    SYM(createSession, "SessionBindings_createSession");
    SYM(focusEvent, "SessionBindings_focusEvent");
    SYM(pointerEvent, "SessionBindings_pointerEvent");
    SYM(keyDownEvent, "SessionBindings_keyDownEvent");
    SYM(keyUpEvent, "SessionBindings_keyUpEvent");
    SYM(setOption, "SessionBindings_setOption");
    SYM(applyOptions, "SessionBindings_applyOptions");
    SYM(closeSession, "SessionBindings_closeSession");
    SYM(setScaleBufferSize, "SessionBindings_setScaleBufferSize");
    SYM(copyScaledRegion, "SessionBindings_copyScaledRegion");
    SYM(info[(int) Info::DesktopName], "ConnectionInfoBindings_getDesktopName");
    SYM(info[(int) Info::ProtoVersion], "ConnectionInfoBindings_getProtoVersion");
    SYM(info[(int) Info::ConnectionType], "ConnectionInfoBindings_getConnectionType");
    SYM(info[(int) Info::SecurityDesc], "ConnectionInfoBindings_getSecurityDesc");
    SYM(info[(int) Info::LastUsedEncoding], "ConnectionInfoBindings_getLastUsedEncoding");
    SYM(info[(int) Info::LineSpeed], "ConnectionInfoBindings_getLineSpeed");
    SYM(info[(int) Info::ServerPixelFormat], "ConnectionInfoBindings_getServerPixelFormat");
    SYM(info[(int) Info::ViewerPixelFormat], "ConnectionInfoBindings_getViewerPixelFormat");
    SYM(passwdResult, "PasswdDlgBindings_passwdResult");
    SYM(securityResult, "SecurityDlgBindings_securityResult");
    SYM(msgBoxResult, "MsgBoxBindings_msgBoxResult");
    SYM(interactiveResult, "InteractiveTextDlgBindings_promptResult");
    SYM(identityChosen, "AuthkeyChoiceDlgBindings_identityChosen");
#undef SYM

    // `RealVncCore.start`: the log directory is ours to make, the data one
    // the core makes under its root.
    mkdir(b.logDir.c_str(), 0700);
    JNIEnv *env = jenv_;
    natives_.initApp(env, nullptr, env->NewStringUTF(b.appName.c_str()),
                     env->NewStringUTF(b.version.c_str()), env->NewStringUTF(b.dataRoot.c_str()),
                     env->NewStringUTF(b.logDir.c_str()), env->NewStringUTF(b.logName.c_str()),
                     toJni(env_, context_), toJni(env_, makeMap(b.global)), b.fileLogging,
                     toJni(env_, makeLogMode(b.logMode)));
    if (env->ExceptionCheck()) {
        env->ExceptionDescribe();
        env->ExceptionClear();
        *why = "initApp threw";
        return false;
    }
    std::shared_ptr<jnivm::Object> noCloud[4];
    for (int i = 0; i < 4; i++) {
        noCloud[i] = makeNoCloud(i);
        keep_.push_back(noCloud[i]);
    }
    natives_.initVwr(env, nullptr, toJni(env_, noCloud[0]), toJni(env_, noCloud[1]),
                     toJni(env_, noCloud[2]), toJni(env_, noCloud[3]));
    if (env->ExceptionCheck()) {
        env->ExceptionDescribe();
        env->ExceptionClear();
        *why = "initVwr threw";
        return false;
    }

    // `RealVncPrompts.registerFactories`: one object of each, for the
    // process.
    const struct {
        const char *name;
        std::shared_ptr<jnivm::Object> object;
    } factories[] = {
            {"ReconnectorBindings_setDlgFactory", std::make_shared<ReconnectFactory>()},
            {"SecurityDlgBindings_setSecurityDlgFactory", std::make_shared<SecurityFactory>()},
            {"MsgBoxBindings_setMsgBoxFactory", std::make_shared<MsgBoxFactory>()},
            {"PasswdDlgBindings_setPasswdDlgFactory", std::make_shared<PasswdFactory>()},
            {"InteractiveTextDlgBindings_setInteractiveTextDlgFactory",
             std::make_shared<InteractiveFactory>()},
            {"AuthkeyChoiceDlgBindings_setAuthkeyChoiceDlgFactory",
             std::make_shared<AuthkeyChoiceFactory>()},
            {"StatusNotificationBindings_setStatusNotifier", std::make_shared<StatusNotifier>()},
            {"SecurityNotificationBindings_setSecurityNotifier", std::make_shared<SecurityNotifier>()},
            {"SaveCredentialsBindings_setCredentialsStore", std::make_shared<CredentialsStore>()},
            {"SaveIdentityBindings_setIdentityStore", std::make_shared<IdentityStore>()},
    };
    for (const auto &f : factories) {
        void (*set)(JNIEnv *, jclass, jobject);
        if (!symbol(core_, (std::string("Java_com_realvnc_vncviewer_jni_") + f.name).c_str(), &set,
                    why)) {
            return false;
        }
        keep_.push_back(f.object);
        set(env, nullptr, toJni(env_, f.object));
    }
    return true;
}

/* The classes of this file, hooked. Each dialog answers through the core's
   static result natives, called later from the core thread by the session
   (through `HostedCore`), never from inside the question. */
void DesktopCore::registerBindings() {
    auto env = vm_->GetEnv();
    auto *E = env.get();
    DesktopCore *core = this;

    env->GetClass<SessionHandle>("com/realvnc/vncviewer/jni/SessionBindings$Session");
    env->GetClass<SessionCallback>("com/realvnc/vncviewer/jni/SessionBindings$SessionCallback");
    env->GetClass<DesktopCallback>("com/realvnc/vncviewer/jni/SessionBindings$DesktopCallback");
    env->GetClass<ReconnectFactory>("com/realvnc/vncviewer/jni/ReconnectorBindings$DlgFactory");
    env->GetClass<ReconnectDlg>("com/realvnc/vncviewer/jni/ReconnectorBindings$ReconnectDlg");
    env->GetClass<SecurityFactory>("com/realvnc/vncviewer/jni/SecurityDlgBindings$SecurityDlgFactory");
    env->GetClass<SecurityDlg>("com/realvnc/vncviewer/jni/SecurityDlgBindings$SecurityDlg");
    env->GetClass<MsgBoxFactory>("com/realvnc/vncviewer/jni/MsgBoxBindings$MsgBoxFactory");
    env->GetClass<MsgBox>("com/realvnc/vncviewer/jni/MsgBoxBindings$MsgBox");
    env->GetClass<PasswdFactory>("com/realvnc/vncviewer/jni/PasswdDlgBindings$PasswdDlgFactory");
    env->GetClass<PasswdDlg>("com/realvnc/vncviewer/jni/PasswdDlgBindings$PasswdDlg");
    env->GetClass<InteractiveFactory>(
            "com/realvnc/vncviewer/jni/InteractiveTextDlgBindings$InteractiveTextDlgFactory");
    env->GetClass<InteractiveDlg>("com/realvnc/vncviewer/jni/InteractiveTextDlgBindings$InteractiveTextDlg");
    env->GetClass<AuthkeyChoiceFactory>(
            "com/realvnc/vncviewer/jni/AuthkeyChoiceDlgBindings$AuthkeyChoiceDlgFactory");
    env->GetClass<AuthkeyChoiceDlg>("com/realvnc/vncviewer/jni/AuthkeyChoiceDlgBindings$AuthkeyChoiceDlg");
    env->GetClass<StatusNotifier>("com/realvnc/vncviewer/jni/StatusNotificationBindings$StatusNotifier");
    env->GetClass<SecurityNotifier>(
            "com/realvnc/vncviewer/jni/SecurityNotificationBindings$SecurityNotifier");
    env->GetClass<CredentialsStore>("com/realvnc/vncviewer/jni/SaveCredentialsBindings$CredentialsStore");
    env->GetClass<IdentityStore>("com/realvnc/vncviewer/jni/SaveIdentityBindings$IdentityStore");

    // ---- Session: the handle, whose constructor the core calls as
    // `<init>(J)V` and whose token it reads back on every call ----
    {
        auto c = env->GetClass("com/realvnc/vncviewer/jni/SessionBindings$Session");
        c->Hook(E, "token", &SessionHandle::token);
        c->HookInstanceFunction(E, "getToken", [](jnivm::ENV *, jnivm::Object *self) -> jlong {
            if (auto *s = unwrap<SessionHandle>((jobject) self)) {
                return s->token;
            }
            LOGW("getToken on something that is not a Session");
            return 0;
        });
        c->HookInstanceFunction(E, "setToken", [](jnivm::ENV *, SessionHandle *s, jlong t) { s->token = t; });
        c->Hook(E, "<init>", [](jnivm::ENV *, jnivm::Class *, jlong t) {
            auto s = std::make_shared<SessionHandle>();
            s->token = t;
            return s;
        });
    }

    // ---- the two callbacks ----
    env->GetClass("com/realvnc/vncviewer/jni/SessionBindings$SessionCallback")
            ->HookInstanceFunction(E, "sessionClosed",
                                   [](jnivm::ENV *, SessionCallback *self, std::shared_ptr<SessionHandle>) {
                                       self->listener->sessionClosed();
                                   });
    {
        auto c = env->GetClass("com/realvnc/vncviewer/jni/SessionBindings$DesktopCallback");
        c->HookInstanceFunction(E, "bell", [](jnivm::ENV *, DesktopCallback *self) { self->listener->bell(); });
        c->HookInstanceFunction(E, "connSuccess",
                                [](jnivm::ENV *, DesktopCallback *self) { self->listener->connSuccess(); });
        c->HookInstanceFunction(E, "drawRegion", [](jnivm::ENV *, DesktopCallback *self, jint x, jint y, jint w, jint h) {
            self->listener->drawRegion(x, y, w, h);
        });
        c->HookInstanceFunction(E, "framebufferUpdateEnd",
                                [](jnivm::ENV *, DesktopCallback *self) { self->listener->framebufferUpdateEnd(); });
        c->HookInstanceFunction(E, "getClipboard",
                                [](jnivm::ENV *, DesktopCallback *self) -> std::shared_ptr<jnivm::String> {
                                    std::string text;
                                    return self->listener->getClipboard(&text) ? jstr(text) : nullptr;
                                });
        c->HookInstanceFunction(E, "setClipboard",
                                [](jnivm::ENV *, DesktopCallback *self, std::shared_ptr<jnivm::String> s) {
                                    self->listener->setClipboard(fromJstr(s));
                                });
        c->HookInstanceFunction(E, "setCursor",
                                [](jnivm::ENV *, DesktopCallback *self, std::shared_ptr<jnivm::Object> bm, jint hx,
                                   jint hy) {
                                    auto b = std::dynamic_pointer_cast<Bitmap>(bm);
                                    if (b) {
                                        self->listener->setCursor(b->pixels.data(), b->width, b->height, hx, hy);
                                    } else {
                                        self->listener->setCursor(nullptr, 0, 0, hx, hy);
                                    }
                                });
        c->HookInstanceFunction(E, "setDesktopSize", [](jnivm::ENV *, DesktopCallback *self, jint w, jint h) {
            self->listener->setDesktopSize(w, h);
        });
    }

    // ---- the reconnector's banner ----
    env->GetClass("com/realvnc/vncviewer/jni/ReconnectorBindings$DlgFactory")
            ->HookInstanceFunction(E, "createReconnectDlg",
                                   [core](jnivm::ENV *, ReconnectFactory *, std::shared_ptr<SessionHandle> s) {
                                       auto d = std::make_shared<ReconnectDlg>();
                                       d->listener = core->ownerOf((jobject) s.get());
                                       return d;
                                   });
    {
        auto c = env->GetClass("com/realvnc/vncviewer/jni/ReconnectorBindings$ReconnectDlg");
        c->HookInstanceFunction(E, "show",
                                [](jnivm::ENV *, ReconnectDlg *d, std::shared_ptr<jnivm::String> m,
                                   std::shared_ptr<jnivm::String> detail) {
                                    if (d->listener) d->listener->reconnecting(fromJstr(m), fromJstr(detail));
                                });
        c->HookInstanceFunction(E, "hide", [](jnivm::ENV *, ReconnectDlg *d) {
            if (d->listener) d->listener->reconnected();
        });
        c->HookInstanceFunction(E, "close", [](jnivm::ENV *, ReconnectDlg *) {});
    }

    // ---- the security prompt ----
    env->GetClass("com/realvnc/vncviewer/jni/SecurityDlgBindings$SecurityDlgFactory")
            ->HookInstanceFunction(E, "createSecurityDlg",
                                   [core](jnivm::ENV *, SecurityFactory *, std::shared_ptr<SessionHandle> s,
                                          jlong cookie) {
                                       auto d = std::make_shared<SecurityDlg>();
                                       d->listener = core->ownerOf((jobject) s.get());
                                       d->cookie = cookie;
                                       return d;
                                   });
    {
        auto c = env->GetClass("com/realvnc/vncviewer/jni/SecurityDlgBindings$SecurityDlg");
        c->HookInstanceFunction(
                E, "promptSecurity",
                [core](jnivm::ENV *, SecurityDlg *d, jint identityState, jint encryptionState,
                       std::shared_ptr<jnivm::String> signature, std::shared_ptr<jnivm::String> catchphrase,
                       std::shared_ptr<jnivm::String> name, std::shared_ptr<jnivm::String> matchingName,
                       std::shared_ptr<jnivm::String> /*hint*/) {
                    if (!d->listener) {
                        core->securityResult(d->cookie, false, false, false);
                        return;
                    }
                    d->listener->promptSecurity(d->cookie, identityState, encryptionState, fromJstr(signature),
                                                fromJstr(catchphrase), fromJstr(name), fromJstr(matchingName));
                });
        c->HookInstanceFunction(E, "close", [](jnivm::ENV *, SecurityDlg *) {});
    }

    // ---- the message box, whose question arrives at the factory ----
    env->GetClass("com/realvnc/vncviewer/jni/MsgBoxBindings$MsgBoxFactory")
            ->HookInstanceFunction(E, "createMsgBox",
                                   [core](jnivm::ENV *, MsgBoxFactory *, std::shared_ptr<SessionHandle> s, jlong cookie,
                                          std::shared_ptr<jnivm::String> message, jint flags,
                                          std::shared_ptr<jnivm::String> custom) {
                                       Listener *l = core->ownerOf((jobject) s.get());
                                       if (l) {
                                           l->msgBox(cookie, fromJstr(message), flags,
                                                     custom ? custom->c_str() : nullptr);
                                       } else {
                                           core->msgBoxResult(cookie, false);
                                       }
                                       return std::make_shared<MsgBox>();
                                   });
    env->GetClass("com/realvnc/vncviewer/jni/MsgBoxBindings$MsgBox")
            ->HookInstanceFunction(E, "close", [](jnivm::ENV *, MsgBox *) {});

    // ---- the password prompt ----
    env->GetClass("com/realvnc/vncviewer/jni/PasswdDlgBindings$PasswdDlgFactory")
            ->HookInstanceFunction(E, "createPasswdDlg",
                                   [core](jnivm::ENV *, PasswdFactory *, std::shared_ptr<SessionHandle> s, jlong cookie,
                                          std::shared_ptr<jnivm::String> /*address*/) {
                                       auto d = std::make_shared<PasswdDlg>();
                                       d->listener = core->ownerOf((jobject) s.get());
                                       d->cookie = cookie;
                                       return d;
                                   });
    {
        auto c = env->GetClass("com/realvnc/vncviewer/jni/PasswdDlgBindings$PasswdDlg");
        c->HookInstanceFunction(
                E, "getUserPasswd",
                [core](jnivm::ENV *, PasswdDlg *d, std::shared_ptr<jnivm::String> /*unused*/,
                       std::shared_ptr<jnivm::String> serverName, std::shared_ptr<jnivm::String> userName,
                       jboolean wantsPassword, std::shared_ptr<jnivm::String> catchphrase,
                       std::shared_ptr<jnivm::String> signature, std::shared_ptr<jnivm::String> instructions) {
                    if (!d->listener) {
                        core->passwdResult(d->cookie, false, nullptr, nullptr);
                        return;
                    }
                    const std::string user = fromJstr(userName);
                    d->listener->promptPasswd(d->cookie, fromJstr(serverName), userName ? user.c_str() : nullptr,
                                              wantsPassword, fromJstr(catchphrase), fromJstr(signature),
                                              fromJstr(instructions));
                });
        c->HookInstanceFunction(E, "close", [](jnivm::ENV *, PasswdDlg *) {});
    }

    // ---- the two declined without asking, as `RealVncPrompts` declines
    // them: interactive authentication needs a form, and choosing a client
    // identity needs identities nobody enrols ----
    env->GetClass("com/realvnc/vncviewer/jni/InteractiveTextDlgBindings$InteractiveTextDlgFactory")
            ->HookInstanceFunction(E, "createInteractiveTextDlg",
                                   [](jnivm::ENV *, InteractiveFactory *, std::shared_ptr<SessionHandle>, jlong cookie) {
                                       auto d = std::make_shared<InteractiveDlg>();
                                       d->cookie = cookie;
                                       return d;
                                   });
    {
        auto c = env->GetClass("com/realvnc/vncviewer/jni/InteractiveTextDlgBindings$InteractiveTextDlg");
        c->HookInstanceFunction(E, "promptInteractive",
                                [core](jnivm::ENV *, InteractiveDlg *d, std::shared_ptr<jnivm::Object>, jboolean) {
                                    LOGW("declining interactive prompt");
                                    core->natives().interactiveResult(core->env(), nullptr, d->cookie, false, nullptr);
                                });
        c->HookInstanceFunction(E, "close", [](jnivm::ENV *, InteractiveDlg *) {});
    }
    env->GetClass("com/realvnc/vncviewer/jni/AuthkeyChoiceDlgBindings$AuthkeyChoiceDlgFactory")
            ->HookInstanceFunction(E, "createAuthkeyChoiceDlg",
                                   [](jnivm::ENV *, AuthkeyChoiceFactory *, std::shared_ptr<SessionHandle>, jlong cookie) {
                                       auto d = std::make_shared<AuthkeyChoiceDlg>();
                                       d->cookie = cookie;
                                       return d;
                                   });
    {
        auto c = env->GetClass("com/realvnc/vncviewer/jni/AuthkeyChoiceDlgBindings$AuthkeyChoiceDlg");
        c->HookInstanceFunction(E, "show",
                                [core](jnivm::ENV *, AuthkeyChoiceDlg *d, std::shared_ptr<jnivm::Array<jnivm::String>>) {
                                    LOGW("declining identity choice");
                                    core->natives().identityChosen(core->env(), nullptr, d->cookie, false, nullptr);
                                });
        c->HookInstanceFunction(E, "close", [](jnivm::ENV *, AuthkeyChoiceDlg *) {});
    }

    // ---- notifications, not questions: the core has already acted ----
    env->GetClass("com/realvnc/vncviewer/jni/StatusNotificationBindings$StatusNotifier")
            ->HookInstanceFunction(E, "update",
                                   [core](jnivm::ENV *, StatusNotifier *, std::shared_ptr<SessionHandle> s,
                                          std::shared_ptr<jnivm::String> text) {
                                       if (Listener *l = core->ownerOf((jobject) s.get())) {
                                           l->status(fromJstr(text));
                                       }
                                   });
    env->GetClass("com/realvnc/vncviewer/jni/SecurityNotificationBindings$SecurityNotifier")
            ->HookInstanceFunction(E, "show",
                                   [core](jnivm::ENV *, SecurityNotifier *, std::shared_ptr<SessionHandle> s,
                                          std::shared_ptr<jnivm::String> text, std::shared_ptr<jnivm::String> background,
                                          jint flags) {
                                       if (Listener *l = core->ownerOf((jobject) s.get())) {
                                           l->securityBanner(fromJstr(text), fromJstr(background), flags);
                                       }
                                   });
    env->GetClass("com/realvnc/vncviewer/jni/SaveCredentialsBindings$CredentialsStore")
            ->HookInstanceFunction(E, "saveCredentials",
                                   [core](jnivm::ENV *, CredentialsStore *, std::shared_ptr<SessionHandle> s,
                                          std::shared_ptr<jnivm::String> user, std::shared_ptr<jnivm::String> password) {
                                       if (Listener *l = core->ownerOf((jobject) s.get())) {
                                           l->saveCredentials(fromJstr(user), fromJstr(password));
                                       }
                                   });
    env->GetClass("com/realvnc/vncviewer/jni/SaveIdentityBindings$IdentityStore")
            ->HookInstanceFunction(E, "saveIdentity",
                                   [core](jnivm::ENV *, IdentityStore *, std::shared_ptr<SessionHandle> s,
                                          std::shared_ptr<jnivm::String> identity) {
                                       if (Listener *l = core->ownerOf((jobject) s.get())) {
                                           l->saveIdentity(fromJstr(identity));
                                       }
                                   });
}

} // namespace

HostedCore *openHostedCore(const std::string &libraryDir, const Bootstrap &bootstrap, std::string *why) {
    if (theCore) {
        return theCore;
    }
    auto *core = new DesktopCore;
    if (!core->open(libraryDir, bootstrap, why)) {
        // Half a core is not worth keeping, and not safe to unload either;
        // the process keeps what was mapped.
        return nullptr;
    }
    theCore = core;
    return core;
}

} // namespace vento_realvnc
