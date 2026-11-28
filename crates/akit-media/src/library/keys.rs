//! A library's keys of encrypted HLS (I147, fit's `medialib/keys.go`): an AES-128 key for each
//! resource, an item of `vod` or a channel, and each epoch of a channel's rotation, derived from
//! the library's secret, so that nothing is stored and every handler of the library derives the
//! same, fit's and akit's alike. A key is
//!
//! ```text
//! HMAC-SHA256(K, "hls aes-128" 0 kind 0 resource 0 epoch)[:16]
//! ```
//!
//! where K is HMAC-SHA256(secret, "fit media library" 0 name), the library's own: its keys open
//! nothing of another library's, nor any of `media_auth`'s tokens. The secret never leaves them,
//! nor reaches a log.

use akio_crypto::aes::Aes;
use akio_crypto::hash::Sha256;
use akio_crypto::mac::Hmac;
use akit_config::medialib::MediaLibrary;

/// The kind of an item of `vod`: one key, epoch 0.
pub const KEY_ITEM: &str = "item";
/// The kind of a channel's HLS, a key an epoch.
pub const KEY_CHANNEL: &str = "channel";

/// A library's keys.
#[derive(Clone, PartialEq, Eq)]
pub struct Keys {
    mac: [u8; 32],
    id: String,
}

impl std::fmt::Debug for Keys {
    /// The ID alone: the keys stay hidden from whatever prints them.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Keys({})", self.id)
    }
}

impl Keys {
    /// The keys of library `name` derived from `secret` (fit's `NewKeys`).
    ///
    /// # Errors
    ///
    /// A secret shorter than [`akit_config::medialib::MIN_SECRET`].
    pub fn new(name: &str, secret: &str) -> Result<Keys, String> {
        if secret.len() < akit_config::medialib::MIN_SECRET {
            return Err(format!(
                "the secret is {} bytes: use at least {}",
                secret.len(),
                akit_config::medialib::MIN_SECRET
            ));
        }
        let mut m = Hmac::<Sha256>::new(secret.as_bytes());
        m.update(b"fit media library\x00");
        m.update(name.as_bytes());
        let mac = m.finalize();
        // The ID tells keys apart, in ETags and logs, and says nothing of them.
        let mut id = Hmac::<Sha256>::new(&mac);
        id.update(b"key id");
        let id = super::catalog::hex(id.finalize().get(..6).unwrap_or_default());
        Ok(Keys { mac, id })
    }

    /// The keys of library `c`, its `secret_file` read now; `None` when it has no secret (fit's
    /// `KeysOf`). It reads the disk: the control plane's.
    ///
    /// # Errors
    ///
    /// Both set, a file that cannot be read, a secret too short.
    pub fn of(c: &MediaLibrary) -> Result<Option<Keys>, String> {
        match (c.secret.is_empty(), c.secret_file.is_empty()) {
            (false, false) => Err("set secret or secret_file, one of the two".into()),
            (false, true) => Keys::new(&c.name, &c.secret).map(Some),
            (true, false) => {
                let data = std::fs::read(&c.secret_file)
                    .map_err(|e| format!("secret_file: {}: {e}", c.secret_file))?;
                let s = String::from_utf8_lossy(&data);
                Keys::new(&c.name, s.trim_end_matches(['\r', '\n']))
                    .map(Some)
                    .map_err(|e| format!("secret_file {}: {e}", c.secret_file))
            }
            (true, true) => Ok(None),
        }
    }

    /// The AES-128 key of `resource` of `kind` at `epoch`: an item's ID with [`KEY_ITEM`], a
    /// channel's with [`KEY_CHANNEL`].
    #[must_use]
    pub fn key(&self, kind: &str, resource: &str, epoch: u64) -> [u8; 16] {
        let mut m = Hmac::<Sha256>::new(&self.mac);
        m.update(b"hls aes-128\x00");
        m.update(kind.as_bytes());
        m.update(b"\x00");
        m.update(resource.as_bytes());
        m.update(b"\x00");
        m.update(&epoch.to_be_bytes());
        let mut out = [0u8; 16];
        if let Some(k) = m.finalize().get(..16) {
            out.copy_from_slice(k);
        }
        out
    }

    /// A short ID of the keys, the same for the same library and secret: a fingerprint that
    /// tells keys apart without saying anything of them.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }
}

/// Encrypts `segment`, a media segment of HLS whose media sequence number is `seq`, with `key`,
/// as RFC 8216 section 4.3.2.4 has `METHOD=AES-128`: AES-128 in CBC mode with PKCS#7 padding,
/// its IV `seq` as a 128-bit big-endian number (fit's `Encrypt`).
#[must_use]
pub fn encrypt(key: &[u8; 16], seq: u64, segment: &[u8]) -> Vec<u8> {
    let aes = Aes::new128(key);
    let iv = u128::from(seq).to_be_bytes();
    let mut out = Vec::with_capacity(segment.len().saturating_add(16));
    aes.encrypt_cbc_padded(&iv, segment, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// fit's `TestKeys`, with the key fit derives for the same secret, library and item
    /// (`keys-vector.txt`, made with fit's `NewKeys`).
    #[test]
    fn keys() {
        let k = Keys::new("films", "0123456789abcdef-secret").unwrap();
        assert!(Keys::new("films", "short").is_err());
        assert_eq!(k, Keys::new("films", "0123456789abcdef-secret").unwrap());
        assert_ne!(
            k.id(),
            Keys::new("other", "0123456789abcdef-secret").unwrap().id()
        );
        assert_ne!(k.key(KEY_ITEM, "a", 0), k.key(KEY_ITEM, "b", 0));
        assert_ne!(k.key(KEY_CHANNEL, "a", 0), k.key(KEY_CHANNEL, "a", 1));
        assert!(!format!("{k:?}").contains(&super::super::catalog::hex(&k.mac)));
        let want = include_str!("keys-vector.txt").trim();
        let got = format!(
            "{} {} {}",
            k.id(),
            super::super::catalog::hex(&k.key(KEY_ITEM, "films/bbb", 0)),
            super::super::catalog::hex(&k.key(KEY_CHANNEL, "tv/films/s1", 7))
        );
        assert_eq!(got, want);
        // AES-128-CBC with PKCS#7: a whole block of padding for a whole block.
        let e = encrypt(&[7u8; 16], 3, &[1u8; 32]);
        assert_eq!(e.len(), 48);
        let mut back = e.clone();
        Aes::new128(&[7u8; 16])
            .decrypt_cbc_padded(&u128::from(3u64).to_be_bytes(), &mut back)
            .unwrap();
        assert_eq!(back, vec![1u8; 32]);
    }
}
