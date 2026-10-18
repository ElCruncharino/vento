// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! `vento`: one process that holds a session on any of the eight backends.
//!
//! [`main`] parses the command line into a [`Mode`] and dispatches. The
//! window is the default: nothing on the command line opens the connection
//! dialog empty, a backend and an address open it filled in — connecting to
//! somebody else's machine is a click, not a side effect of a typo — and
//! `--connect` opens straight into the session, which quits the window
//! when it is disconnected. `--list` prints the
//! backends and their options. Any of the probe's flags
//! (`--probe`, `--out`, `--click`, …) selects the headless mode instead — the
//! spirit of `backends/platform/linux/probe/probe.c`, which is what proved
//! each C session through the neutral API — which connects, prints every
//! event, answers the prompts from the flags, writes the framebuffer and a
//! thumbnail, prints the facts, and leaves with a status that is zero only
//! if it connected, stayed connected, wrote what `--out` asked for and
//! closed when told to. Input is sent and not confirmed, since no protocol
//! confirms it; what can be known — input asked for in view only, a
//! clipboard the backend says has nowhere to go — fails it.

fn main() {
    std::process::exit(host::run(std::env::args().skip(1).collect()));
}

mod host {
    use media::audio::Outputs as _;
    use std::cell::Cell;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use vento::backend::{Backend, Credentials, Monitor, thumbnail_step};
    use vento::backends::{self, Connect, Dirs, Kind, Platform};
    use vento::events::{Event, Mailbox, State};
    use vento::gui;
    use api::trust::{self, Decision};
    use vento::known_hosts::{self, KnownHosts};
    use vento::options::{self, Options, Type};
    use vento::ppm;

    /// What the command line resolved to.
    enum Mode {
        /// The backends and their options.
        List,
        /// A headless session, as the probe.
        Probe(Box<Probe>),
        /// The window: a session, or the dialog to make one.
        Gui(Box<gui::Launch>),
    }

    /// A probe's command line, parsed and checked.
    struct Probe {
        kind: &'static Kind,
        address: String,
        user: String,
        domain: String,
        password: Option<String>,
        options: Options,
        dirs: Dirs,
        seconds: u64,
        connect_timeout: u64,
        trust: bool,
        click: Option<(i32, i32)>,
        key: Option<u32>,
        clipboard: Option<String>,
        /// `--resize`: a desktop size to ask for, as the window's follower
        /// asks for one.
        resize: Option<(u32, u32)>,
        /// `--sound-cycle`: how often the far end's sound is turned off and
        /// on again, where the session's switch can ask it to.
        sound_cycle: Option<Duration>,
        /// The session's sound controls, for `--sound-cycle`, and when it
        /// next turns the switch.
        sound_controls: std::cell::RefCell<Option<Arc<media::audio::Controls>>>,
        sound_cycle_at: Cell<Option<Instant>>,
        /// The session's sound as last printed, so that each change is said
        /// on its own line.
        sound: Cell<vento::events::Sound>,
        /// `--layout`: screens to ask for, left to right.
        layout: Option<Vec<(u32, u32)>>,
        /// `--no-clipboard-in`'s opposite: whether what the far end copies is
        /// shown, which is the probe's way of taking it.
        clipboard_in: bool,
        /// `--hw-decode`: how video is decoded.
        hw_decode: backends::Hardware,
        /// `--audio-output`: where the far end's sound plays.
        audio_output: backends::AudioOutput,
        out: Option<PathBuf>,
        thumb_step: usize,
        verbose: bool,
        /// Whether `--password` has been given to a credentials prompt.
        /// It is given once: a second prompt is the far end saying it was
        /// wrong, and answering that with the same password again is a loop
        /// (SPICE dials afresh for every refused ticket, nearly two hundred
        /// times in five seconds) that ends as "no connection in time".
        answered: Cell<bool>,
        /// Whether a second prompt came, so the password was refused.
        refused: Cell<bool>,
    }

    const USAGE: &str = "\
usage: vento                                   the connection dialog
       vento [<backend> [<address>]] [options] the dialog, filled in
       vento <backend> <address> --connect ... a session in a window
       vento <backend> <address> --probe N ... a headless probe
       vento --list

