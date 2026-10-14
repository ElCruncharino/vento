// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// The seam between the RealVNC session and whatever is hosting the viewer
// core: `realvnc_session.cxx` is written against this and nothing below it,
// and `backends/platform/desktop/realvnc` is the one implementation so far,
// which runs the Android library in a glibc process. It is the core's JNI
// surface (`realvnc/src/main/java/com/realvnc/vncviewer/jni`) said in C++,
// method for method where the session needs one, with the Java objects the
// core resolves by name turned into a listener the host calls.
//
// Threads are the core's rule, not this file's: every call on `HostedCore`
// and `HostedSession` is made from the thread that opened the core, and every
// listener call arrives on it, with the one documented exception —
// `setScaleBufferSize` and `copyScaledRegion` are called from whichever
// thread is drawing, under a lock the session holds (RealVncBackend's class
// comment says why the pair needs one). A prompt carries a cookie, and the
// answer is the matching `*Result` call later, from the core's thread.

#ifndef VENTO_REALVNC_HOSTED_CORE_H
#define VENTO_REALVNC_HOSTED_CORE_H

#include <cstdint>
#include <string>
#include <utility>
#include <vector>

namespace vento_realvnc {

/* The option map `createSession` and `initApp` take, in order. */
using Options = std::vector<std::pair<std::string, std::string>>;

/* What the core tells a session: `SessionBindings.DesktopCallback` and
   `SessionCallback`, the three prompt families and the notifications that
   `RealVncPrompts` bridges. Strings are UTF-8 and the host's; copy what
   outlives the call. */
class Listener {
public:
    virtual ~Listener() = default;

    virtual void connSuccess() = 0;
    virtual void setDesktopSize(int width, int height) = 0;
    virtual void drawRegion(int x, int y, int width, int height) = 0;
    virtual void framebufferUpdateEnd() = 0;
    /* `pixels` is the core's bitmap, `width * height` words of `R, G, B, A`
       in memory, or nullptr for none; the two ints are the negated hotspot,
       as `DesktopCallback.setCursor` documents. */
    virtual void setCursor(const uint32_t *pixels, int width, int height, int negHotX,
                           int negHotY) = 0;
    virtual void bell() = 0;
    /* Our clipboard, for the server: false for none. */
    virtual bool getClipboard(std::string *out) = 0;
    virtual void setClipboard(const std::string &text) = 0;
    /* The session ended, for any reason. The last call about it. */
    virtual void sessionClosed() = 0;

    /* `PasswdDlg.getUserPasswd`, with the arguments in the order that
       class's javadoc says they really are. `userName` is null for a scheme
       with no notion of one. Answered with `HostedCore::passwdResult`. */
    virtual void promptPasswd(int64_t cookie, const std::string &serverName,
                              const char *userName, bool wantsPassword,
                              const std::string &catchphrase, const std::string &signature,
                              const std::string &instructions) = 0;
    /* `SecurityDlg.promptSecurity`, identity first and encryption second, the
       states as `SecurityDlgBindings` numbers them. Answered with
       `HostedCore::securityResult`. */
    virtual void promptSecurity(int64_t cookie, int identityState, int encryptionState,
                                const std::string &signature, const std::string &catchphrase,
                                const std::string &name, const std::string &matchingName) = 0;
    /* `MsgBoxFactory.createMsgBox`, flags as `MsgBoxBindings` has them.
       Answered with `HostedCore::msgBoxResult`. */
    virtual void msgBox(int64_t cookie, const std::string &message, int flags,
                        const char *customButton) = 0;

    virtual void status(const std::string &text) = 0;
    virtual void reconnecting(const std::string &message, const std::string &detail) = 0;
    virtual void reconnected() = 0;
    virtual void saveCredentials(const std::string &userName, const std::string &password) = 0;
    virtual void saveIdentity(const std::string &identity) = 0;
    virtual void securityBanner(const std::string &text, const std::string &background,
                                int flags) = 0;
};

/* The eight strings `ConnectionInfoBindings` formats, in the order
   `vento/realvnc.h` lists them. */
enum class Info {
    DesktopName,
    ProtoVersion,
    ConnectionType,
    SecurityDesc,
    LastUsedEncoding,
    LineSpeed,
    ServerPixelFormat,
    ViewerPixelFormat,
};

/* One `SessionBindings.Session`, as the natives that take one. Never freed by
   the session file, and never `destroySession`ed by anybody: see
   `RealVncBackend.reap`. */
class HostedSession {
public:
    virtual ~HostedSession() = default;

    /* The native `AndroidSession*`, for the estimator walk. */
    virtual uintptr_t token() = 0;

    virtual void focusEvent(bool focused) = 0;
    virtual void pointerEvent(int x, int y, int buttonMask) = 0;
    virtual void keyDownEvent(int64_t keysym, int keycode) = 0;
    virtual void keyUpEvent(int keycode) = 0;
    virtual void setOption(const std::string &name, const std::string &value) = 0;
    virtual void applyOptions() = 0;
    virtual void closeSession() = 0;
    virtual std::string info(Info which) = 0;

    /* The drawing thread's pair. The buffer is the core's, `width * height
       * 4` bytes, valid until the next call; nullptr where the core refused. */
    virtual uint8_t *setScaleBufferSize(int width, int height) = 0;
    virtual bool copyScaledRegion(int x, int y, int width, int height, float scale) = 0;
};

/* The core, once per process. */
class HostedCore {
public:
    virtual ~HostedCore() = default;

    /* Pump the core's looper on this thread for at most `timeoutMs` (-1 for
       ever); returns when something was handled, the timeout passed, or
       `wake` was called. */
    virtual void pollOnce(int timeoutMs) = 0;
    /* From any thread: make the pump return. */
    virtual void wake() = 0;

    /* `SessionBindings.createSession`; nullptr where the core refused. The
       listener must outlive the core's interest in it, which is for ever. */
    virtual HostedSession *createSession(Listener *listener, const std::string &address,
                                         const char *password, const Options &options) = 0;

    virtual void passwdResult(int64_t cookie, bool ok, const char *userName,
                              const char *password) = 0;
    virtual void securityResult(int64_t cookie, bool accept, bool saveIdentity,
                                bool saveEncryption) = 0;
    virtual void msgBoxResult(int64_t cookie, bool ok) = 0;
};

/* `Bindings.initApp`'s parameters, decided by the session file the way
   `RealVncCore` decides them, since they are about this backend and not
   about where it runs. */
struct Bootstrap {
    std::string appName;   /* the parameter namespace, and the data directory's name */
    std::string version;
    std::string dataRoot;  /* `dataRoot/appName` is the core's own directory */
    std::string logDir;
    std::string logName;   /* the log file is `logName.log` */
    bool fileLogging;
    int logMode;           /* `Bindings.LogMode`'s ordinal */
    Options global;        /* parameters applied at init */
};

/* The platform's: load the two libraries out of `libraryDir`, which the
   caller has already verified, bring the core up as `bootstrap` says, and
   register the prompt factories. Called once, on the thread that becomes the
   core's; nullptr with `*why` set where the host could not. */
HostedCore *openHostedCore(const std::string &libraryDir, const Bootstrap &bootstrap,
                           std::string *why);

} // namespace vento_realvnc

#endif
