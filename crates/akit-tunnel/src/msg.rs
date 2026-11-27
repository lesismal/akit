//! fit's `tunnel/internal/proto` (decision 038): the methods, the messages they carry as JSON with
//! fit's field names, the password's registration MAC and the hub's proof, and the frames of a
//! tunnel. What akit writes, fit's `encoding/json` reads, and the other way round.

use akio_crypto::hash::Sha256;
use akio_crypto::mac::Hmac;
use akit_config::value::{Value, write_string};

/// A peer's type: a node.
pub const TYPE_NODE: &str = "node";
/// A peer's type: a proxy.
pub const TYPE_PROXY: &str = "proxy";

/// The methods, one byte on the wire.
pub mod method {
    /// Authenticates and registers a link.
    pub const REGISTER: u8 = 1;
    /// Replaces what a peer told about itself.
    pub const UPDATE: u8 = 2;
    /// The nodes of a hub; also notified to proxies.
    pub const NODES: u8 = 3;
    /// Has the hub set a node's services.
    pub const SET_NODE_SERVICES: u8 = 4;
    /// Measures a peer's link.
    pub const PING: u8 = 5;
    /// The hub closes a peer's links, with the reason.
    pub const KICK: u8 = 6;
    /// Starts a registration with a token.
    pub const CHALLENGE: u8 = 7;
    /// A node's Info.
    pub const NODE_INFO: u8 = 16;
    /// Sets a node's services.
    pub const NODE_SET_SERVICES: u8 = 17;
    /// Opens a tunnel.
    pub const TUNNEL_OPEN: u8 = 32;
    /// A frame of a tunnel's bytes.
    pub const TUNNEL_DATA: u8 = 33;
    /// Acknowledges bytes of a tunnel.
    pub const TUNNEL_ACK: u8 = 34;
    /// Closes a tunnel, with the reason.
    pub const TUNNEL_CLOSE: u8 = 35;
}

/// How many bytes one end of a tunnel may send that the other has not acknowledged.
pub const WINDOW: i64 = 4 << 20;
/// How many bytes an end lets arrive before it acknowledges them.
pub const ACK_EVERY: i64 = 256 << 10;
/// The most bytes one data frame carries.
pub const MAX_CHUNK: usize = 64 << 10;
/// The most bytes of one end that may be unacknowledged: its window and one read past it.
pub const MAX_IN_FLIGHT: i64 = WINDOW + (1 << 20);
/// The most tunnels one link carries at once.
pub const MAX_TUNNELS_PER_LINK: usize = 64;
/// What a link may have waiting to be sent (fit's `proto.LinkQueue`): the windows of its tunnels,
/// and room for the control messages beside them. What waits on a link is bounded by its tunnels'
/// windows, not by a high water of its own.
pub const LINK_QUEUE: usize = MAX_TUNNELS_PER_LINK * (4 << 20) + (4 << 20);
// `LINK_QUEUE` counts windows of `WINDOW` bytes.
const _: () = assert!(WINDOW == 4 << 20);

/// The version every peer reports: akit's (decision 038's point 5).
#[must_use]
pub fn version() -> String {
    format!("akit {}", env!("CARGO_PKG_VERSION"))
}

