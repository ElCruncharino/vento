# X server with a window manager

Not a far end: the near one. A plain X server (Xvfb) and **Openbox**, a real
X11 window manager with global keybindings of its own, in a container, for
running the Linux client (`linux/`) on X11 the way it runs on an X desktop.
The client runs on the host as an ordinary X client of this server, with
`DISPLAY=:42`; the far ends are the other rigs (`testvnc`, `testqemu`), reached
at `127.0.0.1` as usual.

```sh
./run.sh                 # build if needed, run; DISPLAY=:42 then reaches it
./run.sh keys            # the window manager's witness log (below)
./run.sh shot [out.png]  # the whole screen (default: ./vento-testx11.png)
./run.sh logs            # the container's output
./run.sh stop
```

Defaults: display `:42`, `1600x1000`. Override with `DISPLAY_NUM=`,
`GEOMETRY=`, `NAME=` in the environment.

## Why this, rather than the X server already on a Wayland desktop

A Wayland session's `:0` is Xwayland, which answers to its compositor: whether
a keyboard grab holds off the compositor's shortcuts, whether a pointer warp
lands, what the scale is — all of that is the compositor's call, and some of
it is a permission prompt. A test there tests the compositor. Xvfb with
Openbox is X with nothing in between: a grab is a grab, a warp is a warp, and
the window manager is an ordinary X client whose keybindings are passive grabs
on the root window, which is what most X window managers are.

## How it is wired

- `--network host` puts Xvfb's abstract socket (`@/tmp/.X11-unix/X42`) in the
  host's network namespace, which is how `DISPLAY=:42` reaches it from the
  host and why no socket file is left behind. (Bind-mounting
  `/tmp/.X11-unix` instead left a root-owned `X42` after `docker rm -f`.)
- `--ipc=host` lets MIT-SHM work between the server and host clients.
- The client is the host's build, so its rpath'd C backends and FFmpeg are
  the ones it was built with. `xdotool`, `xprop`, `xclip`, `import` run on
  the host too.
- Xvfb has no DRI3, so wgpu takes **llvmpipe** (Vulkan); it renders
  correctly, on the CPU.

## The witnesses

Openbox's stock bindings stay (Alt+Tab, Ctrl+Alt+Left/Right, Super+F1…F4,
Alt+F4, …) and two more are added, which write a line to
`/rig/out/wm-keys.log` when the window manager — and not the focused client —
gets them: **Super+Return** and **Ctrl+Alt+T**. `./run.sh keys` prints it.
While the client holds its keyboard grab the log stays empty and Alt+Tab
does not change the active window; with the xterm focused, Super+Return
writes a line. An xterm titled `witness-xterm` is there to take the focus
away to.

## The cursor

Screenshots never include the cursor. `cursor.py` prints the server's
current cursor through `XFixesGetCursorImage` — its size, hotspot, a few
pixels as ARGB, and how many pixels have a colour channel above their alpha,
which a premultiplied Render cursor must not have:

```sh
python3 -I cursor.py :42
```

## xdotool, and the rules that matter

- Give focus with `xdotool windowactivate --sync $W`, which goes through the
  window manager, then send plain `xdotool key …`. **Never `key --window`**:
  that is `XSendEvent`, which bypasses grabs, so it proves nothing about one.
- While the client has the pointer captured (relative mode), move it only
  with `xdotool mousemove_relative -- DX DY`. An absolute `mousemove` comes
  out of the XTEST device as *absolute* raw values (the root position), which
  the client's raw motion reads as one huge move.
- While a connected session has the keyboard, Alt+F4 is the far end's too.
  `xdotool windowquit $W` closes the client the way the window manager's
  close button does.
- Ctrl+Alt is the client's escape from a captured pointer (the two keys
  still reach the far end); click the desktop to capture again. The debug
  overlay is in the connection panel.
- Relative mode needs a backend that speaks it: `rfb` against testqemu's VNC
  port, or `spice` against its SPICE port. The C VNC backends (`tigervnc`,
  `libvnc`) have no relative pointer and stay absolute there.
- `xdotool key super+Return` on a Wayland session's `:0` reaches Xwayland
  only, never the compositor, so it cannot test the compositor's shortcuts.

## A session against it

```sh
scripts/testvnc/run.sh; scripts/testqemu/run.sh; scripts/testx11/run.sh
cd linux && cargo build --offline
export DISPLAY=:42; unset WAYLAND_DISPLAY
./target/debug/vento tigervnc 127.0.0.1:5901 --password vncpass --connect --verbose &
W=$(xdotool search --sync --name 'Vento' | head -1)
xdotool windowactivate --sync $W
xdotool key super+Return ctrl+alt+t alt+Tab
../scripts/testx11/run.sh keys                     # empty: the client had them
docker exec -e DISPLAY=:1 protovnc xdotool getactivewindow getwindowname
```
