// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! RustDesk's session encryption: two signatures, one sealed key, and a stream
//! cipher that counts its frames.
//!
//! The design is easy to misread as one signature, but it has two:
//!
//! 1. the rendezvous server's `PunchHoleResponse` carries an `IdPk{id, pk}`
//!    signed with **its** secret key, where `pk` is the peer's **long-term**
//!    sign key. That is an introduction: this id belongs to this key.
//! 2. the peer then sends `SignedId`, another `IdPk`, signed with **that** key,
//!    whose `pk` is a **per-connection** curve25519 box key.
//! 3. a fresh secretbox key is sealed to that box key and sent, and every frame
//!    after the one carrying it is encrypted under it.
//!
//! So the server never sees the key a session is sealed to and cannot
//! substitute one. A dishonest server can introduce the wrong machine, but only
//! for an id it has not already introduced honestly, which the pin on the
//! long-term key detects.

use crate::error::{Error, Result};
use crate::protos::message::IdPk;
use crate::wire::Cipher;

use dryoc::classic::crypto_box::{crypto_box_easy, crypto_box_keypair};
use dryoc::classic::crypto_secretbox::{crypto_secretbox_easy, crypto_secretbox_open_easy};
use dryoc::classic::crypto_sign::crypto_sign_open;
use protobuf::Message as _;
use sha2::{Digest, Sha256};

/// An ed25519 signature is prepended to the message it signs rather than sent
/// separately, so an opened message is the frame minus this much.
const SIGNATURE: usize = 64;

/// RustDesk's session cipher: xsalsa20-poly1305 with the frame's sequence
/// number as the nonce, little-endian and zero-padded. The counter increments
/// before use, so frame zero is never sent, and each direction counts
/// separately, so there is one instance per direction rather than a shared
/// one.
pub struct Secretbox {
    key: [u8; 32],
    counter: u64,
}

impl Secretbox {
    pub fn new(key: [u8; 32]) -> Secretbox {
        Secretbox { key, counter: 0 }
    }

    fn nonce(n: u64) -> [u8; 24] {
        let mut nonce = [0u8; 24];
        nonce[..8].copy_from_slice(&n.to_le_bytes());
        nonce
    }
}

impl Cipher for Secretbox {
    fn seal(&mut self, payload: Vec<u8>) -> std::io::Result<Vec<u8>> {
        self.counter += 1;
        let mut sealed = vec![0u8; payload.len() + 16];
        crypto_secretbox_easy(
            &mut sealed,
            &payload,
            &Secretbox::nonce(self.counter),
            &self.key,
        )
        .map_err(std::io::Error::other)?;
        Ok(sealed)
    }

    fn open(&mut self, payload: Vec<u8>) -> std::io::Result<Vec<u8>> {
        self.counter += 1;
        if payload.len() < 16 {
            return Err(std::io::Error::other("a sealed frame with no tag in it"));
        }
        let mut opened = vec![0u8; payload.len() - 16];
        crypto_secretbox_open_easy(
            &mut opened,
            &payload,
            &Secretbox::nonce(self.counter),
            &self.key,
        )
        .map_err(std::io::Error::other)?;
        Ok(opened)
    }
}

/// One link of the chain: opens a signed message, parses the `IdPk` inside,
/// checks that the id is the dialled one, and returns its key.
///
/// The id check is the purpose of the message. Without it a server could
/// introduce any machine for any id, since the signature would still be its
/// own.
pub fn open_id_pk(signed: &[u8], with: &[u8; 32], peer_id: &str) -> Result<[u8; 32]> {
    if signed.len() < SIGNATURE {
        return Err(Error::Protocol("a signature shorter than a signature".into()));
    }
    let mut opened = vec![0u8; signed.len() - SIGNATURE];
    crypto_sign_open(&mut opened, signed, with)
        .map_err(|_| Error::Protocol("the signature does not check out".into()))?;
    let id_pk = IdPk::parse_from_bytes(&opened)
        .map_err(|e| Error::Protocol(format!("what was signed is not an IdPk: {e}")))?;
    if id_pk.id != peer_id {
        return Err(Error::Protocol(format!(
            "signed for {} rather than for {peer_id}",
            id_pk.id
        )));
    }
    id_pk
        .pk
        .to_vec()
        .try_into()
        .map_err(|_| Error::Protocol("the key inside is not 32 bytes".into()))
}

/// A fresh session key sealed to the peer's per-connection box key: returns our
/// public box key, the sealed key to send, and the key itself.
///
/// The nonce is zero, which is safe for a single use per key pair, and it is
/// used once: the peer's box key is new for this connection and ours is new
/// for this call.
pub fn seal_session_key(their_box_pk: &[u8; 32]) -> Result<(Vec<u8>, Vec<u8>, [u8; 32])> {
    let (our_pk, our_sk) = crypto_box_keypair();
    let mut key = [0u8; 32];
    dryoc::rng::copy_randombytes(&mut key);
    let mut sealed = vec![0u8; key.len() + 16];
    crypto_box_easy(&mut sealed, &key, &[0u8; 24], their_box_pk, &our_sk)
        .map_err(|e| Error::Protocol(format!("sealing the session key: {e}")))?;
    Ok((our_pk.to_vec(), sealed, key))
}

