//! The encrypted store (I93), fit's `admin/store` byte for byte: a config kept sealed in a file
//! of fit's format, `fitstore`, which fit and akit both open.
//!
//! ```text
//! offset  bytes
//!  0       8   "FITSTORE"
//!  8       1   format version: 1 (the config alone), 2 (config and users), 3 (and sections)
//!  9       1   key derivation: 1, Argon2id
//! 10       4   Argon2id time, big endian
//! 14       4   Argon2id memory in KiB, big endian
//! 18       1   Argon2id threads (lanes)
//! 19      16   salt
//! 35      24   nonce
//! 59           the payload sealed with XChaCha20-Poly1305 (the header its additional data),
//!              then its 16-byte tag
//! ```
//!
//! The payload of versions 2 and 3 is JSON, `{"config": <the base config>, "users": [{"username",
//! "password_hash"}], "sections": {<name>: <JSON>}}`: the config as it was written, the console
//! users whose passwords changed, and named sections other parts of fit keep there (its
//! assistant's settings, a cluster's members), which akit does not read but keeps as they are, so
//! that a store fit wrote loses nothing when akit writes it. The key comes from the store's
//! password with Argon2id (akio-crypto's `kdf::argon2id`, akio's A230; fit writes time 3, 64 MiB
//! and 4 lanes, RFC 9106's second recommended setting) and seals with XChaCha20-Poly1305
//! (`chacha::XChaCha20Poly1305`, A231), a new random nonce for each write. The magic stays fit's,
//! `FITSTORE`, as the format is fit's.
//!
//! Opening a store derives its key: 64 MiB and a fair share of a second, and reading and writing
//! its file is disk I/O. All of it is for the control plane, never an I/O thread (rule 1).

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use akio_crypto::chacha::{KEY_LEN, TAG_LEN, XChaCha20Poly1305, XNONCE_LEN};
use akit_config::value::write_string;

const MAGIC: &[u8; 8] = b"FITSTORE";
/// The format of a store that keeps sections.
const VERSION_SECTIONS: u8 = 3;
/// The format of a store without sections, and of the files exported.
const VERSION: u8 = 2;
/// The first format, whose payload is the config alone; still read.
const VERSION_CONFIG: u8 = 1;
const KDF_ARGON2: u8 = 1;
const SALT_LEN: usize = 16;
/// The header's length: 59 bytes.
pub const HEADER_LEN: usize = MAGIC.len() + 1 + 1 + 4 + 4 + 1 + SALT_LEN + XNONCE_LEN;
const SALT_OFFSET: usize = HEADER_LEN - XNONCE_LEN - SALT_LEN;
const NONCE_OFFSET: usize = SALT_OFFSET + SALT_LEN;

/// The most bytes of a config (and of a store's payload) a store holds: 64 MiB, as fit's.
pub const MAX_BYTES: usize = 64 << 20;
/// The most Argon2id time a store's header may ask for.
const MAX_TIME: u32 = 16;
/// The most Argon2id memory a store's header may ask for: 1 GiB.
const MAX_MEMORY_KIB: u32 = 1 << 20;
/// The most a file [`unseal`] opens may ask for, as a multiple of [`DEFAULT_PARAMS`]' time,
/// memory and threads: an import sent to the Admin API may come from anyone who can call it.
pub const MAX_UNSEAL_FACTOR: u32 = 2;
/// The largest file read as a store: the largest payload, its header and its tag.
const MAX_FILE: usize = MAX_BYTES + HEADER_LEN + TAG_LEN;
/// The longest name of a section.
pub const MAX_SECTION_NAME: usize = 64;
/// The deepest a payload's JSON nests.
const MAX_DEPTH: usize = 512;

/// Argon2id's settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Params {
    /// The passes over the memory.
    pub time: u32,
    /// The memory, in KiB.
    pub memory_kib: u32,
    /// The lanes, as Go's `threads`.
    pub threads: u8,
}

/// What a new store is written with: fit's, RFC 9106's second recommended setting.
pub const DEFAULT_PARAMS: Params = Params {
    time: 3,
    memory_kib: 64 << 10,
    threads: 4,
};

/// A console user kept beside the config: the name and the bcrypt hash of the password that
/// replaced the one the bootstrap file gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    /// `username`.
    pub username: String,
    /// `password_hash`.
    pub password_hash: String,
}

