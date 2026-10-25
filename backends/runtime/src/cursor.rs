// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! What every platform so far has done with a cursor: packed its pixels the
//! way a `Color` is packed, and hashed them.
//!
//! Here rather than in the Android crate because the hash is a contract with
//! more than one party — it is `CursorCache`'s identity, and the two C shims
//! and the Java compute the same one — and a second platform that wants to
//! share that cache has to compute it too.

/// `0xAA_BB_GG_RR` (memory order R, G, B, A) to `0xAA_RR_GG_BB`, which is
/// Android's `Color` packing and the one `Bitmap.setPixels` takes.
pub fn swap_rb(p: u32) -> u32 {
    (p & 0xff00_ff00) | ((p & 0x00ff_0000) >> 16) | ((p & 0x0000_00ff) << 16)
}

/// FNV-1a over a cursor's size and its `Color`-packed pixels, which is its
/// identity in `CursorCache` — so this and the two C shims' copies of it and
/// the Java one all have to agree, bit for bit. What it saves on the other
/// side is a bitmap and a texture per cursor change rather than anything on
/// the wire.
///
/// Over the *packed* pixels, not the framebuffer's word order, because that
/// is what the other copies see; a platform computes it after
/// [`swap_rb`], where the pixels are already in cache from the conversion.
pub fn hash(argb: &[u32], width: usize, height: usize) -> u64 {
    const PRIME: u64 = 0x100_0000_01b3;
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    h = (h ^ width as u32 as u64).wrapping_mul(PRIME);
    h = (h ^ height as u32 as u64).wrapping_mul(PRIME);
    for &p in argb {
        h = (h ^ p as u64).wrapping_mul(PRIME);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn red_and_blue_swap_and_nothing_else_moves() {
        assert_eq!(swap_rb(0x1234_5678), 0x1278_5634);
        assert_eq!(swap_rb(0x8000_00ff), 0x80ff_0000);
        assert_eq!(swap_rb(0xff00_ff00), 0xff00_ff00);
    }

    /// Pinned values, computed by the copy this one replaced: the hash is
    /// shared with code in two other languages, so a change here that still
    /// passes every other test is a cache that misses on every cursor.
    #[test]
    fn the_hash_is_the_one_the_shims_compute() {
        assert_eq!(hash(&[], 0, 0), 0x0832_8807_b4eb_6fed);
        assert_eq!(hash(&[0xff00_0000], 1, 1), 0xd0a6_fc65_fc2a_1435);
        assert_eq!(
            hash(&[0xffff_ffff, 0x0000_0000, 0x8012_3456], 3, 1),
            0xf206_26d2_6ae8_32d0
        );
    }
}