  <backend>              one of the ids --list prints
  <address>              host, host:port, host:display, [literal]:port — the
                         backend decides what a number means
  --user NAME            the user name; for RDP, DOMAIN\\user or user@domain
  --domain NAME          the RDP domain, if not folded into --user
  --password TEXT        the password; without one the credentials prompt is
                         answered with a cancel (the probe gives it to the
                         first prompt only: a second means it was refused)
  -o KEY=VALUE           a backend option (repeatable); --list shows each
  --connect              connect at once instead of showing the dialog;
                         disconnecting that session quits (an ending the
                         far end caused shows the dialog with the reason)
  --no-clipboard-out     do not send locally copied text to the far end
  --no-clipboard-in      do not take what the far end copies onto the local
                         clipboard
  --no-grab-keyboard     leave the window manager's shortcuts (Alt+Tab, Super)
                         to this desktop rather than sending them on
  --software-cursor      draw the far end's cursor shape into the picture,
                         hiding this desktop's, instead of making it the
                         window's own pointer where this end owns the
                         pointer
  --hw-decode[=HOW]      decode video on the GPU: vaapi, auto (the same, VAAPI
                         then software; the default without =HOW) or off,
                         which is what it is without the flag; where the
                         device does not take a stream it is decoded in
                         software. RustDesk, TigerVNC's H.264 and FreeRDP's
                         H.264 use it (the dialog's Settings have all five,
                         and the panel switches them live)
  --audio-output=WHERE   play the far end's sound on pipewire (a stream of
                         its own; nothing where no daemon answers), alsa
                         (ALSA's default device) or auto, the default:
                         PipeWire where its daemon answers, ALSA otherwise.
                         VENTO_AUDIO_OUTPUT says the same where this is not
                         given
  --x11, --wayland       open the window on that display server (default:
                         Wayland where WAYLAND_DISPLAY is set, X11 otherwise)
  --library-dir DIR      where RealVNC's two libraries are (XDG data dir)
  --state-dir DIR        where backends keep what they leave behind (XDG state
                         dir); the known hosts are in $XDG_CONFIG_HOME/vento
  --verbose              let the backends' debug lines through

headless, any of which selects the probe instead of the window:
  --probe SECONDS        how long to watch after connecting (5)
  --connect-timeout N    how long to wait for the connection (30)
  --out FILE.ppm         write the framebuffer there, and FILE-thumb.ppm
  --thumb-step N         the thumbnail's scale, 1/N (8)
  --trust                answer yes to every trust, security and message prompt,
                         for this run: a certificate the known hosts match
                         needs no --trust, and none is ever pinned from here
  --click X,Y            left-click there once connected
  --key KEYSYM           press and release this X keysym: 0x61, 97 and 0141
                         are all 'a', as strtol reads them
  --clipboard TEXT       send this as the clipboard a second after connecting
  --sound-cycle SECONDS  turn the far end's sound off, and on again, every
                         SECONDS, where the session's switch asks the far
                         end (QEMU's audio, SPICE, RustDesk)
  --resize WxH           ask for a desktop this size a second after connecting
  --layout WxH,WxH,...   ask for these screens side by side instead, the far
                         end's own screens by position and new ones past them";

    pub fn run(args: Vec<String>) -> i32 {
        vento::log::init(args.iter().any(|a| a == "--verbose"));
        match parse(args) {
            Ok(Mode::List) => {
                list();
                0
            }
            Ok(Mode::Probe(probe)) => probe.run(),
            Ok(Mode::Gui(launch)) => gui::run(*launch),
            Err(message) => {
                eprintln!("{message}\n\n{USAGE}");
                2
            }
        }
    }

    /// The backends and, under each, its options as `-o` takes them.
    fn list() {
        for kind in backends::all() {
            println!("{}  {} — {}", kind.id, kind.name, (kind.version)());
            println!("    {}", kind.description);
            for spec in kind.options {
                let type_note = match spec.ty {
                    Type::Bool => "true|false".to_string(),
                    Type::Int => "N".to_string(),
                    Type::Text => "text".to_string(),
                    Type::Choice => spec
                        .choices
                        .iter()
                        .map(|c| c.value)
                        .collect::<Vec<_>>()
                        .join("|"),
                };
                let gate = spec
                    .gate
                    .map(|g| format!(" (only when {}={})", g.key, g.values.join("/")))
                    .unwrap_or_default();
                println!(
                    "    -o {}={}  [{}]{} — {}",
                    spec.key,
                    spec.default,
                    type_note,
                    gate,
                    spec.summary_on(Platform::Linux).unwrap_or(spec.label)
                );
            }
            println!();
        }
    }

    fn parse(args: Vec<String>) -> Result<Mode, String> {
        if args.iter().any(|a| a == "--list") {
            return Ok(Mode::List);
        }
        let mut positional = Vec::new();
        let mut user = String::new();
        let mut domain = String::new();
        let mut password = None;
        let mut assignments: Vec<String> = Vec::new();
        let mut seconds = 5u64;
        let mut connect_timeout = 30u64;
        let mut trust = false;
        let mut click = None;
        let mut key = None;
        let mut clipboard = None;
        let mut resize = None;
        let mut sound_cycle = None;
        let mut layout = None;
        let mut out = None;
        let mut thumb_step = 8usize;
        let mut dirs = Dirs::xdg();
        let mut settings = gui::Settings::default();
        // Any of the probe's own flags asks for the probe; without one, the
        // window.
        let mut headless = false;
        let mut connect = false;
        // The window's display server, if named; the probe has no window and
        // takes either without a word.
        let mut display_server = None;

        let mut it = args.into_iter().peekable();
        let value = |it: &mut std::iter::Peekable<std::vec::IntoIter<String>>, flag: &str| {
            it.next().ok_or_else(|| format!("{flag} needs a value"))
        };
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--verbose" => {}
                "--user" => user = value(&mut it, "--user")?,
                "--domain" => domain = value(&mut it, "--domain")?,
                "--password" => password = Some(value(&mut it, "--password")?),
                "-o" => assignments.push(value(&mut it, "-o")?),
                "--connect" => connect = true,
                "--no-clipboard-out" => settings.clipboard_out = false,
                "--no-clipboard-in" => settings.clipboard_in = false,
                "--no-grab-keyboard" => settings.grab_keyboard = false,
                "--software-cursor" => settings.software_cursor = true,
                "--hw-decode" => settings.hw_decode = backends::Hardware::Auto,
                _ if arg.starts_with("--hw-decode=") => settings.hw_decode = hw_decode(&arg)?,
                _ if arg.starts_with("--audio-output=") => settings.audio_output = audio_output(&arg)?,
                "--x11" | "--wayland" => {
                    let named = if arg == "--x11" { gui::DisplayServer::X11 } else { gui::DisplayServer::Wayland };
                    if display_server.is_some_and(|p| p != named) {
                        return Err("--x11 and --wayland are each other's opposite".into());
                    }
                    display_server = Some(named);
                }
                "--probe" => {
                    headless = true;
                    seconds = number::<u64>(&value(&mut it, "--probe")?, "--probe")?;
                }
                "--connect-timeout" => {
                    headless = true;
                    connect_timeout = number::<u64>(&value(&mut it, "--connect-timeout")?, "--connect-timeout")?
                }
                "--out" => {
                    headless = true;
                    out = Some(PathBuf::from(value(&mut it, "--out")?));
                }
                "--thumb-step" => {
                    headless = true;
                    thumb_step = number::<usize>(&value(&mut it, "--thumb-step")?, "--thumb-step")?.max(1)
                }
                "--trust" => {
                    headless = true;
                    trust = true;
                }
                "--click" => {
                    headless = true;
                    let v = value(&mut it, "--click")?;
                    let (x, y) = v.split_once(',').ok_or("--click: not X,Y")?;
                    click = Some((
                        number::<i32>(x.trim(), "--click")?,
                        number::<i32>(y.trim(), "--click")?,
                    ));
                }
                "--key" => {
                    headless = true;
                    key = Some(keysym(&value(&mut it, "--key")?)?);
                }
                "--clipboard" => {
                    headless = true;
                    clipboard = Some(value(&mut it, "--clipboard")?);
                }
                "--sound-cycle" => {
                    headless = true;
                    let v = value(&mut it, "--sound-cycle")?;
                    let seconds: u64 = v.parse().ok().filter(|&n| n > 0).ok_or_else(|| format!("--sound-cycle {v:?}"))?;
                    sound_cycle = Some(Duration::from_secs(seconds));
                }
                "--resize" => {
                    headless = true;
                    resize = Some(dimensions(&value(&mut it, "--resize")?, "--resize")?);
                }
                "--layout" => {
                    headless = true;
                    let v = value(&mut it, "--layout")?;
                    layout = Some(
                        v.split(',')
                            .map(|s| dimensions(s.trim(), "--layout"))
                            .collect::<Result<Vec<_>, _>>()?,
                    );
                }
                "--library-dir" => dirs.realvnc_libs = PathBuf::from(value(&mut it, "--library-dir")?),
                "--state-dir" => dirs.state = PathBuf::from(value(&mut it, "--state-dir")?),
                other if other.starts_with('-') && other != "-" => {
                    return Err(format!("unknown option {other:?}"));
                }
                _ => positional.push(arg),
            }
        }

        if positional.is_empty() && !headless && !connect && assignments.is_empty() {
            return Ok(Mode::Gui(Box::new(gui::Launch {
                request: None,
                connect: false,
                dirs,
                settings,
                display_server,
            })));
        }
        // The dialog can be filled in from a backend alone; a session needs
        // somewhere to go.
        let whole = headless || connect;
        if positional.is_empty() || positional.len() > 2 || (whole && positional.len() != 2) {
            return Err("give a backend and an address".into());
        }
        let kind = backends::find(&positional[0])
            .ok_or_else(|| format!("no backend {:?}; --list shows them", positional[0]))?;

        let mut options = Options::defaults(kind.options);
        for assignment in &assignments {
            let (k, v) = options::parse_assignment(assignment)?;
            options::find(kind.options, k)?.check(v)?;
            options.set(k, v);
        }
        options.check_requirements(kind.options)?;

        if !headless {
            return Ok(Mode::Gui(Box::new(gui::Launch {
                request: Some(gui::Request {
                    kind,
                    address: positional.get(1).cloned().unwrap_or_default(),
                    user,
                    domain,
                    password: password.filter(|p| !p.is_empty()),
                    options,
                }),
                connect,
                dirs,
                settings,
                display_server,
            })));
        }

        // The window's clipboard settings mean the same here: what the far
        // end copies is not shown under --no-clipboard-in, and --clipboard is
        // text to send, which --no-clipboard-out says not to.
        if clipboard.is_some() && !settings.clipboard_out {
            return Err("--clipboard sends text to the far end, which --no-clipboard-out says not to".into());
        }
        // Nor can input be asked for in view only, where every backend drops
        // it without a word: the probe would report a click it never sent.
        if options.bool("ViewOnly") && (click.is_some() || key.is_some() || clipboard.is_some()) {
            return Err("-o ViewOnly=true sends no input or clipboard, so --click, --key and --clipboard cannot happen".into());
        }
        if resize.is_some() && layout.is_some() {
            return Err("--resize and --layout are two answers to one question; give one".into());
        }
        Ok(Mode::Probe(Box::new(Probe {
            kind,
            address: positional.remove(1),
            user,
            domain,
            password,
            options,
            dirs,
            seconds,
            connect_timeout,
            trust,
            click,
            key,
            clipboard,
            resize,
            sound_cycle,
            sound_controls: std::cell::RefCell::new(None),
            sound_cycle_at: Cell::new(None),
            sound: Cell::new(Default::default()),
            layout,
            clipboard_in: settings.clipboard_in,
            hw_decode: settings.hw_decode,
            audio_output: settings.audio_output,
            out,
            thumb_step,
            verbose: false,
            answered: Cell::new(false),
            refused: Cell::new(false),
        })))
    }

    /// A screen as the probe prints it: `id=N WxH+X+Y`, as heads.sh does.
    fn screen(m: &Monitor) -> String {
        format!("id={} {}x{}+{}+{}", m.id, m.width, m.height, m.x, m.y)
    }

    /// `--hw-decode=HOW`'s answer.
    fn hw_decode(arg: &str) -> Result<backends::Hardware, String> {
        let how = arg.split_once('=').map_or("", |(_, how)| how);
        backends::Hardware::from_name(how)
            .ok_or_else(|| format!("--hw-decode: {how:?} is not one of vaapi, auto or off"))
    }

    /// `--audio-output=WHERE`'s answer.
    fn audio_output(arg: &str) -> Result<backends::AudioOutput, String> {
        let name = arg.split_once('=').map_or("", |(_, name)| name);
        backends::AudioOutput::from_name(name)
            .ok_or_else(|| format!("--audio-output: {name:?} is not one of pipewire, alsa or auto"))
    }

    /// `WxH`, neither of them zero.
    fn dimensions(value: &str, flag: &str) -> Result<(u32, u32), String> {
        let (w, h) = value.split_once('x').ok_or(format!("{flag}: not WxH: {value:?}"))?;
        let (w, h) = (number::<u32>(w, flag)?, number::<u32>(h, flag)?);
        if w == 0 || h == 0 {
            return Err(format!("{flag}: a screen has an area: {value:?}"));
        }
        Ok((w, h))
    }

    fn number<T: std::str::FromStr>(value: &str, flag: &str) -> Result<T, String> {
        value.trim().parse().map_err(|_| format!("{flag}: {value:?} is not a number"))
    }

    /// A keysym as probe.c's `strtol(.., 0)` reads one, so that the same
    /// `--key` means the same key to both probes: hex after `0x` or `0X`
    /// (`0x61`), octal after any other leading zero (`0141`), decimal
    /// otherwise (`97`), after leading white space and an optional `+`, with
    /// nothing after the digits. `0o` and `0b` are not prefixes to strtol
    /// (without C23's), so they are not here either. What does not fit a
    /// keysym's 32 bits, or is negative, is refused rather than wrapped.
    fn keysym(value: &str) -> Result<u32, String> {
        let wrong = || format!("--key: {value:?} is not a keysym");
        // C's isspace, which is not Unicode's.
        let v = value.trim_start_matches([' ', '\t', '\n', '\x0b', '\x0c', '\r']);
        let v = v.strip_prefix('+').unwrap_or(v);
        let (digits, radix) = if let Some(hex) = v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
            (hex, 16)
        } else if v.len() > 1 && v.starts_with('0') {
            (&v[1..], 8)
        } else {
            (v, 10)
        };
        // from_str_radix takes a sign of its own, which strtol does not after
        // a prefix; so the digits are checked first.
        if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
            return Err(wrong());
        }
        u32::from_str_radix(digits, radix).map_err(|_| wrong())
    }

    /// At most `bytes` of `text`, cut where a character starts: the far end's
    /// clipboard is anybody's text, and a cut through the middle of a
    /// character is a panic rather than a shorter line.
    fn head(text: &str, bytes: usize) -> &str {
        &text[..text.floor_char_boundary(bytes)]
    }

    impl Probe {
        fn run(self: Box<Self>) -> i32 {
            let _ = self.verbose;
            let start = Instant::now();
            let say = |event: &str| println!("[{:8.3}] {event}", start.elapsed().as_secs_f64());

            println!("{}", (self.kind.version)());
            let mailbox = Mailbox::new(None);
            let backend = {
                let client_name = backends::client_name();
                let identity = known_hosts::realvnc_identity(&self.dirs.config, &self.address);
                let connect = Connect {
                    address: &self.address,
                    user: &self.user,
                    domain: &self.domain,
                    password: self.password.as_deref(),
                    options: &self.options,
                    mailbox: Arc::clone(&mailbox),
                    dirs: &self.dirs,
                    screen: None,
                    // The desktop keeps no note of a desktop's last size, so
                    // `DesktopSize=remote` is the fallback here.
                    last_size: None,
                    identity: identity.as_deref(),
                    client_name: &client_name,
                    platform: Platform::Linux,
                    decoders: Some(backends::decoders(self.hw_decode)),
                    audio: Some({
                        let audio = backends::audio(&self.address, self.audio_output);
                        *self.sound_controls.borrow_mut() = Some(Arc::clone(audio.controls()));
                        audio
                    }),
                };
                say("create");
                match self.kind.start(&connect) {
                    Ok(backend) => backend,
                    Err(why) => {
                        say(&format!("create failed: {why}"));
                        return 1;
                    }
                }
            };

            let mut status = 0;
            // Drain and print until connected, or the connect deadline.
            let connected = self.pump(&mailbox, &backend, &say, self.connect_timeout, |m| {
                m.state() != State::Connecting
            });
            if mailbox.state() != State::Connected {
                say(if self.refused.get() {
                    "authentication failed: the password was refused"
                } else if connected {
                    "ended before connecting"
                } else {
                    "no connection in time"
                });
                status = 1;
            } else {
                // A second for the first frame, so a click lands on a drawn
                // desktop; then the rest of the time asked for.
                self.pump(&mailbox, &backend, &say, 1, |_| false);
                if mailbox.state() == State::Connected {
                    self.poke(&backend, &say);
                    self.pump(&mailbox, &backend, &say, self.seconds.saturating_sub(1), |_| false);
                }
                if mailbox.state() != State::Connected {
                    say(if self.refused.get() {
                        "ended before the time was up: the password was refused"
                    } else {
                        "ended before the time was up"
                    });
                    status = 1;
                } else {
                    self.facts(&backend, &say);
                    if self.out.is_some() && !self.write_pictures(&backend, &say) {
                        status = 1;
                    }
                    // Asked at the end rather than when it was sent, since an
                    // agent that comes up late still gets the offer.
                    if self.clipboard.is_some() && backend.carries_clipboard() == Some(false) {
                        say("clipboard not carried: the far end has nowhere to put it");
                        status = 1;
                    }
                }
            }

            say("disconnect");
            backend.disconnect();
            if !self.pump(&mailbox, &backend, &say, 10, |m| m.state() == State::Closed) {
                say("no close within 10 s of disconnect");
                status = 1;
            }
            say("destroy");
            backend.destroy();
            say("done");
            status
        }

        /// Prints events until `done` is true, the session closes, or
        /// `seconds` pass; whether `done` was reached. Prompts are answered
        /// from the flags as they arrive.
        fn pump(
            &self,
            mailbox: &Mailbox,
            backend: &Arc<dyn Backend>,
            say: &dyn Fn(&str),
            seconds: u64,
            done: impl Fn(&Mailbox) -> bool,
        ) -> bool {
            let deadline = Instant::now() + Duration::from_secs(seconds);
            loop {
                while let Some(event) = mailbox.take() {
                    self.print(&event, say);
                    self.answer(&event, backend, say);
                    if let Event::Closed { .. } = event {
                        return done(mailbox);
                    }
                }
                if done(mailbox) {
                    return true;
                }
                let now = Instant::now();
                if now >= deadline {
                    return done(mailbox);
                }
                self.cycle_sound(now, say);
                mailbox.wait((deadline - now).min(Duration::from_millis(200)));
            }
        }

        /// `--sound-cycle`: turns the switch once it is due, where the far
        /// end can be asked; the first turn is a period after the switch is
        /// offered.
        fn cycle_sound(&self, now: Instant, say: &dyn Fn(&str)) {
            let (Some(every), Some(controls)) = (self.sound_cycle, self.sound_controls.borrow().clone()) else {
                return;
            };
            if controls.sound().switch != Some(media::audio::Switch::Enable) {
                self.sound_cycle_at.set(None);
                return;
            }
            match self.sound_cycle_at.get() {
                None => self.sound_cycle_at.set(Some(now + every)),
                Some(at) if now >= at => {
                    let on = !controls.enabled();
                    say(if on { "turning the far end's sound on" } else { "turning the far end's sound off" });
                    controls.set_enabled(on);
                    self.sound_cycle_at.set(Some(now + every));
                }
                Some(_) => {}
            }
        }

        fn print(&self, event: &Event, say: &dyn Fn(&str)) {
            match event {
                Event::Connected { width, height } => say(&format!("connected {width}x{height}")),
                Event::DesktopSize { width, height } => say(&format!("desktop_size {width}x{height}")),
                Event::Frame { rects, .. } => say(&format!("frame_end ({} rectangles)", rects.len())),
                Event::Cursor(None) => say("cursor default"),
                Event::Cursor(Some(c)) => {
                    let opaque = c.rgba.chunks_exact(4).filter(|p| p[3] != 0).count();
                    say(&format!(
                        "cursor {}x{} hot {},{}, {opaque} of {} pixels visible",
                        c.width,
                        c.height,
                        c.hot_x,
                        c.hot_y,
                        c.width * c.height
                    ));
                }
                Event::PointerMode { relative } => {
                    say(&format!("pointer_mode {}", if *relative { "relative" } else { "absolute" }))
                }
                Event::Bell => say("bell"),
                Event::Sound(sound) => {
                    let before = self.sound.replace(*sound);
                    if (sound.switch, sound.enabled, sound.muted) != (before.switch, before.enabled, before.muted) {
                        say(&match sound.switch {
                            None => "sound switch: none".to_string(),
                            Some(switch) => format!(
                                "sound switch: {} {}",
                                switch.label(),
                                if sound.on() { "on" } else { "off" }
                            ),
                        });
                    }
                    if sound.playing != before.playing {
                        say(if sound.playing { "audio playing" } else { "audio stopped" });
                    }
                }
                Event::Clipboard(text) if !self.clipboard_in => {
                    say(&format!("clipboard {} bytes, not taken (--no-clipboard-in)", text.len()))
                }
                Event::Clipboard(text) => say(&format!(
                    "clipboard {} bytes: {:?}{}",
                    text.len(),
                    head(text, 60),
                    if text.len() > 60 { "..." } else { "" }
                )),
                Event::CredentialsNeeded { needs_user } => {
                    say(&format!("credentials_needed (user {})", if *needs_user { "wanted" } else { "not wanted" }))
                }
                Event::TrustNeeded { fingerprint } => say(&format!("trust_needed {fingerprint}")),
                Event::Unverified { why } => say(&format!("unverified: {why}")),
                Event::Question { title, text } => say(&format!("question {title:?}: {text}")),
                Event::SecurityNeeded {
                    identity,
                    encryption,
                    signature,
                    catchphrase,
                    matching_address,
                } => say(&format!(
                    "security_needed {identity:?}, {encryption:?}, {signature:?} ({catchphrase}){}",
                    if matching_address.is_empty() {
                        String::new()
                    } else {
                        format!(", known as {matching_address}")
                    }
                )),
                Event::Message {
                    text,
                    severity,
                    question,
                    confirm_label,
                } => say(&format!(
                    "message ({severity:?}) {text:?}{} {}",
                    if confirm_label.is_empty() {
                        String::new()
                    } else {
                        format!(" [{confirm_label}]")
                    },
                    if *question { "(question)" } else { "(ok)" }
                )),
                Event::Closed { detail } => {
                    say(&format!("closed{}{}", if detail.is_empty() { "" } else { ": " }, detail))
                }
            }
        }

        /// Answers a prompt from the flags, as probe.c does: the password
        /// once, then a cancel, and `--trust` to everything else.
        fn answer(&self, event: &Event, backend: &Arc<dyn Backend>, say: &dyn Fn(&str)) {
            match event {
                Event::CredentialsNeeded { .. } => match &self.password {
                    Some(_) if self.answered.get() => {
                        say("  -> asked again, so the password was refused; cancelling");
                        self.refused.set(true);
                        backend.answer_credentials(None);
                    }
                    Some(password) => {
                        self.answered.set(true);
                        say("  -> answering with the password");
                        backend.answer_credentials(Some(Credentials {
                            user: self.user.clone(),
                            domain: self.domain.clone(),
                            password: password.clone(),
                        }));
                    }
                    None => {
                        say("  -> no --password, cancelling");
                        backend.answer_credentials(None);
                    }
                },
                Event::TrustNeeded { fingerprint } => {
                    // The known hosts first, as the window has them: read,
                    // and never written — `--trust` is for this run.
                    let pinned = backends::pinned_as(self.kind, &self.address, &self.options);
                    let hosts = KnownHosts::in_dir(&self.dirs.config);
                    let (yes, why) = match trust::decide(&trust::ReadOnly(&hosts), &pinned, fingerprint) {
                        Decision::Matches => (true, format!("matches the pin for {}", pinned.key)),
                        Decision::FirstSight => {
                            (true, format!("first sight of {}, taken without asking (not pinned)", pinned.key))
                        }
                        Decision::Ask {
                            identity,
                            matching_address,
                        } => (
                            self.trust,
                            format!(
                                "{identity:?}{}, {}",
                                if matching_address.is_empty() {
                                    String::new()
                                } else {
                                    format!(" (pinned for {matching_address})")
                                },
                                if self.trust { "yes (--trust, not pinned)" } else { "no" }
                            ),
                        ),
                    };
                    say(&format!("  -> {why}"));
                    backend.answer_trust(yes);
                }
                Event::Unverified { .. } | Event::Question { .. } => {
                    say(&format!("  -> {}", if self.trust { "yes (--trust)" } else { "no" }));
                    backend.answer_trust(self.trust);
                }
                Event::SecurityNeeded { .. } => {
                    say(&format!("  -> {}", if self.trust { "yes (--trust)" } else { "no" }));
                    backend.answer_security(self.trust, false);
                }
                Event::Message { question, .. } => {
                    // A box with one button has one answer; a question gets
                    // --trust's.
                    let yes = !question || self.trust;
                    say(&format!("  -> {}", if yes { "ok/yes" } else { "no" }));
                    backend.answer_trust(yes);
                }
                _ => {}
            }
        }

        fn poke(&self, backend: &Arc<dyn Backend>, say: &dyn Fn(&str)) {
            if let Some((x, y)) = self.click {
                say(&format!("pointer -> click at {x},{y}"));
                backend.pointer(x, y, 0);
                backend.pointer(x, y, 1);
                backend.pointer(x, y, 0);
            }
            if let Some(text) = &self.clipboard {
                say(&format!("clipboard -> {text:?}"));
                backend.clipboard(text);
            }
            if let Some(keysym) = self.key {
                say(&format!("key_down/key_up -> keysym {keysym:#x}"));
                backend.key_down(keysym, 1);
                backend.key_up(1);
            }
            if let Some((w, h)) = self.resize {
                say(&format!("request_desktop_size -> {w}x{h}"));
                backend.request_desktop_size(w, h);
            }
            if let Some(sizes) = &self.layout {
                // The far end's screens keep their ids by position, as a
                // window placing its monitors left to right would hand them
                // back; one past them has an id the far end cannot have yet
                // (none has all 2^32), and the backend numbers it.
                let have = backend.facts().monitors;
                let fresh = (0..=u32::MAX).rev().find(|id| have.iter().all(|m| m.id != *id));
                let mut x = 0i32;
                let screens: Vec<Monitor> = sizes
                    .iter()
                    .enumerate()
                    .map(|(i, &(w, h))| {
                        let id = have.get(i).map(|m| m.id).or(fresh).unwrap_or(0);
                        let m = Monitor {
                            id,
                            x,
                            y: 0,
                            width: w as i32,
                            height: h as i32,
                        };
                        x += w as i32;
                        m
                    })
                    .collect();
                let width = x.max(0) as u32;
                let height = sizes.iter().map(|s| s.1).max().unwrap_or(0);
                say(&format!(
                    "request_layout -> {width}x{height}: {}",
                    screens.iter().map(screen).collect::<Vec<_>>().join(", ")
                ));
                backend.request_layout(width, height, &screens);
            }
        }

        fn facts(&self, backend: &Arc<dyn Backend>, say: &dyn Fn(&str)) {
            let facts = backend.facts();
            say(&format!(
                "facts {}x{}, {} monitors, {} displays (on {}){}{}",
                facts.width,
                facts.height,
                facts.monitors.len(),
                facts.displays.len(),
                facts.display,
                if facts.can_resize { ", resizable" } else { "" },
                if facts.pointer_relative { ", relative pointer" } else { "" }
            ));
            if facts.can_layout {
                say("facts: takes a layout");
            }
            for m in &facts.monitors {
                say(&format!("monitor {}", screen(m)));
            }
            for (i, fact) in backend.info().iter().enumerate() {
                say(&format!(
                    "info[{i}] {}{}: {}",
                    fact.label,
                    if fact.diagnostic { " (diag)" } else { "" },
                    fact.value
                ));
            }
            // The overlay's line, which says why a picture asked for in
            // hardware is in software.
            if let Some(status) = backend.decoder_status() {
                say(&format!("decoder {status}"));
            }
            match backend.traffic() {
                Some(t) => say(&format!(
                    "traffic {} in{}",
                    t.received,
                    t.sent.map(|s| format!(", {s} out")).unwrap_or_default()
                )),
                None => say("traffic: none yet"),
            }
        }

        fn write_pictures(&self, backend: &Arc<dyn Backend>, say: &dyn Fn(&str)) -> bool {
            let Some(out) = &self.out else {
                return true;
            };
            let facts = backend.facts();
            let (width, height) = (facts.width, facts.height);
            if width == 0 || height == 0 {
                say("no framebuffer size to read");
                return false;
            }
            let mut ok = true;
            let mut rgba = vec![0u8; width * height * 4];
            if backend.read_region(0, 0, width, height, &mut rgba, width * 4, 0, 0) {
                let picture = vento::backend::Picture { width, height, rgba };
                match ppm::write(out, &picture) {
                    Ok(()) => say(&format!("read_region {width}x{height} -> {}", out.display())),
                    Err(e) => {
                        say(&format!("writing {}: {e}", out.display()));
                        ok = false;
                    }
                }
            } else {
                say(&format!("read_region {width}x{height} refused"));
                ok = false;
            }

            let step = self.thumb_step;
            let thumb_path = ppm::thumbnail_path(out);
            match backend.thumbnail(step) {
                Some(picture) => match ppm::write(&thumb_path, &picture) {
                    Ok(()) => say(&format!(
                        "read_thumbnail 1/{step} {}x{} -> {}",
                        picture.width,
                        picture.height,
                        thumb_path.display()
                    )),
                    Err(e) => {
                        say(&format!("writing {}: {e}", thumb_path.display()));
                        ok = false;
                    }
                },
                None => {
                    say(&format!("read_thumbnail 1/{step} refused"));
                    ok = false;
                }
            }
            let _ = thumbnail_step;
            ok
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_keysym_is_read_as_strtol_with_base_zero_reads_it() {
            assert_eq!(keysym("0x61"), Ok(0x61));
            assert_eq!(keysym("0X61"), Ok(0x61));
            assert_eq!(keysym("97"), Ok(0x61));
            assert_eq!(keysym("0141"), Ok(0x61));
            assert_eq!(keysym("0"), Ok(0));
            assert_eq!(keysym("00"), Ok(0));
            assert_eq!(keysym(" \t+0xff0d"), Ok(0xff0d));
            assert_eq!(keysym("0xffffffff"), Ok(u32::MAX));
            for refused in [
                "0o141", "0b101", "08", "0x", "", " ", "0x+61", "0x-61", "-1", "97 ", "1e3", "0x1ffffffff", "++1",
            ] {
                assert!(keysym(refused).is_err(), "{refused:?}");
            }
        }

        #[test]
        fn hardware_decoding_is_off_unless_asked_and_auto_when_bare() {
            let settings = |args: &[&str]| match parse(args.iter().map(|a| a.to_string()).collect()) {
                Ok(Mode::Gui(launch)) => launch.settings.hw_decode,
                Ok(Mode::Probe(probe)) => probe.hw_decode,
                _ => panic!("{args:?} did not parse"),
            };
            use backends::Hardware;
            assert_eq!(settings(&[]), Hardware::Off);
            assert_eq!(settings(&["--hw-decode"]), Hardware::Auto);
            assert_eq!(settings(&["--hw-decode=vaapi"]), Hardware::Vaapi);
            assert_eq!(settings(&["--hw-decode=auto"]), Hardware::Auto);
            assert_eq!(settings(&["--hw-decode", "--hw-decode=off"]), Hardware::Off);
            assert_eq!(
                settings(&["rustdesk", "127.0.0.1", "--probe", "1", "--hw-decode=vaapi"]),
                Hardware::Vaapi
            );
            for refused in ["--hw-decode=", "--hw-decode=VAAPI", "--hw-decode=nvdec", "--hw-decode=vulkan"] {
                assert!(parse(vec![refused.to_string()]).is_err(), "{refused}");
            }
        }

        #[test]
        fn the_sound_plays_where_it_is_told_and_auto_otherwise() {
            let output = |args: &[&str]| match parse(args.iter().map(|a| a.to_string()).collect()) {
                Ok(Mode::Gui(launch)) => launch.settings.audio_output,
                Ok(Mode::Probe(probe)) => probe.audio_output,
                _ => panic!("{args:?} did not parse"),
            };
            use backends::AudioOutput;
            assert_eq!(output(&[]), AudioOutput::Auto);
            assert_eq!(output(&["--audio-output=alsa"]), AudioOutput::Alsa);
            assert_eq!(output(&["--audio-output=pipewire"]), AudioOutput::PipeWire);
            assert_eq!(
                output(&["tigervnc", "127.0.0.1", "--probe", "1", "--audio-output=alsa", "--audio-output=auto"]),
                AudioOutput::Auto
            );
            for refused in ["--audio-output=", "--audio-output=pulse", "--audio-output=PipeWire", "--audio-output"] {
                assert!(parse(vec![refused.to_string()]).is_err(), "{refused}");
            }
        }

        #[test]
        fn a_clipboard_line_is_cut_between_characters() {
            let text = format!("{}é tail", "0".repeat(59));
            assert_eq!(head(&text, 60), "0".repeat(59));
            assert_eq!(head(&text, 61), format!("{}é", "0".repeat(59)));
            assert_eq!(head("short", 60), "short");
            assert_eq!(head("", 60), "");
        }
    }
}