/// Why a store did not open or was not written.
#[derive(Debug)]
pub enum Error {
    /// Not a `fitstore` file (fit's `ErrNotStore`).
    NotStore(String),
    /// The password is wrong, or the file was changed (fit's `ErrOpen`).
    Open(String),
    /// Anything else, said as fit says it.
    Other(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotStore(p) => write!(f, "store: not a fitstore file: {p}"),
            Error::Open(p) => write!(
                f,
                "store: the store does not open with the password: it is wrong, or the file was \
                 changed: {p}"
            ),
            Error::Other(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {}

fn other(msg: impl Into<String>) -> Error {
    Error::Other(msg.into())
}

fn io_error(e: &io::Error) -> Error {
    other(format!("store: {e}"))
}

/// An open store: its path, its password, and the key the password gives with its salt, kept so
/// that writes need no derivation; and what it holds, last read or written.
pub struct Store {
    path: PathBuf,
    password: String,
    params: Params,
    salt: [u8; SALT_LEN],
    aead: Option<XChaCha20Poly1305>,
    /// The base config last read or written, `None` before either.
    data: Option<Vec<u8>>,
    users: Vec<User>,
    /// The sections, each a JSON value as it was read.
    sections: BTreeMap<String, Vec<u8>>,
}

impl fmt::Debug for Store {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Store")
            .field("path", &self.path)
            .field("params", &self.params)
            .field("users", &self.users.len())
            .field("sections", &self.sections.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl Store {
    /// Opens the store at `path` with `password`: the store, and the base config it holds, or
    /// `None` when there is no file there yet (then the store has a new salt, and its first
    /// [`write`](Self::write) makes the file).
    ///
    /// # Errors
    ///
    /// An empty password, a file that is not a store, one that asks for more Argon2id cost than
    /// a store may, or that does not open with the password ([`Error::Open`]), or that cannot be
    /// read.
    pub fn open(path: impl AsRef<Path>, password: &str) -> Result<(Store, Option<Vec<u8>>), Error> {
        Store::open_with(path, password, DEFAULT_PARAMS)
    }

    /// [`open`](Self::open), a new store made with Argon2id's settings `params` (tests make them
    /// cheap); an existing file's own settings are its.
    ///
    /// # Errors
    ///
    /// As [`open`](Self::open)'s.
    pub fn open_with(
        path: impl AsRef<Path>,
        password: &str,
        params: Params,
    ) -> Result<(Store, Option<Vec<u8>>), Error> {
        let path = path.as_ref();
        if password.is_empty() {
            return Err(other("store: the password is empty"));
        }
        let file = match read_file(path) {
            Err(e) if matches!(&e, Failed::Io(io) if io.kind() == io::ErrorKind::NotFound) => {
                return Ok((Store::create_with(path, password, params)?, None));
            }
            Err(e) => return Err(e.into_error()),
            Ok(f) => f,
        };
        let mut s = Store::empty(path, password, DEFAULT_PARAMS);
        let data = s.open_file(&file)?;
        Ok((s, Some(data)))
    }

    /// A store at `path` with a new salt, whatever is there: its first write replaces it.
    ///
    /// # Errors
    ///
    /// An empty password, or no randomness for the salt.
    pub fn create(path: impl AsRef<Path>, password: &str) -> Result<Store, Error> {
        Store::create_with(path, password, DEFAULT_PARAMS)
    }

    /// [`create`](Self::create) with Argon2id's settings `params` (tests make them cheap).
    ///
    /// # Errors
    ///
    /// As [`create`](Self::create)'s, and settings Argon2id refuses.
    pub fn create_with(
        path: impl AsRef<Path>,
        password: &str,
        params: Params,
    ) -> Result<Store, Error> {
        if password.is_empty() {
            return Err(other("store: the password is empty"));
        }
        let mut s = Store::empty(path.as_ref(), password, params);
        akio_crypto::rand::fill_random(&mut s.salt).map_err(|e| io_error(&e))?;
        s.derive()?;
        Ok(s)
    }

    fn empty(path: &Path, password: &str, params: Params) -> Store {
        Store {
            path: path.to_owned(),
            password: password.to_owned(),
            params,
            salt: [0; SALT_LEN],
            aead: None,
            data: None,
            users: Vec::new(),
            sections: BTreeMap::new(),
        }
    }

    /// The store's file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reads the store's file again, as another process may have written it, and returns the
    /// base config it holds; the key is derived again only when the file's salt or settings are
    /// not the store's.
    ///
    /// # Errors
    ///
    /// As [`open`](Self::open)'s, a missing file included.
    pub fn read(&mut self) -> Result<Vec<u8>, Error> {
        let file = read_file(&self.path).map_err(Failed::into_error)?;
        self.open_file(&file)
    }

    /// Opens `file`, a store's whole file, deriving its key unless it is the one the store has,
    /// and keeps what it holds.
    fn open_file(&mut self, file: &[u8]) -> Result<Vec<u8>, Error> {
        let where_ = self.path.display().to_string();
        let h = parse_header(&where_, file)?;
        if self.aead.is_none() || h.params != self.params || h.salt != self.salt {
            self.params = h.params;
            self.salt = h.salt;
            self.derive()?;
        }
        let (Some((header, sealed)), Some(aead)) = (file.split_at_checked(HEADER_LEN), &self.aead)
        else {
            return Err(Error::Open(where_));
        };
        let mut nonce = [0u8; XNONCE_LEN];
        nonce.copy_from_slice(header.get(NONCE_OFFSET..).unwrap_or(&[0; XNONCE_LEN]));
        let plaintext = aead
            .decrypt(&nonce, header, sealed)
            .map_err(|_| Error::Open(where_.clone()))?;
        let p = decode(h.version, plaintext).ok_or_else(|| no_config(&where_))?;
        self.data = Some(p.config.clone());
        self.users = p.users;
        self.sections = p.sections;
        Ok(p.config)
    }

    fn derive(&mut self) -> Result<(), Error> {
        let mut key = [0u8; KEY_LEN];
        akio_crypto::kdf::argon2id(
            self.password.as_bytes(),
            &self.salt,
            self.params.time,
            self.params.memory_kib,
            self.params.threads,
            &mut key,
        )
        .map_err(|e| other(format!("store: deriving the key: {e}")))?;
        self.aead = Some(XChaCha20Poly1305::new(&key));
        key.fill(0);
        Ok(())
    }

    /// The console users kept beside the config.
    #[must_use]
    pub fn users(&self) -> &[User] {
        &self.users
    }

    /// Replaces the console users kept beside the config with `users`, and writes the store.
    ///
    /// # Errors
    ///
    /// A store whose config has not been read or written yet keeps no users; a write that
    /// fails changes nothing.
    pub fn set_users(&mut self, users: Vec<User>) -> Result<(), Error> {
        let Some(data) = self.data.clone() else {
            return Err(other(
                "store: the store holds no config yet, so it can keep no users",
            ));
        };
        let sections = self.sections.clone();
        self.write_all(data, users, sections)
    }

    /// The section kept under `name`, a JSON value as it was written; `None` when there is
    /// none.
    #[must_use]
    pub fn section(&self, name: &str) -> Option<&[u8]> {
        self.sections.get(name).map(Vec::as_slice)
    }

    /// The names of the sections the store keeps.
    pub fn section_names(&self) -> impl Iterator<Item = &str> {
        self.sections.keys().map(String::as_str)
    }

    /// Keeps `data`, a JSON value, as the section `name`, or removes the section when `data` is
    /// empty, and writes the store.
    ///
    /// # Errors
    ///
    /// A name of 0 or more than [`MAX_SECTION_NAME`] bytes, data that is not JSON, a store whose
    /// config has not been read or written yet, a write that fails (which changes nothing).
    pub fn set_section(&mut self, name: &str, data: &[u8]) -> Result<(), Error> {
        if name.is_empty() || name.len() > MAX_SECTION_NAME {
            return Err(other(format!(
                "store: a section's name is 1 to {MAX_SECTION_NAME} bytes"
            )));
        }
        if !data.is_empty() && !json::valid(data) {
            return Err(other(format!("store: the section {name:?} is not JSON")));
        }
        let Some(config) = self.data.clone() else {
            return Err(other(
                "store: the store holds no config yet, so it can keep no sections",
            ));
        };
        let mut sections = self.sections.clone();
        if data.is_empty() {
            sections.remove(name);
        } else {
            sections.insert(name.to_owned(), data.to_vec());
        }
        let users = self.users.clone();
        self.write_all(config, users, sections)
    }

    /// Seals `data` with a new nonce and replaces the store's file with it, beside the users and
    /// sections the store keeps: written to a temporary file beside it, made 0600, synced and
    /// renamed over it, so that the store is either the old one or the new one.
    ///
    /// # Errors
    ///
    /// A config that is not JSON or is larger than [`MAX_BYTES`], or a file that cannot be
    /// written; the store's file is as it was then.
    pub fn write(&mut self, data: Vec<u8>) -> Result<(), Error> {
        let users = self.users.clone();
        let sections = self.sections.clone();
        self.write_all(data, users, sections)
    }

    fn write_all(
        &mut self,
        data: Vec<u8>,
        users: Vec<User>,
        sections: BTreeMap<String, Vec<u8>>,
    ) -> Result<(), Error> {
        if data.len() > MAX_BYTES {
            return Err(other(format!(
                "store: the config is larger than {MAX_BYTES} bytes"
            )));
        }
        let (plaintext, version) = encode(&data, &users, &sections)?;
        if plaintext.len() > MAX_BYTES {
            return Err(other(format!(
                "store: the config, its users and its sections are larger than {MAX_BYTES} bytes"
            )));
        }
        let out = self.seal_payload(version, plaintext)?;
        write_atomically(&self.path, &out)?;
        self.data = Some(data);
        self.users = users;
        self.sections = sections;
        Ok(())
    }

    /// A `fitstore` file holding `data`, a base config as JSON, and `users`, sealed with the
    /// store's own key, as an export of it that opens with the store's password; the store's
    /// file is not touched. Its sections are not in it: they hold secrets that stay with the
    /// store.
    ///
    /// # Errors
    ///
    /// A config that is not JSON, or no randomness for the nonce.
    pub fn seal(&self, data: &[u8], users: &[User]) -> Result<Vec<u8>, Error> {
        let (plaintext, version) = encode(data, users, &BTreeMap::new())?;
        self.seal_payload(version, plaintext)
    }

    /// Opens `file`, a whole `fitstore` file such as an export, with the store's password: the
    /// base config and the users it holds, not its sections. The store is not touched.
    ///
    /// # Errors
    ///
    /// As [`unseal`]'s.
    pub fn unseal(&self, file: &[u8]) -> Result<(Vec<u8>, Vec<User>), Error> {
        unseal(file, &self.password)
    }

    /// What seals and opens files with the store's key and password apart from the store
    /// ([`Sealer`]); `None` before its key is derived, which [`open`](Self::open) and
    /// [`create`](Self::create) do.
    #[must_use]
    pub fn sealer(&self) -> Option<Sealer> {
        Some(Sealer {
            password: self.password.clone(),
            params: self.params,
            salt: self.salt,
            aead: self.aead.clone()?,
        })
    }

    /// The store's file for `plaintext`, as a file of version `version`: the header, then the
    /// plaintext sealed.
    fn seal_payload(&self, version: u8, plaintext: Vec<u8>) -> Result<Vec<u8>, Error> {
        let Some(aead) = &self.aead else {
            return Err(other("store: no key"));
        };
        seal_payload(aead, self.params, &self.salt, version, plaintext)
    }
}

/// A store's key, its salt and Argon2id settings, and its password, apart from the store: what
/// seals the files the Admin API exports with the store's key, and opens the files it imports
/// with the store's password ([`Store::sealer`]), on a pool while the store is written
/// elsewhere. Shown, it shows none of them.
#[derive(Clone)]
pub struct Sealer {
    password: String,
    params: Params,
    salt: [u8; SALT_LEN],
    aead: XChaCha20Poly1305,
}

impl fmt::Debug for Sealer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sealer")
            .field("params", &self.params)
            .finish_non_exhaustive()
    }
}

impl Sealer {
    /// A `fitstore` file holding `data` and `users`, sealed with the store's key, as
    /// [`Store::seal`]: it opens with the store's password. No key is derived.
    ///
    /// # Errors
    ///
    /// A config that is not JSON, or no randomness for the nonce.
    pub fn seal(&self, data: &[u8], users: &[User]) -> Result<Vec<u8>, Error> {
        let (plaintext, version) = encode(data, users, &BTreeMap::new())?;
        seal_payload(&self.aead, self.params, &self.salt, version, plaintext)
    }

    /// Opens `file`, a whole `fitstore` file, with the store's password, as [`unseal`]: a key
    /// is derived with the file's salt.
    ///
    /// # Errors
    ///
    /// As [`unseal`]'s.
    pub fn unseal(&self, file: &[u8]) -> Result<(Vec<u8>, Vec<User>), Error> {
        unseal(file, &self.password)
    }
}

/// A file of version `version` holding `plaintext`, sealed with `aead`, the key of `params` and
/// `salt`, under a new nonce: the header, then the plaintext sealed.
fn seal_payload(
    aead: &XChaCha20Poly1305,
    params: Params,
    salt: &[u8; SALT_LEN],
    version: u8,
    mut plaintext: Vec<u8>,
) -> Result<Vec<u8>, Error> {
    let mut nonce = [0u8; XNONCE_LEN];
    akio_crypto::rand::fill_random(&mut nonce).map_err(|e| io_error(&e))?;
    let mut header = Vec::with_capacity(
        HEADER_LEN
            .saturating_add(plaintext.len())
            .saturating_add(TAG_LEN),
    );
    header.extend_from_slice(MAGIC);
    header.push(version);
    header.push(KDF_ARGON2);
    header.extend_from_slice(&params.time.to_be_bytes());
    header.extend_from_slice(&params.memory_kib.to_be_bytes());
    header.push(params.threads);
    header.extend_from_slice(salt);
    header.extend_from_slice(&nonce);
    aead.encrypt_in_place(&nonce, &header, &mut plaintext);
    header.append(&mut plaintext);
    Ok(header)
}

/// A new random password, for the key file kept beside a store `admin.store` does not describe
/// (I93c): 52 characters of RFC 4648's base32 alphabet, 260 bits, as fit's `rand.Text() +
/// rand.Text()`.
///
/// # Errors
///
/// No randomness.
pub fn new_key() -> io::Result<String> {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut b = [0u8; 52];
    akio_crypto::rand::fill_random(&mut b)?;
    let key = b
        .iter()
        .filter_map(|x| ALPHABET.get(usize::from(x & 31)).map(|c| char::from(*c)))
        .collect();
    b.fill(0);
    Ok(key)
}

/// A `fitstore` file holding `data` and `users`, sealed with a key `password` gives with a new
/// salt, as a new store's first write would make it.
///
/// # Errors
///
/// An empty password, a config that is not JSON.
pub fn seal(password: &str, data: &[u8], users: &[User]) -> Result<Vec<u8>, Error> {
    seal_with(password, data, users, DEFAULT_PARAMS)
}

/// [`seal`] with Argon2id's settings `params`.
///
/// # Errors
///
/// As [`seal`]'s.
pub fn seal_with(
    password: &str,
    data: &[u8],
    users: &[User],
    params: Params,
) -> Result<Vec<u8>, Error> {
    Store::create_with("", password, params)?.seal(data, users)
}

/// Opens `file`, a whole `fitstore` file, with `password`: the base config and the users it
/// holds. A file that asks for more than [`MAX_UNSEAL_FACTOR`] times [`DEFAULT_PARAMS`]'
/// Argon2id cost is refused before any key is derived.
///
/// # Errors
///
/// An empty password, a file larger than a store may be, not a store, asking for too much, or
/// that does not open with the password ([`Error::Open`]).
pub fn unseal(file: &[u8], password: &str) -> Result<(Vec<u8>, Vec<User>), Error> {
    if password.is_empty() {
        return Err(other("store: the password is empty"));
    }
    if file.len() > MAX_FILE {
        return Err(other(format!(
            "store: the file is larger than {MAX_BYTES} bytes"
        )));
    }
    let h = parse_header("the file", file)?;
    let (p, d, f) = (h.params, DEFAULT_PARAMS, MAX_UNSEAL_FACTOR);
    if p.time > f.saturating_mul(d.time)
        || p.memory_kib > f.saturating_mul(d.memory_kib)
        || u32::from(p.threads) > f.saturating_mul(u32::from(d.threads))
    {
        return Err(other(format!(
            "store: the file asks for Argon2id time {}, {} KiB and {} threads, more than {f} \
             times what akit writes ({}, {} KiB, {})",
            p.time, p.memory_kib, p.threads, d.time, d.memory_kib, d.threads
        )));
    }
    let mut s = Store::empty(Path::new("the file"), password, h.params);
    let data = s.open_file(file)?;
    Ok((data, s.users))
}

/// What a store's header says.
struct Header {
    version: u8,
    params: Params,
    salt: [u8; SALT_LEN],
}

fn parse_header(path: &str, file: &[u8]) -> Result<Header, Error> {
    let not_store = || Error::NotStore(path.to_owned());
    if file.len() < HEADER_LEN + TAG_LEN || !file.starts_with(MAGIC) {
        return Err(not_store());
    }
    let h: &[u8; HEADER_LEN] = file
        .get(..HEADER_LEN)
        .and_then(|h| h.try_into().ok())
        .ok_or_else(not_store)?;
    let version = h[8];
    if !matches!(version, VERSION_CONFIG | VERSION | VERSION_SECTIONS) {
        return Err(other(format!(
            "store: {path} is fitstore version {version}; this akit reads versions \
             {VERSION_CONFIG} to {VERSION_SECTIONS}"
        )));
    }
    if h[9] != KDF_ARGON2 {
        return Err(other(format!(
            "store: {path} derives its key by method {}, which this akit does not know",
            h[9]
        )));
    }
    let params = Params {
        time: u32::from_be_bytes([h[10], h[11], h[12], h[13]]),
        memory_kib: u32::from_be_bytes([h[14], h[15], h[16], h[17]]),
        threads: h[18],
    };
    if params.time == 0
        || params.time > MAX_TIME
        || params.threads == 0
        || params.memory_kib < 8u32.saturating_mul(u32::from(params.threads))
        || params.memory_kib > MAX_MEMORY_KIB
    {
        return Err(other(format!(
            "store: {path} asks for Argon2id time {}, {} KiB and {} threads, past what akit \
             takes",
            params.time, params.memory_kib, params.threads
        )));
    }
    let mut salt = [0u8; SALT_LEN];
    salt.copy_from_slice(&h[SALT_OFFSET..NONCE_OFFSET]);
    Ok(Header {
        version,
        params,
        salt,
    })
}

fn no_config(path: &str) -> Error {
    other(format!("store: {path} holds no config"))
}

/// What a store of version 2 or 3 holds.
struct Payload {
    config: Vec<u8>,
    users: Vec<User>,
    sections: BTreeMap<String, Vec<u8>>,
}

/// What a store of version `version` holds, `plaintext` being what its key seals; `None` when it
/// holds no config (fit's "holds no config").
fn decode(version: u8, plaintext: Vec<u8>) -> Option<Payload> {
    if version == VERSION_CONFIG {
        return Some(Payload {
            config: plaintext,
            users: Vec::new(),
            sections: BTreeMap::new(),
        });
    }
    let members = json::members(&plaintext)?;
    // As Go's encoding/json: a field's name matched exactly, or else without case; the last
    // of a name wins.
    let field = |name: &str| {
        members
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .or_else(|| {
                members
                    .iter()
                    .rev()
                    .find(|(k, _)| k.eq_ignore_ascii_case(name))
            })
            .map(|(_, v)| v.clone())
    };
    let config = plaintext.get(field("config")?)?.to_vec();
    // `null`, as Go's RawMessage takes it, is no config.
    if config.is_empty() || config == b"null" {
        return None;
    }
    let users = match field("users") {
        None => Vec::new(),
        Some(r) => users(plaintext.get(r)?)?,
    };
    let mut sections = BTreeMap::new();
    if version == VERSION_SECTIONS {
        if let Some(r) = field("sections") {
            let raw = plaintext.get(r)?;
            if raw != b"null" {
                for (k, v) in json::members(raw)? {
                    sections.insert(k, raw.get(v)?.to_vec());
                }
            }
        }
    }
    Some(Payload {
        config,
        users,
        sections,
    })
}

/// The users of a payload, `raw` its `users` value: `null` or a list of objects with string
/// `username` and `password_hash`.
fn users(raw: &[u8]) -> Option<Vec<User>> {
    use akit_config::value::{Value, parse};
    let text = std::str::from_utf8(raw).ok()?;
    match parse(text).ok()? {
        Value::Null => Some(Vec::new()),
        Value::Array(items) => items
            .iter()
            .map(|u| {
                let get = |k: &str| -> Option<String> {
                    match u.get(k) {
                        None | Some(Value::Null) => Some(String::new()),
                        Some(Value::String(s)) => Some(s.clone()),
                        Some(_) => None,
                    }
                };
                matches!(u, Value::Object(_) | Value::Null).then_some(())?;
                Some(User {
                    username: get("username")?,
                    password_hash: get("password_hash")?,
                })
            })
            .collect(),
        _ => None,
    }
}

/// The payload of a store and its version: the config as it was written, the users beside it,
/// and the sections, which make it version 3.
fn encode(
    data: &[u8],
    users: &[User],
    sections: &BTreeMap<String, Vec<u8>>,
) -> Result<(Vec<u8>, u8), Error> {
    if !json::valid(data) {
        return Err(other("store: the config is not JSON"));
    }
    let mut b = Vec::with_capacity(
        data.len()
            .saturating_add(32)
            .saturating_add(users.len().saturating_mul(128)),
    );
    b.extend_from_slice(br#"{"config":"#);
    b.extend_from_slice(data);
    if !users.is_empty() {
        b.extend_from_slice(br#","users":["#);
        for (i, u) in users.iter().enumerate() {
            if i > 0 {
                b.push(b',');
            }
            let mut s = String::from("{\"username\":");
            write_string(&mut s, &u.username);
            s.push_str(",\"password_hash\":");
            write_string(&mut s, &u.password_hash);
            s.push('}');
            b.extend_from_slice(s.as_bytes());
        }
        b.push(b']');
    }
    let mut version = VERSION;
    if !sections.is_empty() {
        b.extend_from_slice(br#","sections":{"#);
        for (i, (name, v)) in sections.iter().enumerate() {
            if i > 0 {
                b.push(b',');
            }
            let mut s = String::new();
            write_string(&mut s, name);
            b.extend_from_slice(s.as_bytes());
            b.push(b':');
            b.extend_from_slice(v);
        }
        b.push(b'}');
        version = VERSION_SECTIONS;
    }
    b.push(b'}');
    Ok((b, version))
}

/// Why a file was not read.
enum Failed {
    Io(io::Error),
    Error(Error),
}

impl Failed {
    fn into_error(self) -> Error {
        match self {
            Failed::Io(e) => io_error(&e),
            Failed::Error(e) => e,
        }
    }
}

/// The whole file at `path`, at most a store's largest.
fn read_file(path: &Path) -> Result<Vec<u8>, Failed> {
    let f = File::open(path).map_err(Failed::Io)?;
    let mut data = Vec::new();
    f.take(
        u64::try_from(MAX_FILE)
            .unwrap_or(u64::MAX)
            .saturating_add(1),
    )
    .read_to_end(&mut data)
    .map_err(Failed::Io)?;
    if data.len() > MAX_FILE {
        return Err(Failed::Error(other(format!(
            "store: {} is larger than {MAX_BYTES} bytes",
            path.display()
        ))));
    }
    Ok(data)
}

/// Replaces the file at `path` with `data`: written to a temporary file beside it, made 0600,
/// synced and renamed over it, then its directory synced, so that the file is either the old one
/// or the new one, and stays the new one through a power loss.
fn write_atomically(path: &Path, data: &[u8]) -> Result<(), Error> {
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_owned(),
        _ => PathBuf::from("."),
    };
    let base = path
        .file_name()
        .map_or_else(|| "store".into(), |n| n.to_string_lossy().into_owned());
    let mut suffix = [0u8; 8];
    akio_crypto::rand::fill_random(&mut suffix).map_err(|e| io_error(&e))?;
    let suffix: String = suffix.iter().map(|b| format!("{b:02x}")).collect();
    let tmp = dir.join(format!(".{base}.{suffix}"));
    let written = (|| -> io::Result<()> {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(io_error(&e));
    }
    // The rename made durable; a directory that cannot be synced is no reason to fail.
    if let Ok(d) = File::open(&dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// Strict JSON (RFC 8259), as Go's `json.Valid` takes it, and the spans of an object's members.
/// Offsets are bounded by the input's length, so the position arithmetic below cannot overflow.
#[allow(clippy::arithmetic_side_effects)]
mod json {
    use std::ops::Range;

    /// Whether `b` is one JSON value, with only white space around it.
    pub(super) fn valid(b: &[u8]) -> bool {
        let start = skip_ws(b, 0);
        match value(b, start, 0) {
            Some(end) => skip_ws(b, end) == b.len(),
            None => false,
        }
    }

    /// The members of `b`, a JSON object: each name, unescaped, and its value's span in `b`;
    /// `None` when `b` is not one valid object.
    pub(super) fn members(b: &[u8]) -> Option<Vec<(String, Range<usize>)>> {
        if !valid(b) {
            return None;
        }
        let mut i = skip_ws(b, 0);
        if b.get(i) != Some(&b'{') {
            return None;
        }
        i = skip_ws(b, i + 1);
        let mut out = Vec::new();
        if b.get(i) == Some(&b'}') {
            return Some(out);
        }
        loop {
            let end = string(b, i)?;
            let name = unescape(b.get(i..end)?)?;
            i = skip_ws(b, end);
            i = skip_ws(b, i + 1); // the ':'
            let v_end = value(b, i, 1)?;
            out.push((name, i..v_end));
            i = skip_ws(b, v_end);
            match b.get(i) {
                Some(b',') => i = skip_ws(b, i + 1),
                Some(b'}') => return Some(out),
                _ => return None,
            }
        }
    }

    fn skip_ws(b: &[u8], mut i: usize) -> usize {
        while matches!(b.get(i), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            i += 1;
        }
        i
    }

    /// The end of the value starting at `i`, at nesting `depth`.
    fn value(b: &[u8], i: usize, depth: usize) -> Option<usize> {
        if depth > super::MAX_DEPTH {
            return None;
        }
        match b.get(i)? {
            b'{' => {
                let mut i = skip_ws(b, i + 1);
                if b.get(i) == Some(&b'}') {
                    return Some(i + 1);
                }
                loop {
                    i = string(b, i)?;
                    i = skip_ws(b, i);
                    if b.get(i) != Some(&b':') {
                        return None;
                    }
                    i = value(b, skip_ws(b, i + 1), depth + 1)?;
                    i = skip_ws(b, i);
                    match b.get(i)? {
                        b',' => i = skip_ws(b, i + 1),
                        b'}' => return Some(i + 1),
                        _ => return None,
                    }
                }
            }
            b'[' => {
                let mut i = skip_ws(b, i + 1);
                if b.get(i) == Some(&b']') {
                    return Some(i + 1);
                }
                loop {
                    i = value(b, i, depth + 1)?;
                    i = skip_ws(b, i);
                    match b.get(i)? {
                        b',' => i = skip_ws(b, i + 1),
                        b']' => return Some(i + 1),
                        _ => return None,
                    }
                }
            }
            b'"' => string(b, i),
            b't' => literal(b, i, b"true"),
            b'f' => literal(b, i, b"false"),
            b'n' => literal(b, i, b"null"),
            b'-' | b'0'..=b'9' => number(b, i),
            _ => None,
        }
    }

    fn literal(b: &[u8], i: usize, word: &[u8]) -> Option<usize> {
        b.get(i..)?.starts_with(word).then_some(i + word.len())
    }

    fn digits(b: &[u8], mut i: usize) -> (usize, usize) {
        let start = i;
        while matches!(b.get(i), Some(b'0'..=b'9')) {
            i += 1;
        }
        (i, i - start)
    }

    fn number(b: &[u8], mut i: usize) -> Option<usize> {
        if b.get(i) == Some(&b'-') {
            i += 1;
        }
        match b.get(i)? {
            b'0' => i += 1,
            b'1'..=b'9' => i = digits(b, i).0,
            _ => return None,
        }
        if b.get(i) == Some(&b'.') {
            let (end, n) = digits(b, i + 1);
            if n == 0 {
                return None;
            }
            i = end;
        }
        if matches!(b.get(i), Some(b'e' | b'E')) {
            i += 1;
            if matches!(b.get(i), Some(b'+' | b'-')) {
                i += 1;
            }
            let (end, n) = digits(b, i);
            if n == 0 {
                return None;
            }
            i = end;
        }
        Some(i)
    }

    /// The end of the string starting at `i` (past its closing quote).
    fn string(b: &[u8], i: usize) -> Option<usize> {
        if b.get(i) != Some(&b'"') {
            return None;
        }
        let mut i = i + 1;
        loop {
            match *b.get(i)? {
                b'"' => return Some(i + 1),
                b'\\' => match b.get(i + 1)? {
                    b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => i += 2,
                    b'u' => {
                        let hex = b.get(i + 2..i + 6)?;
                        if !hex.iter().all(u8::is_ascii_hexdigit) {
                            return None;
                        }
                        i += 6;
                    }
                    _ => return None,
                },
                c if c < 0x20 => return None,
                _ => i += 1,
            }
        }
    }

    /// The text of `s`, a JSON string with its quotes.
    fn unescape(s: &[u8]) -> Option<String> {
        let text = std::str::from_utf8(s).ok()?;
        match akit_config::value::parse(text).ok()? {
            akit_config::value::Value::String(v) => Some(v),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
