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
//!    after the one carrying it is encrypted under keys made from it.
//!
//! So the server never sees the key a session is sealed to and cannot
//! substitute one. A dishonest server can introduce the wrong machine, but only
//! for an id it has not already introduced honestly, which the pin on the
//! long-term key detects.
//!
//! Which keys are made from it is the key exchange version, negotiated in the
//! same three messages. In version 0, the original scheme, the sealed key is
//! used as it is in both directions, and each direction counts its nonces from
//! one, so the first frame each way is sealed with the same key and nonce, and
//! so is every frame after it: the keystream and the authenticator's key repeat
//! across directions. In version 1 (RustDesk 1.5.0) the peer advertises the newest
//! version it speaks inside the `IdPk` it signs, the controller answers with
//! the one it picked in `PublicKey`, and each direction gets a key of its own
//! derived from the sealed one ([`session_keys`]). A peer that advertises
//! nothing is version 0, which is what every release before 1.5.0 is.

use crate::error::{Error, Result};
use crate::protos::rendezvous::IdPk;
use crate::wire::Cipher;

use dryoc::classic::crypto_box::{crypto_box_easy, crypto_box_keypair};
use dryoc::classic::crypto_generichash::{
    crypto_generichash_final, crypto_generichash_init, crypto_generichash_update,
};
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
/// one, each with that direction's key from [`session_keys`].
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

/// What a signed `IdPk` vouches for, once its signature and id have checked
/// out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Signed {
    /// The key inside: the peer's long-term sign key in the server's
    /// introduction, its box key for this connection in its own `SignedId`.
    pub key: [u8; 32],
    /// The newest key exchange version the signer speaks. A peer's `SignedId`
    /// carries it from RustDesk 1.5.0; absent, as in the server's
    /// introduction and from every older peer, it is 0.
    pub kx_version: u32,
}

/// One link of the chain: opens a signed message, parses the `IdPk` inside,
/// checks that the id is the dialled one, and returns its key and the version
/// it advertises.
///
/// The id check is the purpose of the message. Without it a server could
/// introduce any machine for any id, since the signature would still be its
/// own.
pub fn open_id_pk(signed: &[u8], with: &[u8; 32], peer_id: &str) -> Result<Signed> {
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
    let key = id_pk
        .pk
        .to_vec()
        .try_into()
        .map_err(|_| Error::Protocol("the key inside is not 32 bytes".into()))?;
    Ok(Signed {
        key,
        kx_version: id_pk.kx_version,
    })
}

/// A fresh session key sealed to the peer's per-connection box key: returns our
/// public box key, the sealed key to send, and the key itself.
///
/// The nonce is zero, which is safe for a single use per key pair, and it is
/// used once: the peer's box key is new for this connection and ours is new
/// for this call.
pub fn seal_session_key(their_box_pk: &[u8; 32]) -> Result<([u8; 32], Vec<u8>, [u8; 32])> {
    let (our_pk, our_sk) = crypto_box_keypair();
    let mut key = [0u8; 32];
    dryoc::rng::copy_randombytes(&mut key);
    let mut sealed = vec![0u8; key.len() + 16];
    crypto_box_easy(&mut sealed, &key, &[0u8; 24], their_box_pk, &our_sk)
        .map_err(|e| Error::Protocol(format!("sealing the session key: {e}")))?;
    Ok((our_pk, sealed, key))
}

/// The newest key exchange version this client speaks: 1, a key per
/// direction, as RustDesk 1.5.0's `KX_VERSION_LATEST`.
pub const KX_VERSION_LATEST: u32 = 1;

/// The version to run against a peer that advertised `advertised`: the highest
/// both sides speak. The peer refuses a pick above what it advertised.
pub fn kx_version_for(advertised: u32) -> u32 {
    advertised.min(KX_VERSION_LATEST)
}

/// What both ends saw of the key exchange, mixed into version 1's keys, so a
/// version or a box key changed in transit leaves the two ends with keys that
/// do not match rather than with a weaker exchange.
///
/// The initiator is the side that sealed the session key, which is always this
/// client (the controller); the responder is the side it was sealed to.
pub struct Transcript<'a> {
    /// Our box key, sent in `PublicKey.asymmetric_value`.
    pub initiator_pk: &'a [u8; 32],
    /// The peer's box key, from its `SignedId`.
    pub responder_pk: &'a [u8; 32],
    /// The version the peer advertised in its `SignedId`.
    pub advertised: u32,
    /// The version picked, sent in `PublicKey.kx_version`.
    pub picked: u32,
}