/// A string member.
fn s(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

/// A number member, as an integer; 0 when it is not one.
fn n(v: &Value, k: &str) -> i64 {
    match v.get(k) {
        Some(Value::Number(n)) => n.parse::<i64>().ok().unwrap_or(0),
        _ => 0,
    }
}

fn f(v: &Value, k: &str) -> f64 {
    match v.get(k) {
        Some(Value::Number(n)) => n.parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// Parses a message's body: `None` for a body that is not a JSON object (an empty body is an
/// empty object, as fit's `Decode` takes it).
#[must_use]
pub fn parse(body: &[u8]) -> Option<Value> {
    if body.is_empty() {
        return Some(Value::Object(Vec::new()));
    }
    let text = std::str::from_utf8(body).ok()?;
    akit_config::value::parse(text).ok()
}

/// A JSON object written member by member, as Go's `encoding/json` writes a struct.
#[derive(Debug, Default)]
pub struct Obj(String);

impl Obj {
    /// An empty object.
    #[must_use]
    pub fn new() -> Obj {
        Obj(String::from("{"))
    }
    fn key(&mut self, k: &str) {
        if self.0.len() > 1 {
            self.0.push(',');
        }
        write_string(&mut self.0, k);
        self.0.push(':');
    }
    /// A string member.
    #[must_use]
    pub fn str(mut self, k: &str, v: &str) -> Obj {
        self.key(k);
        write_string(&mut self.0, v);
        self
    }
    /// A string member left out when empty (`omitempty`).
    #[must_use]
    pub fn str_omit(self, k: &str, v: &str) -> Obj {
        if v.is_empty() { self } else { self.str(k, v) }
    }
    /// A number member.
    #[must_use]
    pub fn num(mut self, k: &str, v: impl std::fmt::Display) -> Obj {
        self.key(k);
        self.0.push_str(&v.to_string());
        self
    }
    /// A number member left out when zero.
    #[must_use]
    pub fn num_omit(self, k: &str, v: i64) -> Obj {
        if v == 0 { self } else { self.num(k, v) }
    }
    /// A float member, as Go writes one.
    #[must_use]
    pub fn float(self, k: &str, v: f64) -> Obj {
        let text = if v.is_finite() {
            format!("{v}")
        } else {
            "0".to_owned()
        };
        self.raw(k, &text)
    }
    /// A boolean member.
    #[must_use]
    pub fn bool(self, k: &str, v: bool) -> Obj {
        self.raw(k, if v { "true" } else { "false" })
    }
    /// A boolean member, left out when false (Go's `omitempty`).
    #[must_use]
    pub fn bool_omit(self, k: &str, v: bool) -> Obj {
        if v { self.raw(k, "true") } else { self }
    }
    /// A member already written as JSON.
    #[must_use]
    pub fn raw(mut self, k: &str, json: &str) -> Obj {
        self.key(k);
        self.0.push_str(json);
        self
    }
    /// The object's JSON.
    #[must_use]
    pub fn end(mut self) -> String {
        self.0.push('}');
        self.0
    }
}

/// `s` as a JSON string, quoted and escaped as Go's `encoding/json` writes it.
#[must_use]
pub fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len().saturating_add(2));
    write_string(&mut out, s);
    out
}

/// A JSON array of what `items` write.
#[must_use]
pub fn array<T>(items: &[T], each: impl Fn(&T) -> String) -> String {
    let mut out = String::from("[");
    for (i, t) in items.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&each(t));
    }
    out.push(']');
    out
}

/// A service a node reaches.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Service {
    /// Its name among its node's.
    pub name: String,
    /// Its TCP address.
    pub addr: String,
    /// What it is.
    pub desc: String,
}

impl Service {
    /// As JSON.
    #[must_use]
    pub fn json(&self) -> String {
        Obj::new()
            .str("name", &self.name)
            .str("addr", &self.addr)
            .str_omit("desc", &self.desc)
            .end()
    }
    /// From JSON.
    #[must_use]
    pub fn from(v: &Value) -> Service {
        Service {
            name: s(v, "name"),
            addr: s(v, "addr"),
            desc: s(v, "desc"),
        }
    }
}

/// A list of services from a JSON array.
#[must_use]
pub fn services(v: Option<&Value>) -> Vec<Service> {
    match v {
        Some(Value::Array(items)) => items.iter().map(Service::from).collect(),
        _ => Vec::new(),
    }
}

/// What a peer tells the hub about itself.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Info {
    /// Its name.
    pub name: String,
    /// `node` or `proxy`.
    pub kind: String,
    /// Its version.
    pub version: String,
    /// Its host's name.
    pub hostname: String,
    /// Its OS and architecture.
    pub os: String,
    /// The services a node reaches.
    pub services: Vec<Service>,
    /// Anything else it was configured to tell.
    pub meta: Vec<(String, String)>,
}

