// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The command line's words: the options [`split`] takes apart, GNU-style,
//! the help text, and the two listings of the backends (the help's
//! `backends:` section and `--list`), all formatted from plain data so that
//! they can be tested without opening a library.

/// An option the command line takes: its long name without the dashes, its
/// one-letter name if it has one, and whether it takes a value.
pub struct Flag {
    pub long: &'static str,
    pub short: Option<char>,
    pub value: bool,
}

const fn flag(long: &'static str, short: Option<char>, value: bool) -> Flag {
    Flag { long, short, value }
}

/// Every option, in the help's order.
pub const FLAGS: &[Flag] = &[
    flag("help", Some('h'), false),
    flag("list", Some('L'), false),
    flag("forget", Some('F'), true),
    flag("connect", Some('c'), false),
    flag("user", Some('u'), true),
    flag("domain", None, true),
    flag("password", Some('p'), true),
    flag("option", Some('o'), true),
    flag("trust", None, false),
    flag("platform", None, true),
    flag("video", None, true),
    flag("audio", None, true),
    flag("cursor", None, true),
    flag("extlib-dir", None, true),
    flag("state-dir", None, true),
    flag("verbose", None, false),
    flag("scale", None, true),
    flag("view-only", None, false),
    flag("hw-decode", None, false),
    flag("no-send-clipboard", None, false),
    flag("no-recv-clipboard", None, false),
    flag("no-grab-keyboard", None, false),
    flag("probe", Some('P'), true),
    flag("connect-timeout", None, true),
    flag("out", None, true),
    flag("out-thumb", None, true),
    flag("thumb-step", None, true),
    flag("click", None, true),
    flag("key", None, true),
    flag("clipboard", None, true),
    flag("sound-cycle", None, true),
    flag("resize", None, true),
    flag("layout", None, true),
];

/// The probe's own options, which mean something only with `--probe`.
pub const PROBE_ONLY: &[&str] = &[
    "connect-timeout",
    "out",
    "out-thumb",
    "thumb-step",
    "click",
    "key",
    "clipboard",
    "sound-cycle",
    "resize",
    "layout",
];

/// One word of the command line, taken apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Arg {
    /// An option by its long name, with its value if it takes one.
    Flag(&'static str, Option<String>),
    Positional(String),
}

/// Takes `args` apart as GNU's getopt_long does: `--name value`,
/// `--name=value`, `-n value` and `-nvalue` for an option that takes a
/// value, and short options stacked (`-cu NAME`, `-cuNAME`) up to the first
/// one that takes a value, which takes the rest of the word or the next one.
/// A lone `-` is a positional argument, and everything after `--` is one too.
/// Positional arguments may come between the options.
pub fn split(args: Vec<String>, flags: &'static [Flag]) -> Result<Vec<Arg>, String> {
    let mut out = Vec::new();
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        if arg == "--" {
            out.extend(it.by_ref().map(Arg::Positional));
            break;
        }
        if let Some(long) = arg.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value.to_string())),
                None => (long, None),
            };
            let f = flags
                .iter()
                .find(|f| f.long == name)
                .ok_or_else(|| format!("unknown option --{name}"))?;
            let value = match (f.value, inline) {
                (true, Some(value)) => Some(value),
                (true, None) => Some(it.next().ok_or_else(|| format!("--{name} needs a value"))?),
                (false, Some(_)) => return Err(format!("--{name} takes no value")),
                (false, None) => None,
            };
            out.push(Arg::Flag(f.long, value));
        } else if let Some(shorts) = arg.strip_prefix('-').filter(|s| !s.is_empty()) {
            for (at, c) in shorts.char_indices() {
                let f = flags
                    .iter()
                    .find(|f| f.short == Some(c))
                    .ok_or_else(|| format!("unknown option -{c}"))?;
                if !f.value {
                    out.push(Arg::Flag(f.long, None));
                    continue;
                }
                let rest = &shorts[at + c.len_utf8()..];
                let value = if rest.is_empty() {
                    it.next().ok_or_else(|| format!("-{c} needs a value"))?
                } else {
                    rest.to_string()
                };
                out.push(Arg::Flag(f.long, Some(value)));
                break;
            }
        } else {
            out.push(Arg::Positional(arg));
        }
    }
    Ok(out)
}

/// The defaults the help shows, which depend on the machine.
pub struct Defaults {
    pub extlib_dir: String,
    pub state_dir: String,
}

/// The default `--scale`, `--connect-timeout` and `--thumb-step`.
pub const SCALE: &str = "auto";
pub const CONNECT_TIMEOUT: u64 = 30;
pub const THUMB_STEP: usize = 8;