/// The form of a peer's long-term key shown to the user and stored in the pin
/// store: its SHA-256 as colon-separated uppercase hex pairs.
///
/// A digest rather than the base64 RustDesk's client shows, because the store
/// compares hex digits and ignores everything else: it is keyed on machines
/// rather than clients, so entries have to be in the form every backend here
/// already uses.
pub fn fingerprint(key: &[u8; 32]) -> String {
    Sha256::digest(key)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// A rendezvous server's public key in its text form: 32 bytes in base64, as
/// used by RustDesk's config file, web UI and `id_ed25519.pub`.
pub fn server_key(text: &str) -> Result<[u8; 32]> {
    base64(text.trim())?
        .try_into()
        .map_err(|_| Error::Protocol("a rendezvous key that is not 32 bytes".into()))
}

/// Decodes standard base64. Padding and whitespace are ignored rather than
/// required, since the input is pasted by the user.
fn base64(text: &str) -> Result<Vec<u8>> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in text
        .bytes()
        .filter(|c| *c != b'=' && !c.is_ascii_whitespace())
    {
        let value = ALPHABET
            .iter()
            .position(|a| *a == c)
            .ok_or_else(|| Error::Protocol("a rendezvous key that is not base64".into()))?
            as u32;
        acc = (acc << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The public network's key, which is built into every RustDesk build and
    /// is the only input here with a known value.
    #[test]
    fn the_public_network_key_is_32_bytes() {
        let key = server_key(crate::rendezvous::PUBLIC_KEY).unwrap();
        assert_eq!(key.len(), 32);
        assert_eq!(key[0], 0x39);
    }

    /// Frames open in the order they were sealed: the nonce is the frame's
    /// number, and the two ends agree on it only because each direction counts
    /// separately.
    #[test]
    fn frames_open_in_the_order_they_were_sealed() {
        let key = [7u8; 32];
        let mut sender = Secretbox::new(key);
        let mut receiver = Secretbox::new(key);
        for n in 0..4usize {
            let payload = vec![n as u8; n * 100];
            let sealed = sender.seal(payload.clone()).unwrap();
            assert_eq!(receiver.open(sealed).unwrap(), payload);
        }
    }

    /// A frame opened out of order fails rather than producing garbage: the
    /// nonce is wrong, so the tag does not verify.
    #[test]
    fn a_frame_out_of_order_does_not_open() {
        let key = [9u8; 32];
        let mut sender = Secretbox::new(key);
        let mut receiver = Secretbox::new(key);
        let first = sender.seal(b"one".to_vec()).unwrap();
        let second = sender.seal(b"two".to_vec()).unwrap();
        assert!(receiver.open(second).is_err());
        assert!(receiver.open(first).is_err());
    }

    /// The whole chain, built here and verified as a session verifies it: a
    /// server vouching for a peer's long-term key, the peer signing a box key
    /// with it, and a session key sealed to that.
    #[test]
    fn the_two_signatures_and_the_sealed_key() {
        use dryoc::classic::crypto_box::crypto_box_open_easy;
        use dryoc::classic::crypto_sign::{crypto_sign, crypto_sign_keypair};

        let (server_pk, server_sk) = crypto_sign_keypair();
        let (peer_pk, peer_sk) = crypto_sign_keypair();
        let (box_pk, box_sk) = crypto_box_keypair();

        let sign = |pk: &[u8; 32], id: &str, sk: &_| {
            let mut id_pk = IdPk::new();
            id_pk.id = id.into();
            id_pk.pk = pk.to_vec().into();
            let message = id_pk.write_to_bytes().unwrap();
            let mut signed = vec![0u8; message.len() + SIGNATURE];
            crypto_sign(&mut signed, &message, sk).unwrap();
            signed
        };

        let introduction = sign(&peer_pk, "123456789", &server_sk);
        assert_eq!(
            open_id_pk(&introduction, &server_pk, "123456789").unwrap(),
            peer_pk
        );
        // The id in the signature is checked against the id that was dialled,
        // so a valid signature for another machine is refused.
        assert!(open_id_pk(&introduction, &server_pk, "987654321").is_err());

        let signed_id = sign(&box_pk, "123456789", &peer_sk);
        let their_box_pk = open_id_pk(&signed_id, &peer_pk, "123456789").unwrap();
        let (our_box_pk, sealed, key) = seal_session_key(&their_box_pk).unwrap();
        let mut opened = vec![0u8; sealed.len() - 16];
        crypto_box_open_easy(
            &mut opened,
            &sealed,
            &[0u8; 24],
            &our_box_pk.try_into().unwrap(),
            &box_sk,
        )
        .unwrap();
        assert_eq!(opened, key);
    }
}