impl Info {
    /// An Info of this process.
    #[must_use]
    pub fn new(name: &str, kind: &str) -> Info {
        Info {
            name: name.to_owned(),
            kind: kind.to_owned(),
            version: version(),
            hostname: akit_config::roles::host_name(),
            os: format!("{}/{}", std::env::consts::OS, go_arch()),
            ..Info::default()
        }
    }
    /// As JSON.
    #[must_use]
    pub fn json(&self) -> String {
        let mut o = Obj::new()
            .str("name", &self.name)
            .str("type", &self.kind)
            .str_omit("version", &self.version)
            .str_omit("hostname", &self.hostname)
            .str_omit("os", &self.os);
        if !self.services.is_empty() {
            o = o.raw("services", &array(&self.services, Service::json));
        }
        if !self.meta.is_empty() {
            let mut m = Obj::new();
            for (k, v) in &self.meta {
                m = m.str(k, v);
            }
            o = o.raw("meta", &m.end());
        }
        o.end()
    }
    /// From JSON.
    #[must_use]
    pub fn from(v: &Value) -> Info {
        Info {
            name: s(v, "name"),
            kind: s(v, "type"),
            version: s(v, "version"),
            hostname: s(v, "hostname"),
            os: s(v, "os"),
            services: services(v.get("services")),
            meta: match v.get("meta") {
                Some(Value::Object(ms)) => ms
                    .iter()
                    .filter_map(|(k, v)| match v {
                        Value::String(s) => Some((k.clone(), s.clone())),
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            },
        }
    }
    /// The service named `name`.
    #[must_use]
    pub fn service(&self, name: &str) -> Option<&Service> {
        self.services.iter().find(|s| s.name == name)
    }
}

/// Go's name of the architecture, as `runtime.GOARCH`.
fn go_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "x86" => "386",
        other => other,
    }
}

/// A registration.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RegisterReq {
    /// The peer.
    pub info: Info,
    /// With the password: the time.
    pub time: i64,
    /// With the password: a fresh nonce in hex.
    pub nonce: String,
    /// With the password: its MAC.
    pub mac: String,
    /// With a token: its ID.
    pub token: String,
    /// With a token: its proof.
    pub proof: String,
}

impl RegisterReq {
    /// As JSON.
    #[must_use]
    pub fn json(&self) -> String {
        Obj::new()
            .raw("info", &self.info.json())
            .num_omit("time", self.time)
            .str_omit("nonce", &self.nonce)
            .str_omit("mac", &self.mac)
            .str_omit("token", &self.token)
            .str_omit("proof", &self.proof)
            .end()
    }
    /// From JSON.
    #[must_use]
    pub fn from(v: &Value) -> RegisterReq {
        RegisterReq {
            info: v.get("info").map(Info::from).unwrap_or_default(),
            time: n(v, "time"),
            nonce: s(v, "nonce"),
            mac: s(v, "mac"),
            token: s(v, "token"),
            proof: s(v, "proof"),
        }
    }
}

/// The answer to a registration.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RegisterRsp {
    /// The peer's number on the hub.
    pub peer: u32,
    /// The link's number on the hub, the high 32 bits of its tunnels' IDs.
    pub conn: u32,
    /// The link's place among the peer's links.
    pub index: i64,
    /// The hub's name.
    pub hub: String,
    /// The hub's proof.
    pub proof: String,
}

impl RegisterRsp {
    /// As JSON.
    #[must_use]
    pub fn json(&self) -> String {
        Obj::new()
            .num("peer", self.peer)
            .num("conn", self.conn)
            .num("index", self.index)
            .str("hub", &self.hub)
            .str_omit("proof", &self.proof)
            .end()
    }
    /// From JSON.
    #[must_use]
    pub fn from(v: &Value) -> RegisterRsp {
        RegisterRsp {
            peer: u32::try_from(n(v, "peer")).unwrap_or(0),
            conn: u32::try_from(n(v, "conn")).unwrap_or(0),
            index: n(v, "index"),
            hub: s(v, "hub"),
            proof: s(v, "proof"),
        }
    }

    /// The fields of the answer a hub's token proof covers.
    fn fields(&self) -> [String; 4] {
        [
            self.peer.to_string(),
            self.conn.to_string(),
            self.index.to_string(),
            self.hub.clone(),
        ]
    }
}

/// The round trips a peer measured, as fit's `latency.Stats`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Stats {
    /// The last, in milliseconds.
    pub last: f64,
    /// The moving average.
    pub avg: f64,
    /// The shortest.
    pub min: f64,
    /// The share lost.
    pub loss: f64,
    /// How many.
    pub samples: i64,
}

