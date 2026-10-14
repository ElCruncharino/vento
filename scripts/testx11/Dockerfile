# A plain X server and a real window manager, for the Linux client's X11 path.
# Xvfb (no Xwayland, no compositor in between) and Openbox, whose own global
# keybindings are what the client's keyboard grab is checked against. The
# client runs on the host as an ordinary X client of this server (see run.sh);
# xdotool, xwd, import and xclip run on the host too.
FROM debian:trixie
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
        xvfb openbox xterm x11-utils x11-xserver-utils xdotool \
        fonts-dejavu-core \
    && rm -rf /var/lib/apt/lists/*
COPY rc-keys.xml /rig/rc-keys.xml
COPY start.sh /rig/start.sh
CMD ["/rig/start.sh"]
