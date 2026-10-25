#!/bin/bash
# SPDX-FileCopyrightText: 2026 Patrick Gaskin
# SPDX-License-Identifier: GPL-3.0-or-later
#
# The decoder tests' fixtures, and the only record of how they were made. Run
# from anywhere; it writes beside itself. What came out of it was committed,
# so a test does not depend on which encoders the machine running it has —
# only on libavcodec decoding them.
#
# Every clip is the same 128×128 picture, three frames of it at ten a second:
#
#   frame 0  all #3366cc
#   frame 1  the same, with rows 16 to 31 #e0a030 across the whole width
#   frame 2  frame 1 again
#
# so that the colour can be asserted on, the second frame moves sixteen rows,
# and the third moves nothing the encoder did not choose to touch up. What a
# lossy encoder may change around the band is its own business as far as the
# block the band is in; 128 rows is what puts the bottom half in a different
# 64-row superblock (VP9, AV1) or coding tree unit (H.265) from the band, so
# that nothing there can move, and the test says so.
#
# The source is RGB, converted to YUV by an explicit matrix and range and
# tagged as such, so what the stream says about its colour is what was done to
# it. VP8 is BT.601 because its bitstream has nowhere to say anything else; the
# rest are BT.709 limited range, so that a decoder that ignored the tags would
# fail on colour, and one H.264 clip is BT.601 full range to prove the same of
# range. The two "untagged" clips are what a RustDesk peer actually sends:
# BT.601 limited range (libyuv's ARGBToI420) with no colour description at all,
# which a decoder has to take for BT.601 rather than assume BT.709. Two more
# are deeper than eight bits, which is narrowed rather than refused: H.264
# High 10 (4:2:0) and VP9 profile 3 (4:4:4, ten bits).
#
# The encoders are asked for what a remote desktop asks for: no lookahead, no
# reordering, one encoded frame per picture — no alt-ref frames hidden in a
# VP9 superframe, no B-frames. H.264 and H.265 carry access unit delimiters,
# which are what the test splits their Annex B streams into frames at; VP8,
# VP9 and AV1 are IVF, whose frame headers do that already.
#
# Made with ffmpeg 8 (libavcodec 62.28) and libvpx, libaom, x264 and x265.
set -euo pipefail
cd "$(dirname "$0")"

picture() { # matrix range [format]
    echo "color=c=0x3366cc:s=128x128:r=10:d=0.3,format=rgb24,drawbox=x=0:y=16:w=128:h=16:color=0xe0a030:t=fill:enable='gte(n,1)',scale=out_color_matrix=$1:out_range=$2,format=${3:-yuv420p}"
}
bt709=(-colorspace bt709 -color_primaries bt709 -color_trc bt709 -color_range tv)
bt601=(-colorspace smpte170m -color_primaries smpte170m -color_trc smpte170m)
# The scaler tags what it converts, and the encoders write what the frames say;
# this takes the description off again, as RustDesk's encoders never set one.
untagged="setparams=colorspace=unknown:color_primaries=unknown:color_trc=unknown:range=tv"
ff() { ffmpeg -hide_banner -loglevel error -y -f lavfi -i "$@"; }

ff "$(picture bt709 tv)" "${bt709[@]}" -frames:v 3 \
    -c:v libvpx-vp9 -deadline good -cpu-used 4 -lag-in-frames 0 -auto-alt-ref 0 -crf 4 -b:v 0 -g 100 \
    -f ivf vp9.ivf
ff "$(picture bt601 tv)" "${bt601[@]}" -color_range tv -frames:v 3 \
    -c:v libvpx -deadline good -cpu-used 4 -lag-in-frames 0 -auto-alt-ref 0 -crf 4 -b:v 2M -g 100 \
    -f ivf vp8.ivf
ff "$(picture bt601 tv),$untagged" -frames:v 3 \
    -c:v libvpx-vp9 -deadline good -cpu-used 4 -lag-in-frames 0 -auto-alt-ref 0 -crf 4 -b:v 0 -g 100 \
    -f ivf vp9-untagged.ivf
ff "$(picture bt709 tv)" "${bt709[@]}" -frames:v 3 \
    -c:v libaom-av1 -usage realtime -cpu-used 8 -lag-in-frames 0 -crf 8 -g 100 \
    -f ivf av1.ivf
ff "$(picture bt601 tv),$untagged" -frames:v 3 \
    -c:v libaom-av1 -usage realtime -cpu-used 8 -lag-in-frames 0 -crf 8 -g 100 \
    -f ivf av1-untagged.ivf
ff "$(picture bt709 tv)" "${bt709[@]}" -frames:v 3 \
    -c:v libx264 -preset medium -tune zerolatency -crf 6 -g 100 -x264-params aud=1 \
    -f h264 h264.h264
ff "$(picture bt601 pc)" "${bt601[@]}" -color_range pc -frames:v 3 \
    -c:v libx264 -preset medium -tune zerolatency -crf 6 -g 100 -x264-params aud=1 \
    -f h264 h264-bt601-full.h264
ff "$(picture bt709 tv)" "${bt709[@]}" -frames:v 3 \
    -c:v libx265 -preset medium -tune zerolatency -crf 6 -x265-params aud=1:keyint=100:log-level=error \
    -f hevc h265.h265
ff "$(picture bt709 tv yuv420p10le)" "${bt709[@]}" -frames:v 3 \
    -c:v libx264 -preset medium -tune zerolatency -crf 6 -g 100 -x264-params aud=1 \
    -f h264 h264-high10.h264
ff "$(picture bt709 tv yuv444p10le)" "${bt709[@]}" -frames:v 3 \
    -c:v libvpx-vp9 -deadline good -cpu-used 4 -lag-in-frames 0 -auto-alt-ref 0 -crf 4 -b:v 0 -g 100 \
    -f ivf vp9-444-10bit.ivf
# Two more for the hardware decoders' output formats: H.265 Main 10, which
# VAAPI decodes into P010, and H.265 4:4:4 at ten bits (Main 4:4:4 10), into
# XV30. Software narrows both like the other deep clips.
ff "$(picture bt709 tv yuv420p10le)" "${bt709[@]}" -frames:v 3 \
    -c:v libx265 -preset medium -tune zerolatency -crf 6 -x265-params aud=1:keyint=100:log-level=error \
    -f hevc h265-main10.h265
ff "$(picture bt709 tv yuv444p10le)" "${bt709[@]}" -frames:v 3 \
    -c:v libx265 -preset medium -tune zerolatency -crf 6 -x265-params aud=1:keyint=100:log-level=error \
    -f hevc h265-444-10bit.h265
# And AV1 4:4:4 (profile 1), which iHD has no decoder for: what libavcodec's
# hardware-only `av1` cannot take, so the decoder is dav1d after all.
ff "$(picture bt709 tv yuv444p)" "${bt709[@]}" -frames:v 3 \
    -c:v libaom-av1 -usage realtime -cpu-used 8 -lag-in-frames 0 -crf 8 -g 100 \
    -f ivf av1-444.ivf