impl Stats {
    /// As JSON: the average and the loss rounded to thousandths, as fit's `Meter.Stats`.
    #[must_use]
    pub fn json(&self) -> String {
        let round = |v: f64| (v * 1000.0).round() / 1000.0;
        Obj::new()
            .float("last", self.last)
            .float("avg", round(self.avg))
            .float("min", self.min)
            .float("loss", round(self.loss))
            .num("samples", self.samples)
            .end()
    }

    /// Whether a round trip was measured (fit's `Known`).
    #[must_use]
    pub fn known(&self) -> bool {
        self.samples > 0
    }
    /// From JSON.
    #[must_use]
    pub fn from(v: Option<&Value>) -> Stats {
        let Some(v) = v else {
            return Stats::default();
        };
        Stats {
            last: f(v, "last"),
            avg: f(v, "avg"),
            min: f(v, "min"),
            loss: f(v, "loss"),
            samples: n(v, "samples"),
        }
    }
    /// A round trip measured.
    pub fn add(&mut self, ms: f64) {
        const ALPHA: f64 = 0.3;
        if self.samples == 0 {
            self.avg = ms;
            self.min = ms;
        } else {
            self.avg += ALPHA * (ms - self.avg);
            self.min = self.min.min(ms);
        }
        self.last = ms;
        self.loss *= 1.0 - ALPHA;
        self.samples = self.samples.saturating_add(1);
    }
    /// A ping lost.
    pub fn lost(&mut self) {
        self.loss += 0.3 * (1.0 - self.loss);
    }
}

/// A tunnel's open request.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TunnelOpenReq {
    /// The tunnel's ID.
    pub id: u64,
    /// The node.
    pub node: String,
    /// The service.
    pub service: String,
    /// The proxy, which the hub fills in for the node.
    pub proxy: String,
}

impl TunnelOpenReq {
    /// As JSON.
    #[must_use]
    pub fn json(&self) -> String {
        Obj::new()
            .num("id", self.id)
            .str("node", &self.node)
            .str("service", &self.service)
            .str_omit("proxy", &self.proxy)
            .end()
    }
    /// From JSON.
    #[must_use]
    pub fn from(v: &Value) -> TunnelOpenReq {
        TunnelOpenReq {
            id: match v.get("id") {
                Some(Value::Number(n)) => n.parse().unwrap_or(0),
                _ => 0,
            },
            node: s(v, "node"),
            service: s(v, "service"),
            proxy: s(v, "proxy"),
        }
    }
}

/// The ID of the `seq`-th tunnel opened over link `conn`.
#[must_use]
pub fn tunnel_id(conn: u32, seq: u32) -> u64 {
    (u64::from(conn) << 32) | u64::from(seq)
}

/// The link number in a tunnel's ID.
#[must_use]
pub fn tunnel_conn(id: u64) -> u32 {
    u32::try_from(id >> 32).unwrap_or(0)
}

/// A tunnel frame's body: the ID, 8 bytes big-endian, and the payload.
#[must_use]
pub fn frame(id: u64, payload: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(payload.len().saturating_add(8));
    b.extend_from_slice(&id.to_be_bytes());
    b.extend_from_slice(payload);
    b
}

/// Splits a tunnel frame's body into its ID and payload.
#[must_use]
pub fn parse_frame(b: &[u8]) -> Option<(u64, &[u8])> {
    let (id, payload) = b.split_first_chunk::<8>()?;
    Some((u64::from_be_bytes(*id), payload))
}

/// An acknowledgement's count.
#[must_use]
pub fn parse_ack(payload: &[u8]) -> Option<i64> {
    let b: [u8; 8] = payload.try_into().ok()?;
    i64::try_from(u64::from_be_bytes(b)).ok()
}

fn hmac_hex(key: &[u8], data: &[u8]) -> String {
    akit_roles::linktoken::hex(&Hmac::<Sha256>::mac(key, data))
}

fn reg_mac(req: &RegisterReq, password: &str) -> String {
    let msg = format!(
        "{}\n{}\n{}\n{}",
        req.info.kind, req.info.name, req.time, req.nonce
    );
    hmac_hex(password.as_bytes(), msg.as_bytes())
}

