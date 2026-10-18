// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

// A picture left on the GPU, NV12 or P010 in two textures, into the
// `R, G, B, A` the desktop's texture holds: `media::video::convert`'s
// arithmetic, in integers, so that every pixel is the one the processor
// would have made of the same samples (`zerocopy.rs` tests that it is).
//
// One triangle over the whole target, which is the picture's visible size:
// each fragment reads its own luma sample and the chroma sample its two by
// two block shares, by index, with no filtering, and the rows of the surface
// past the picture (1088 for 1080) are never read.

struct Params {
    // `convert::Coefficients`, in 1/256ths.
    y: i32,
    rv: i32,
    gu: i32,
    gv: i32,
    bu: i32,
    y_offset: i32,
    // Sixteen bits a sample (P010, ten of them at the top) rather than eight.
    deep: i32,
    unused: i32,
}

@group(0) @binding(0) var luma: texture_2d<f32>;
@group(0) @binding(1) var chroma: texture_2d<f32>;
@group(0) @binding(2) var<uniform> p: Params;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    // (-1, -1), (3, -1), (-1, 3): the target and more.
    let x = f32((i & 1u) * 4u) - 1.0;
    let y = f32((i >> 1u) * 4u) - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

// A sample at eight bits, as the processor reads it: the byte itself, or a
// sixteen-bit sample narrowed as `ffmpeg.rs` narrows one, rounded to the
// nearest (limited range's 64 and 940 in ten bits are 16 and 235).
fn sample(v: f32) -> i32 {
    if p.deep != 0 {
        let wide = i32(round(v * 65535.0));
        return min((wide + 128) >> 8u, 255);
    }
    return i32(round(v * 255.0));
}

@fragment
fn fs(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(at.xy);
    let y = sample(textureLoad(luma, pixel, 0).r);
    let uv = textureLoad(chroma, pixel >> vec2<u32>(1u, 1u), 0);
    let u = sample(uv.r) - 128;
    let v = sample(uv.g) - 128;
    let yy = (y + p.y_offset) * p.y;
    let r = clamp((yy + p.rv * v + 128) >> 8u, 0, 255);
    let g = clamp((yy + p.gu * u + p.gv * v + 128) >> 8u, 0, 255);
    let b = clamp((yy + p.bu * u + 128) >> 8u, 0, 255);
    return vec4<f32>(f32(r), f32(g), f32(b), 255.0) / 255.0;
}