const USAGE: &str = "\
usage:
  vento                                       connection dialog
  vento [<backend> [<address>]] [options]     connection dialog (prefilled)
  vento <backend> <address> --connect ...     single session, auto-connect
  vento <backend> <address> --probe N ...     headless probe
  vento --list                                list backends
  vento --forget <address> [--state-dir DIR]  forget host

arguments:
  <backend>                  backend name from --list
  <address>                  host, host:port, host:display, [literal]:port, etc

commands:
  -h, --help                 show this help
  -L, --list                 list backends
  -F, --forget ADDRESS       forget the server with the specified address

options:
  -c, --connect              automatically connect, and quit on disconnect
  -u, --user NAME            username (can also include domain for RDP, overriding --domain)
      --domain NAME          domain (RDP only)
  -p, --password TEXT        password
  -o, --option KEY=VALUE     backend option (see backends below)
      --trust                automatically accept all trust/security/message prompts (ignoring and not writing to known_hosts)

      --platform NAME        window server protocol (auto, x11, wayland) (default: auto)
      --video NAME           video hardware decoding backend (auto, vaapi, off) (default: auto)
      --audio NAME           audio backend (auto, pipewire, alsa, off) (default: auto)
      --cursor MODE          how to draw the remote cursor (native, software) (default: native)
      --extlib-dir DIR       path to extra libraries required by some backends (default: <extlib-dir>)
      --state-dir DIR        path to store known hosts and other state (default: <state-dir>)
      --verbose              show debug logs

session options (also see backends below):
      --scale MODE           initial window scale (auto, fit, fill, fit-height, fit-width, fill-width, fill-height, none, none-resize) (default: <scale>)
      --view-only            force view-only mode
      --hw-decode            enable hardware decoding for video where supported
      --no-send-clipboard    do not send locally copied text to the server
      --no-recv-clipboard    do not update local clipboard from the server
      --no-grab-keyboard     do not automatically grab the keyboard

probe options (--probe must be provided for these options to work):
  -P, --probe N              run headless, wait for N seconds after connecting (implies --connect)
      --connect-timeout N    wait up to N seconds for the connection (default: <connect-timeout>)
      --out FILE             save framebuffer ppm
      --out-thumb FILE       save framebuffer thumbnail ppm
      --thumb-step N         thumbnail scale, 1/N (default: <thumb-step>)
      --click X,Y            left-click once connected
      --key KEYSYM           press and release X keysym (e.g., 0x61, 97, 0141 are all 'a')
      --clipboard TEXT       send this as the clipboard a second after connecting
      --sound-cycle N        turn the far end's sound off and on every N seconds
      --resize WxH           ask for a desktop this size a second after connecting
      --layout WxH,WxH,...   ask for these screens side by side instead
";

/// The help without its `backends:` section, the defaults filled in.
pub fn usage(defaults: &Defaults) -> String {
    USAGE
        .replace("<extlib-dir>", &defaults.extlib_dir)
        .replace("<state-dir>", &defaults.state_dir)
        .replace("<scale>", SCALE)
        .replace("<connect-timeout>", &CONNECT_TIMEOUT.to_string())
        .replace("<thumb-step>", &THUMB_STEP.to_string())
}

/// A backend as the help lists it.
pub struct Backend {
    /// The name the command line takes.
    pub id: String,
    /// What the dialog calls it.
    pub name: String,
    /// The library's version, or "not available: <why>".
    pub version: String,
    pub description: String,
    pub options: Vec<BackendOption>,
}

/// One of a backend's options as the help lists it.
pub struct BackendOption {
    pub key: String,
    pub default: String,
    /// The values it takes, where it is a choice or a switch; empty for a
    /// number or text, whose description says what it takes.
    pub values: Vec<String>,
    pub description: String,
}

/// Where the descriptions of the backends' options start, unless every row
/// ends before it: a row longer than this has its description on the next
/// line, at this column.
const OPTION_COLUMN: usize = 50;

/// The help's `backends:` section: per backend a line with its name,
/// what the dialog calls it and its version, a line with its description,
/// and its options as `-o` takes them, the descriptions in one column.
pub fn backends(list: &[Backend]) -> String {
    let left = |o: &BackendOption| {
        let mut s = format!("    -o {}={}", o.key, o.default);
        if !o.values.is_empty() {
            s.push_str(&format!(" [{}]", o.values.join("|")));
        }
        s
    };
    let column = list
        .iter()
        .flat_map(|b| &b.options)
        .map(|o| left(o).len() + 2)
        .filter(|&w| w <= OPTION_COLUMN)
        .max()
        .unwrap_or(OPTION_COLUMN);
    let mut out = String::from("backends:\n");
    for (i, b) in list.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&format!("  {}  {}  {}\n", b.id, b.name, b.version));
        out.push_str(&format!("  {}\n", b.description));
        for o in &b.options {
            let left = left(o);
            if left.len() + 2 <= column {
                out.push_str(&format!("{left:column$}{}\n", o.description));
            } else {
                out.push_str(&format!("{left}\n{:column$}{}\n", "", o.description));
            }
        }
    }
    out
}