/// Fills in `req`'s time, nonce and MAC for the shared `password` (fit's `Sign`).
pub fn sign(req: &mut RegisterReq, password: &str) {
    let mut nonce = [0u8; 16];
    let _ = akio_crypto::rand::fill_random(&mut nonce);
    req.time = akit_roles::linktoken::unix_now();
    req.nonce = akit_roles::linktoken::hex(&nonce);
    req.mac = reg_mac(req, password);
}

const PROOF_LABEL: &str = "fit tunnel hub proof v1";

fn proof(req: &RegisterReq, rsp: &RegisterRsp, password: &str) -> String {
    let mut b = Vec::new();
    for f in [
        PROOF_LABEL.to_owned(),
        req.info.kind.clone(),
        req.info.name.clone(),
        req.time.to_string(),
        req.nonce.clone(),
        req.mac.clone(),
        rsp.peer.to_string(),
        rsp.conn.to_string(),
        rsp.index.to_string(),
        rsp.hub.clone(),
    ] {
        b.extend_from_slice(&u32::try_from(f.len()).unwrap_or(u32::MAX).to_be_bytes());
        b.extend_from_slice(f.as_bytes());
    }
    hmac_hex(password.as_bytes(), &b)
}

/// Fills in `rsp`'s proof, the hub's answer to `req`, for `password`.
pub fn prove(req: &RegisterReq, rsp: &mut RegisterRsp, password: &str) {
    rsp.proof = proof(req, rsp, password);
}

/// Checks that `rsp`, the answer to `req`, proves its hub knows `password`.
///
/// # Errors
///
/// A hub that sent no proof, or a wrong one.
pub fn check_proof(req: &RegisterReq, rsp: &RegisterRsp, password: &str) -> Result<(), String> {
    if rsp.proof.is_empty() {
        return Err(format!(
            "tunnel: the hub runs a fit without hub authentication, older than this side's {}; upgrade it",
            version()
        ));
    }
    if !akio_crypto::ct_eq(proof(req, rsp, password).as_bytes(), rsp.proof.as_bytes()) {
        return Err("tunnel: the hub did not prove it knows the password: it is not the hub, or its password differs".to_owned());
    }
    Ok(())
}

/// Checks the type and name a registration tells.
///
/// # Errors
///
/// No name, or a type that is neither.
pub fn check_peer(info: &Info) -> Result<(), String> {
    if info.name.is_empty() {
        return Err("tunnel: a peer needs a name".to_owned());
    }
    if info.kind != TYPE_NODE && info.kind != TYPE_PROXY {
        return Err(format!("tunnel: unknown peer type {:?}", info.kind));
    }
    Ok(())
}

/// How far a registration's time may be from the hub's clock.
pub const MAX_CLOCK_SKEW: i64 = 5 * 60;
/// How many registration nonces a verifier remembers.
pub const MAX_NONCES: usize = 1 << 16;

/// Checks registrations against a password, refusing a nonce seen within the skew.
#[derive(Debug, Default)]
pub struct Verifier {
    /// The password.
    pub password: String,
    seen: std::collections::HashMap<String, i64>,
    swept: i64,
}

impl Verifier {
    /// A verifier of `password`.
    #[must_use]
    pub fn new(password: &str) -> Verifier {
        Verifier {
            password: password.to_owned(),
            ..Verifier::default()
        }
    }

    /// Checks `req`.
    ///
    /// # Errors
    ///
    /// Why it is refused.
    pub fn verify(&mut self, req: &RegisterReq) -> Result<(), String> {
        check_peer(&req.info)?;
        let now = akit_roles::linktoken::unix_now();
        if now.saturating_sub(req.time).abs() > MAX_CLOCK_SKEW {
            return Err("tunnel: registration time too far from the hub's clock".to_owned());
        }
        let want = reg_mac(req, &self.password);
        if req.nonce.len() < 16
            || req.nonce.len() > 128
            || !akio_crypto::ct_eq(want.as_bytes(), req.mac.as_bytes())
        {
            return Err("tunnel: authentication failed".to_owned());
        }
        if self.seen.len() >= MAX_NONCES || now.saturating_sub(self.swept) > 60 {
            self.swept = now;
            self.seen
                .retain(|_, at| now.saturating_sub(*at) <= 2 * MAX_CLOCK_SKEW);
        }
        if self.seen.contains_key(&req.nonce) {
            return Err("tunnel: registration replayed".to_owned());
        }
        if self.seen.len() >= MAX_NONCES {
            return Err("tunnel: too many registrations; try again later".to_owned());
        }
        self.seen.insert(req.nonce.clone(), now);
        Ok(())
    }
}