/// The keys a session's two directions are sealed with, for the initiator.
#[derive(Debug, PartialEq, Eq)]
pub struct SessionKeys {
    pub send: [u8; 32],
    pub receive: [u8; 32],
}

/// Version 1's domain separation, as hbb_common's `KX_SPLIT_CONTEXT`.
const SPLIT_CONTEXT: &[u8] = b"rdkx-spl";
const SPLIT_INITIATOR: u8 = 1;
const SPLIT_RESPONDER: u8 = 2;

/// The keys for the version picked: in version 0 the sealed key both ways, in
/// version 1 one derived per direction (hbb_common's `Encrypt::new_split`), and
/// any other version is refused rather than run as one of these.
pub fn session_keys(key: &[u8; 32], t: &Transcript) -> Result<SessionKeys> {
    match t.picked {
        0 => Ok(SessionKeys {
            send: *key,
            receive: *key,
        }),
        1 => Ok(SessionKeys {
            send: split(key, SPLIT_INITIATOR, t)?,
            receive: split(key, SPLIT_RESPONDER, t)?,
        }),
        v => Err(Error::Protocol(format!("unsupported key exchange version {v}"))),
    }
}

/// One direction's key in version 1: BLAKE2b-256 keyed with the sealed key, over
/// the context, the direction (1 for what the initiator sends, 2 for what the
/// responder sends), both versions as little-endian u32s and both box keys,
/// initiator's first.
fn split(key: &[u8; 32], direction: u8, t: &Transcript) -> Result<[u8; 32]> {
    let failed = |e| Error::Protocol(format!("deriving a session key: {e}"));
    let mut h = crypto_generichash_init(Some(key), 32).map_err(failed)?;
    for part in [
        SPLIT_CONTEXT,
        &[direction],
        &t.advertised.to_le_bytes(),
        &t.picked.to_le_bytes(),
        t.initiator_pk,
        t.responder_pk,
    ] {
        crypto_generichash_update(&mut h, part);
    }
    let mut out = [0u8; 32];
    crypto_generichash_final(h, &mut out).map_err(failed)?;
    Ok(out)
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
            open_id_pk(&introduction, &server_pk, "123456789")
                .unwrap()
                .key,
            peer_pk
        );
        // The id in the signature is checked against the id that was dialled,
        // so a valid signature for another machine is refused.
        assert!(open_id_pk(&introduction, &server_pk, "987654321").is_err());

        let signed_id = sign(&box_pk, "123456789", &peer_sk);
        let their_box_pk = open_id_pk(&signed_id, &peer_pk, "123456789").unwrap().key;
        let (our_box_pk, sealed, key) = seal_session_key(&their_box_pk).unwrap();
        let mut opened = vec![0u8; sealed.len() - 16];
        crypto_box_open_easy(&mut opened, &sealed, &[0u8; 24], &our_box_pk, &box_sk).unwrap();
        assert_eq!(opened, key);
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Fixed bytes from hbb_common's `test_wire_vectors` (229b904, the commit
    /// RustDesk 1.5.0 builds with): the first version 0 frame, which released
    /// peers send, and version 1's two keys, for an advertisement equal to the
    /// pick and one above it. A change here breaks the wire, not just a test.
    #[test]
    fn the_keys_are_the_ones_rustdesk_derives() {
        let key = [0x11u8; 32];
        assert_eq!(
            hex(&Secretbox::new(key).seal(b"hello".to_vec()).unwrap()),
            "3b768f827fdcc4af555b9e42533be6611e2c333481"
        );
        let mut t = Transcript {
            initiator_pk: &[0x22u8; 32],
            responder_pk: &[0x33u8; 32],
            advertised: 1,
            picked: 1,
        };
        let keys = session_keys(&key, &t).unwrap();
        assert_eq!(
            hex(&keys.send),
            "c189ec6e1935c0751cfc85a0f075405cae507d7925bb19687caf56cbb54e0ecf"
        );
        assert_eq!(
            hex(&keys.receive),
            "8a62648e9195cb10ea900c0a24d2a166c1e982347a2dc6833c3e756bf5715d36"
        );
        // With the two versions equal, swapping their order in the derivation
        // would go unseen.
        t.advertised = 3;
        let keys = session_keys(&key, &t).unwrap();
        assert_eq!(
            hex(&keys.send),
            "25b0778ae600f9731c7fa457d1cc5d40071b0aa3d34c90b53e1d5344b0e7123a"
        );
        assert_eq!(
            hex(&keys.receive),
            "5bbde0ac6a708d4cd1ff807125d79cb6030bde62f7a66b6cc1a79e31bd657756"
        );
    }

    #[test]
    fn the_version_picked_is_the_highest_both_speak() {
        assert_eq!(kx_version_for(0), 0);
        assert_eq!(kx_version_for(1), 1);
        assert_eq!(kx_version_for(KX_VERSION_LATEST + 7), KX_VERSION_LATEST);
    }

    fn transcript(advertised: u32, picked: u32) -> Transcript<'static> {
        Transcript {
            initiator_pk: &[1u8; 32],
            responder_pk: &[2u8; 32],
            advertised,
            picked,
        }
    }

    /// Version 0 is the problem version 1 fixes: frame one in each direction is
    /// the same key and nonce, so the same payload seals to the same bytes.
    /// Version 1's directions differ, and neither key is the sealed one.
    #[test]
    fn version_1_seals_each_direction_with_its_own_key() {
        let key = [5u8; 32];
        let v0 = session_keys(&key, &transcript(0, 0)).unwrap();
        assert_eq!((v0.send, v0.receive), (key, key));
        assert_eq!(
            Secretbox::new(v0.send).seal(b"hello".to_vec()).unwrap(),
            Secretbox::new(v0.receive).seal(b"hello".to_vec()).unwrap()
        );

        let v1 = session_keys(&key, &transcript(1, 1)).unwrap();
        assert_ne!(v1.send, v1.receive);
        assert_ne!(v1.send, key);
        assert_ne!(v1.receive, key);
        let sent = Secretbox::new(v1.send).seal(b"hello".to_vec()).unwrap();
        let back = Secretbox::new(v1.receive).seal(b"hello".to_vec()).unwrap();
        assert_ne!(sent, back);
        // The peer receives under our sending key, and a version 0 peer, which
        // would use the sealed key, cannot open it.
        assert!(Secretbox::new(v1.send).open(sent.clone()).is_ok());
        assert!(Secretbox::new(key).open(sent).is_err());
    }

    /// Both versions and both box keys are in the derivation, so a pick or an
    /// advertisement changed in transit gives the two ends different keys.
    #[test]
    fn version_1_binds_the_transcript() {
        let key = [6u8; 32];
        let ours = session_keys(&key, &transcript(1, 1)).unwrap();
        assert_ne!(ours, session_keys(&key, &transcript(2, 1)).unwrap());
        let mut other = transcript(1, 1);
        other.responder_pk = &[3u8; 32];
        assert_ne!(ours, session_keys(&key, &other).unwrap());
        other = transcript(1, 1);
        other.initiator_pk = &[3u8; 32];
        assert_ne!(ours, session_keys(&key, &other).unwrap());
    }

    #[test]
    fn an_unknown_version_is_refused() {
        assert!(session_keys(&[0u8; 32], &transcript(2, 2)).is_err());
    }

    /// The advertisement is read from the signed bytes: field 4 of `IdPk`, and
    /// 0 when it is absent, as from a peer older than 1.5.0.
    #[test]
    fn the_advertised_version_is_inside_the_signature() {
        use dryoc::classic::crypto_sign::{crypto_sign, crypto_sign_keypair};

        let (pk, sk) = crypto_sign_keypair();
        let signed = |kx_version| {
            let mut id_pk = IdPk::new();
            id_pk.id = "123456789".into();
            id_pk.pk = vec![4u8; 32];
            id_pk.kx_version = kx_version;
            let message = id_pk.write_to_bytes().unwrap();
            let mut signed = vec![0u8; message.len() + SIGNATURE];
            crypto_sign(&mut signed, &message, &sk).unwrap();
            signed
        };
        let old = open_id_pk(&signed(0), &pk, "123456789").unwrap();
        assert_eq!(old.kx_version, 0);
        assert_eq!(old.key, [4u8; 32]);
        assert_eq!(
            open_id_pk(&signed(1), &pk, "123456789").unwrap().kx_version,
            1
        );
    }
}