/// `--list`'s line for a backend: its name and the library it runs from,
/// or the library it looked for and why that cannot be opened.
pub fn list_line(id: &str, library: &Result<std::path::PathBuf, (std::path::PathBuf, String)>) -> String {
    match library {
        Ok(path) => format!("{id} {}", path.display()),
        Err((path, why)) => format!("{id} {} (error: {why})", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn split_(args: &[&str]) -> Result<Vec<Arg>, String> {
        split(args.iter().map(|a| a.to_string()).collect(), FLAGS)
    }

    fn f(long: &'static str, value: Option<&str>) -> Arg {
        Arg::Flag(long, value.map(str::to_string))
    }

    fn p(s: &str) -> Arg {
        Arg::Positional(s.to_string())
    }

    #[test]
    fn a_value_is_taken_in_each_of_the_four_forms() {
        let want = vec![f("user", Some("me"))];
        assert_eq!(split_(&["--user", "me"]), Ok(want.clone()));
        assert_eq!(split_(&["--user=me"]), Ok(want.clone()));
        assert_eq!(split_(&["-u", "me"]), Ok(want.clone()));
        assert_eq!(split_(&["-ume"]), Ok(want));
        // Taken whatever it looks like.
        assert_eq!(split_(&["--user", "--verbose"]), Ok(vec![f("user", Some("--verbose"))]));
        assert_eq!(split_(&["-u", "-c"]), Ok(vec![f("user", Some("-c"))]));
        assert_eq!(split_(&["--user="]), Ok(vec![f("user", Some(""))]));
        assert_eq!(split_(&["-o", "A=b=c"]), Ok(vec![f("option", Some("A=b=c"))]));
        assert_eq!(split_(&["--option=A=b"]), Ok(vec![f("option", Some("A=b"))]));
        assert_eq!(split_(&["-oA=b"]), Ok(vec![f("option", Some("A=b"))]));
    }

    #[test]
    fn short_options_stack_up_to_one_that_takes_a_value() {
        assert_eq!(split_(&["-cL"]), Ok(vec![f("connect", None), f("list", None)]));
        assert_eq!(split_(&["-cume"]), Ok(vec![f("connect", None), f("user", Some("me"))]));
        assert_eq!(split_(&["-cu", "me"]), Ok(vec![f("connect", None), f("user", Some("me"))]));
        // The rest of the word is the value, letters of other options and all.
        assert_eq!(split_(&["-ucL"]), Ok(vec![f("user", Some("cL"))]));
        assert_eq!(split_(&["-cP5"]), Ok(vec![f("connect", None), f("probe", Some("5"))]));
    }

    #[test]
    fn positionals_go_anywhere_and_after_a_double_dash_everything_is_one() {
        assert_eq!(
            split_(&["rfb", "-c", "host:1", "-"]),
            Ok(vec![p("rfb"), f("connect", None), p("host:1"), p("-")])
        );
        assert_eq!(split_(&["--", "-c", "--user"]), Ok(vec![p("-c"), p("--user")]));
        assert_eq!(split_(&[]), Ok(vec![]));
    }

    #[test]
    fn mistakes_are_refused_by_name() {
        assert_eq!(split_(&["--x11"]), Err("unknown option --x11".into()));
        assert_eq!(split_(&["--x11=1"]), Err("unknown option --x11".into()));
        assert_eq!(split_(&["-x"]), Err("unknown option -x".into()));
        assert_eq!(split_(&["-cx"]), Err("unknown option -x".into()));
        assert_eq!(split_(&["--user"]), Err("--user needs a value".into()));
        assert_eq!(split_(&["-u"]), Err("-u needs a value".into()));
        assert_eq!(split_(&["-cu"]), Err("-u needs a value".into()));
        assert_eq!(split_(&["--connect=yes"]), Err("--connect takes no value".into()));
        assert_eq!(split_(&["--hw-decode=vaapi"]), Err("--hw-decode takes no value".into()));
        // No abbreviations.
        assert_eq!(split_(&["--conn"]), Err("unknown option --conn".into()));
    }

    /// Every option is in the help, by its names and with a value where it
    /// takes one, and the help has no option the parser does not take.
    #[test]
    fn the_help_and_the_parser_have_the_same_options() {
        let help = usage(&Defaults { extlib_dir: "E".into(), state_dir: "S".into() });
        let lines: Vec<&str> = help.lines().filter(|l| l.starts_with("  -") || l.starts_with("      --")).collect();
        for flag in FLAGS {
            let names = match flag.short {
                Some(c) => format!("  -{c}, --{}", flag.long),
                None => format!("      --{}", flag.long),
            };
            let line = lines
                .iter()
                .find(|l| l.starts_with(&format!("{names} ")))
                .unwrap_or_else(|| panic!("--{} is not in the help", flag.long));
            let after = &line[names.len()..];
            let has_value = !after.starts_with("  ");
            assert_eq!(has_value, flag.value, "--{}: {line:?}", flag.long);
        }
        for line in &lines {
            let long = line.split_whitespace().find_map(|w| w.strip_prefix("--")).unwrap();
            assert!(FLAGS.iter().any(|f| f.long == long), "{line:?}");
        }
        for name in PROBE_ONLY {
            assert!(FLAGS.iter().any(|f| f.long == *name), "{name}");
        }
    }

    #[test]
    fn the_help_shows_the_defaults_and_its_descriptions_line_up() {
        let help = usage(&Defaults {
            extlib_dir: "/opt/vento/lib".into(),
            state_dir: "/home/u/.local/state/vento".into(),
        });
        assert!(help.contains("(default: /opt/vento/lib)\n"), "{help}");
        assert!(help.contains("(default: /home/u/.local/state/vento)\n"), "{help}");
        assert!(help.contains("none-resize) (default: auto)\n"), "{help}");
        assert!(help.contains("(default: 30)\n"), "{help}");
        assert!(help.contains("1/N (default: 8)\n"), "{help}");
        for placeholder in ["<extlib-dir>", "<state-dir>", "<scale>", "<connect-timeout>", "<thumb-step>"] {
            assert!(!help.contains(placeholder), "{placeholder}");
        }
        // Every option's description starts in the same column.
        for line in help.lines().filter(|l| l.starts_with("  -") || l.starts_with("      --")) {
            assert_eq!(&line[27..29], "  ", "{line:?}");
            assert_ne!(line.as_bytes()[29], b' ', "{line:?}");
        }
        assert!(!help.contains(['\u{2013}', '\u{2014}']));
    }

    fn option(key: &str, default: &str, values: &[&str], description: &str) -> BackendOption {
        BackendOption {
            key: key.into(),
            default: default.into(),
            values: values.iter().map(|v| v.to_string()).collect(),
            description: description.into(),
        }
    }

    #[test]
    fn the_backends_are_listed_with_their_options_in_one_column() {
        let list = [
            Backend {
                id: "rfb".into(),
                name: "VNC (Rust)".into(),
                version: "RFB 3.8".into(),
                description: "Rust RFB implementation.".into(),
                options: vec![
                    option("ViewOnly", "false", &["true", "false"], "Do not send input."),
                    option("KeepAlive", "30", &[], "Seconds between keep-alives."),
                ],
            },
            Backend {
                id: "realvnc".into(),
                name: "VNC (RealVNC)".into(),
                version: "not available: libvento_realvnc.so: cannot open shared object file".into(),
                description: "RealVNC's client.".into(),
                options: vec![option(
                    "DesktopSize",
                    "1920x1200",
                    &["remote", "device", "1920x1200", "1920x1080", "1600x1000"],
                    "Preferred screen resolution.",
                )],
            },
        ];
        assert_eq!(
            backends(&list),
            "\
backends:
  rfb  VNC (Rust)  RFB 3.8
  Rust RFB implementation.
    -o ViewOnly=false [true|false]  Do not send input.
    -o KeepAlive=30                 Seconds between keep-alives.

  realvnc  VNC (RealVNC)  not available: libvento_realvnc.so: cannot open shared object file
  RealVNC's client.
    -o DesktopSize=1920x1200 [remote|device|1920x1200|1920x1080|1600x1000]
                                    Preferred screen resolution.
"
        );
        assert_eq!(backends(&[]), "backends:\n");
    }

    #[test]
    fn a_list_line_is_the_name_and_the_library_or_why_not() {
        assert_eq!(list_line("rfb", &Ok(PathBuf::from("/opt/vento/vento"))), "rfb /opt/vento/vento");
        assert_eq!(
            list_line(
                "libvnc",
                &Err((
                    PathBuf::from("/opt/vento/lib/libvento_libvnc.so"),
                    "libz.so.1: cannot open shared object file".into()
                ))
            ),
            "libvnc /opt/vento/lib/libvento_libvnc.so (error: libz.so.1: cannot open shared object file)"
        );
    }
}