/// The tunnel's registration label in a token's proofs.
pub const TOKEN_LABEL: &str = "fit tunnel register v1";

/// Fills in `req`'s token and proof with `sec`, over the challenge's nonces.
pub fn prove_token(
    req: &mut RegisterReq,
    sec: &akit_roles::linktoken::Secret,
    peer_nonce: &[u8],
    hub_nonce: &[u8],
) {
    req.token = sec.id().to_owned();
    req.time = 0;
    req.nonce.clear();
    req.mac.clear();
    let extra = [req.info.kind.as_str(), req.info.name.as_str()];
    let t = akit_roles::linktoken::Transcript {
        label: TOKEN_LABEL,
        token: &req.token,
        dialer: peer_nonce,
        listener: hub_nonce,
        extra: &extra,
    };
    req.proof = akit_roles::linktoken::hex(&sec.prove(&t));
}

/// Checks `req`'s proof of the token whose stored key is `stored`.
#[must_use]
pub fn verify_token(req: &RegisterReq, stored: &[u8], peer_nonce: &[u8], hub_nonce: &[u8]) -> bool {
    let Some(proof) = akit_roles::linktoken::unhex(&req.proof) else {
        return false;
    };
    let extra = [req.info.kind.as_str(), req.info.name.as_str()];
    let t = akit_roles::linktoken::Transcript {
        label: TOKEN_LABEL,
        token: &req.token,
        dialer: peer_nonce,
        listener: hub_nonce,
        extra: &extra,
    };
    akit_roles::linktoken::verify(stored, &t, &proof)
}

/// Fills in `rsp`'s proof of the token whose stored key is `stored`.
pub fn prove_token_hub(
    req: &RegisterReq,
    rsp: &mut RegisterRsp,
    stored: &[u8],
    peer_nonce: &[u8],
    hub_nonce: &[u8],
) {
    let extra = [req.info.kind.as_str(), req.info.name.as_str()];
    let t = akit_roles::linktoken::Transcript {
        label: TOKEN_LABEL,
        token: &req.token,
        dialer: peer_nonce,
        listener: hub_nonce,
        extra: &extra,
    };
    let fields = rsp.fields();
    let answer: Vec<&str> = fields.iter().map(String::as_str).collect();
    rsp.proof =
        akit_roles::linktoken::hex(&akit_roles::linktoken::listener_proof(stored, &t, &answer));
}

/// Checks that `rsp` proves its hub holds the token `sec`.
#[must_use]
pub fn check_token_proof(
    req: &RegisterReq,
    rsp: &RegisterRsp,
    sec: &akit_roles::linktoken::Secret,
    peer_nonce: &[u8],
    hub_nonce: &[u8],
) -> bool {
    let Some(proof) = akit_roles::linktoken::unhex(&rsp.proof) else {
        return false;
    };
    let extra = [req.info.kind.as_str(), req.info.name.as_str()];
    let t = akit_roles::linktoken::Transcript {
        label: TOKEN_LABEL,
        token: &req.token,
        dialer: peer_nonce,
        listener: hub_nonce,
        extra: &extra,
    };
    let fields = rsp.fields();
    let answer: Vec<&str> = fields.iter().map(String::as_str).collect();
    sec.verify_listener(&t, &proof, &answer)
}

/// A challenge's nonce, from hex.
#[must_use]
pub fn decode_nonce(s: &str) -> Option<Vec<u8>> {
    akit_roles::linktoken::unhex(s).filter(|b| b.len() == akit_roles::linktoken::NONCE_SIZE)
}

/// A string member of a message, for the role modules.
#[must_use]
pub fn get_str(v: &Value, k: &str) -> String {
    s(v, k)
}

/// A number member of a message, for the role modules.
#[must_use]
pub fn get_num(v: &Value, k: &str) -> i64 {
    n(v, k)
}
