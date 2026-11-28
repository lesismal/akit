//! The samples of the roles' fields (I98): for each field of `tunnel` and `llm`, a config that
//! sets it and either another that differs only in it, both valid, which akit-config must read as
//! different configs, or, for a field fit refuses in that role (`llm.hub.listen.hub_password`), the
//! refusal its check gives, not "unknown field".

/// How a sample shows its field is read.
pub(crate) enum Read {
    /// The two configs differ only in the field: their models differ.
    Differs(String),
    /// The config is refused by the field's own check, with this message.
    Refused(&'static str),
}

const H64: &str = "abababababababababababababababababababababababababababababababab";
const C64: &str = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";
const TOK_A: &str = "akitt1.0123456789abcdef.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const TOK_B: &str = "akitt1.0123456789abcdef.BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";
const HASH_A: &str = "$2a$04$5xt5UjNNUVWTqOgf9FDZm.bfWVyXg.U2as/rOFV.Ny5IQstc/jC26";
const HASH_B: &str = "$2a$04$5xt5UjNNUVWTqOgf9FDZm.bfWVyXg.U2as/rOFV.Ny5IQstc/jC27";
// Two self-signed EC certificates and their keys, made for this test (openssl, P-256).
const CERT_A: &str = r#""-----BEGIN CERTIFICATE-----\nMIIBZTCCAQygAwIBAgIUHbSe+D/8lFj//T18SoXsQ5N5lxIwCgYIKoZIzj0EAwIw\nGDEWMBQGA1UEAwwNYWtpdC1jb21wYXQtYTAgFw0yNjExMjYxODAwNTVaGA8yMTI2\nMTEwMjE4MDA1NVowGDEWMBQGA1UEAwwNYWtpdC1jb21wYXQtYTBZMBMGByqGSM49\nAgEGCCqGSM49AwEHA0IABG4Ytk27PBBxHipzgVQ+mSxIr6M1izYwkKaOp94tpRyw\nBVAj04tqJ/D61zCSbs/PZAdg3SnwxdZIkdzdHpKaatSjMjAwMB0GA1UdDgQWBBRr\nW6ncyPSw7Zln9PlqMPrHMuZ8xzAPBgNVHRMBAf8EBTADAQH/MAoGCCqGSM49BAMC\nA0cAMEQCIGRJ1EuS+IvbXnDJbpy3Zh9XwL3Ze4pODEGTojLkrZa3AiAPeOh36Jmg\n8ompTorBetXBf9zprsN4ju/gVrnx9kP2vw==\n-----END CERTIFICATE-----\n""#;
const CERT_B: &str = r#""-----BEGIN CERTIFICATE-----\nMIIBZTCCAQygAwIBAgIUQvpi0CyFqEPvJQ/AAu6rYTuLbbgwCgYIKoZIzj0EAwIw\nGDEWMBQGA1UEAwwNYWtpdC1jb21wYXQtYjAgFw0yNjExMjYxODAwNTVaGA8yMTI2\nMTEwMjE4MDA1NVowGDEWMBQGA1UEAwwNYWtpdC1jb21wYXQtYjBZMBMGByqGSM49\nAgEGCCqGSM49AwEHA0IABC2yqiOtizo6Sr+OEbtlnbpaldNhivg+74V3QI35HVMq\noQBmlcvppc8h++BhlnplRaQVwBP9MydrzpvGCdDMeISjMjAwMB0GA1UdDgQWBBS1\nK1Ub4N8OM4FiJ4lgdZFrFmKupzAPBgNVHRMBAf8EBTADAQH/MAoGCCqGSM49BAMC\nA0cAMEQCIHaGvD/kenQrXN2/+mY+XZgY4b//YqKPV1LcC0zSPpEWAiAdwOpm1iPb\nvFKWoAvCvTw/nfAEQccbQCAf48stio6fgw==\n-----END CERTIFICATE-----\n""#;
const KEY_A: &str = r#""-----BEGIN PRIVATE KEY-----\nMIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgmq0GaJ7+BFMNIKA6\nT5lJWwSi1ARPRp+jSg/dvCYZ+IWhRANCAARuGLZNuzwQcR4qc4FUPpksSK+jNYs2\nMJCmjqfeLaUcsAVQI9OLaifw+tcwkm7Pz2QHYN0p8MXWSJHc3R6SmmrU\n-----END PRIVATE KEY-----\n""#;
const KEY_B: &str = r#""-----BEGIN PRIVATE KEY-----\nMIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgGSSsv5aZQGjxoXnf\n70VTmS0ofXnQf+TYrPdF3/73hwmhRANCAAQtsqojrYs6Okq/jhG7ZZ26WpXTYYr4\nPu+Fd0CN+R1TKqEAZpXL6aXPIfvgYZZ6ZUWkFcAT/TMna86bxgnQzHiE\n-----END PRIVATE KEY-----\n""#;

fn q(s: &str) -> String {
    format!("\"{s}\"")
}

/// A field: its key, two values, and members it needs beside it (a value of "" leaves a member
/// the object needs out).
struct F {
    key: &'static str,
    a: String,
    b: Read,
    extra: Vec<(&'static str, String)>,
}

fn f(key: &'static str, a: impl Into<String>, b: impl Into<String>) -> F {
    F {
        key,
        a: a.into(),
        b: Read::Differs(b.into()),
        extra: Vec::new(),
    }
}

fn fx(
    key: &'static str,
    a: impl Into<String>,
    b: impl Into<String>,
    extra: &[(&'static str, &str)],
) -> F {
    F {
        key,
        a: a.into(),
        b: Read::Differs(b.into()),
        extra: extra.iter().map(|(k, v)| (*k, (*v).to_owned())).collect(),
    }
}

fn refused(key: &'static str, a: impl Into<String>, msg: &'static str) -> F {
    F {
        key,
        a: a.into(),
        b: Read::Refused(msg),
        extra: Vec::new(),
    }
}

/// An object's members: what it needs, then the field's extras over them, then the field.
fn members(need: &[(&str, String)], field: &F, value: &str) -> String {
    let mut out: Vec<(String, String)> = need
        .iter()
        .filter(|(k, _)| *k != field.key)
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect();
    for (k, v) in &field.extra {
        out.retain(|(o, _)| o != k);
        if !v.is_empty() {
            out.push(((*k).to_owned(), v.clone()));
        }
    }
    out.push((field.key.to_owned(), value.to_owned()));
    out.iter()
        .map(|(k, v)| format!("\"{k}\": {v}"))
        .collect::<Vec<_>>()
        .join(", ")
}

type Wrap<'a> = &'a dyn Fn(&str) -> String;

fn emit(
    out: &mut Vec<(String, String, Read)>,
    path: &str,
    wrap: Wrap<'_>,
    need: &[(&str, String)],
    fields: Vec<F>,
) {
    for field in fields {
        let doc_a = wrap(&format!("{{{}}}", members(need, &field, &field.a)));
        let read = match &field.b {
            Read::Differs(b) => Read::Differs(wrap(&format!("{{{}}}", members(need, &field, b)))),
            Read::Refused(m) => Read::Refused(m),
        };
        out.push((format!("{path}.{}", field.key), doc_a, read));
    }
}

fn console(port: u16) -> String {
    format!(
        r#"{{"address": "127.0.0.1:{port}", "users": [{{"username": "a", "password_hash": "{HASH_A}"}}]}}"#
    )
}

fn access_fields() -> Vec<F> {
    vec![
        f("allow", r#"["10.0.0.0/8"]"#, r#"["10.0.0.0/9"]"#),
        f("deny", r#"["10.0.0.0/8"]"#, r#"["10.0.0.0/9"]"#),
        f("allow_files", r#"["a"]"#, r#"["b"]"#),
        f("deny_files", r#"["a"]"#, r#"["b"]"#),
    ]
}

/// A role's `console` and `access` at `path`, `wrap` putting members into the role.
fn console_and_access(
    out: &mut Vec<(String, String, Read)>,
    path: &str,
    wrap: Wrap<'_>,
    access: bool,
) {
    let c = format!("{path}.console");
    let cw = |m: &str| wrap(&format!(r#""console": {m}"#));
    let need = [
        ("address", q("127.0.0.1:7080")),
        (
            "users",
            format!(r#"[{{"username": "a", "password_hash": "{HASH_A}"}}]"#),
        ),
    ];
    emit(
        out,
        &c,
        &cw,
        &need,
        vec![
            f("address", q("127.0.0.1:7080"), q("127.0.0.1:7081")),
            fx(
                "cert_file",
                q("a.pem"),
                q("b.pem"),
                &[("key_file", r#""k.pem""#)],
            ),
            fx(
                "key_file",
                q("a.pem"),
                q("b.pem"),
                &[("cert_file", r#""c.pem""#)],
            ),
            f(
                "users",
                format!(r#"[{{"username": "a", "password_hash": "{HASH_A}"}}]"#),
                format!(r#"[{{"username": "b", "password_hash": "{HASH_A}"}}]"#),
            ),
            f("session_timeout", q("1m"), q("2m")),
            f("session_lifetime", q("1h"), q("2h")),
            f("max_sessions", "1", "2"),
            f("max_conns", "1", "2"),
            f("access", "{}", r#"{"allow": ["10.0.0.0/8"]}"#),
        ],
    );
    let uw = |m: &str| {
        wrap(&format!(
            r#""console": {{"address": "127.0.0.1:7080", "users": [{m}]}}"#
        ))
    };
    emit(
        out,
        &format!("{c}.users[]"),
        &uw,
        &[("username", q("a")), ("password_hash", q(HASH_A))],
        vec![
            f("username", q("a"), q("b")),
            f("password_hash", q(HASH_A), q(HASH_B)),
        ],
    );
    let aw = |m: &str| {
        wrap(&format!(
            r#""console": {{"address": "127.0.0.1:7080", "users": [{{"username": "a", "password_hash": "{HASH_A}"}}], "access": {m}}}"#
        ))
    };
    emit(out, &format!("{c}.access"), &aw, &[], access_fields());
    if access {
        let aw = |m: &str| wrap(&format!(r#""access": {m}"#));
        emit(out, &format!("{path}.access"), &aw, &[], access_fields());
    }
}

fn token_fields() -> Vec<F> {
    vec![
        f("id", q("0123456789abcdef"), q("0123456789abcde0")),
        f("name", q("a"), q("b")),
        f("hash", q(H64), q(C64)),
        f("created_at", "1", "2"),
        f("expires_at", "1", "2"),
        f("note", q("a"), q("b")),
        f("members", r#"["a"]"#, r#"["b"]"#),
    ]
}

fn tokens(out: &mut Vec<(String, String, Read)>, path: &str, wrap: Wrap<'_>) {
    let tw = |m: &str| wrap(&format!(r#""tokens": [{m}]"#));
    emit(
        out,
        &format!("{path}.tokens[]"),
        &tw,
        &[
            ("id", q("0123456789abcdef")),
            ("name", q("a")),
            ("hash", q(H64)),
        ],
        token_fields(),
    );
}

fn uplinks(out: &mut Vec<(String, String, Read)>, path: &str, wrap: Wrap<'_>) {
    let hw = |m: &str| wrap(&format!(r#""hubs": [{m}]"#));
    emit(
        out,
        &format!("{path}.hubs[]"),
        &hw,
        &[
            ("endpoints", r#"[{"address": "127.0.0.1:7000"}]"#.to_owned()),
            ("password", q("p")),
        ],
        vec![
            f("name", q("a"), q("b")),
            f(
                "endpoints",
                r#"[{"address": "127.0.0.1:7000"}]"#,
                r#"[{"address": "127.0.0.1:7001"}]"#,
            ),
            fx("token", q(TOK_A), q(TOK_B), &[("password", "")]),
            f("password", q("a"), q("b")),
            f("body_password", q("a"), q("b")),
            f("pin", q(H64), q(C64)),
            f("quic_pin", q(H64), q(C64)),
            f("ca_file", q("a.pem"), q("b.pem")),
            f("quic_insecure", "true", "false"),
            f("pool_size", "1", "2"),
            f("dial_timeout", q("1s"), q("2s")),
            f("call_timeout", q("1s"), q("2s")),
            f("retry_interval", q("1s"), q("2s")),
            f("ping_interval", q("1s"), q("2s")),
            f("ping_timeout", q("1s"), q("2s")),
        ],
    );
    let ew = |m: &str| {
        wrap(&format!(
            r#""hubs": [{{"password": "p", "endpoints": [{m}, {{"address": "127.0.0.1:7999"}}]}}]"#
        ))
    };
    emit(
        out,
        &format!("{path}.hubs[].endpoints[]"),
        &ew,
        &[("address", q("127.0.0.1:7000"))],
        vec![
            f("address", q("127.0.0.1:7000"), q("127.0.0.1:7001")),
            f("disabled", "true", "false"),
        ],
    );
}

fn tunnel(out: &mut Vec<(String, String, Read)>) {
    out.push((
        "tunnel".into(),
        r#"{"tunnel": {}}"#.into(),
        Read::Differs(r#"{"tunnel": {"hub": {}}}"#.into()),
    ));
    let uplink = r#"[{"endpoints": [{"address": "127.0.0.1:7000"}], "password": "p"}]"#;
    // The hub.
    let hub = |m: &str| format!(r#"{{"tunnel": {{"hub": {m}}}}}"#);
    out.push((
        "tunnel.hub".into(),
        hub("{}"),
        Read::Differs(hub(r#"{"name": "a"}"#)),
    ));
    let hw = |m: &str| hub(&format!("{{{m}}}"));
    emit(
        out,
        "tunnel.hub",
        &|m: &str| hub(m),
        &[],
        vec![
            f("name", q("a"), q("b")),
            f("password", q("a"), q("b")),
            f(
                "tokens",
                "[]",
                format!(r#"[{{"id": "0123456789abcdef", "name": "a", "hash": "{H64}"}}]"#),
            ),
            f("body_password", q("a"), q("b")),
            f(
                "listeners",
                "[]",
                r#"[{"protocol": "tcp", "address": "127.0.0.1:7000"}]"#,
            ),
            f("access", "{}", r#"{"allow": ["10.0.0.0/8"]}"#),
            fx(
                "console_socket",
                r#"{"address": "127.0.0.1:7002"}"#,
                r#"{"address": "127.0.0.1:7003"}"#,
                &[("password", r#""p""#), ("console", &console(7080))],
            ),
            f("quic", "{}", r#"{"idle_timeout": "5s"}"#),
            f("register_timeout", q("1s"), q("2s")),
            f("call_timeout", q("1s"), q("2s")),
            f("max_links", "1", "2"),
            f("max_unregistered", "1", "2"),
            f("max_unregistered_per_ip", "1", "2"),
            f("max_links_per_peer", "1", "2"),
            f("max_peers", "1", "2"),
            f("max_tunnels", "1", "2"),
            f("allow_proxy_services", "true", "false"),
            f("console", console(7080), console(7081)),
        ],
    );
    let lw = |m: &str| hw(&format!(r#""listeners": [{m}]"#));
    emit(
        out,
        "tunnel.hub.listeners[]",
        &lw,
        &[("protocol", q("tcp")), ("address", q("127.0.0.1:7000"))],
        vec![
            f("protocol", q("tcp"), q("ws")),
            f("address", q("127.0.0.1:7000"), q("127.0.0.1:7001")),
            fx("path", q("/a"), q("/b"), &[("protocol", r#""ws""#)]),
            fx(
                "cert_file",
                q("a.pem"),
                q("b.pem"),
                &[("protocol", r#""tls""#), ("key_file", r#""k.pem""#)],
            ),
            fx(
                "key_file",
                q("a.pem"),
                q("b.pem"),
                &[("protocol", r#""tls""#), ("cert_file", r#""c.pem""#)],
            ),
        ],
    );
    let sw = |m: &str| {
        hw(&format!(
            r#""password": "p", "console": {}, "console_socket": {m}"#,
            console(7080)
        ))
    };
    emit(
        out,
        "tunnel.hub.console_socket",
        &sw,
        &[("address", q("127.0.0.1:7002"))],
        vec![
            f("address", q("127.0.0.1:7002"), q("127.0.0.1:7003")),
            f("password", q("a"), q("b")),
            f("max_conns", "1", "2"),
        ],
    );
    let qw = |m: &str| hw(&format!(r#""quic": {m}"#));
    emit(
        out,
        "tunnel.hub.quic",
        &qw,
        &[],
        vec![
            fx("cert", CERT_A, CERT_B, &[("key", KEY_A)]),
            fx("key", KEY_A, KEY_B, &[("cert", CERT_A)]),
            f("idle_timeout", q("1s"), q("2s")),
            f("keep_alive", q("1s"), q("2s")),
            f("handshake_timeout", q("1s"), q("2s")),
            f("max_handshakes", "1", "2"),
        ],
    );
    tokens(out, "tunnel.hub", &hw);
    console_and_access(out, "tunnel.hub", &hw, true);

    // The node.
    let node = |m: &str| format!(r#"{{"tunnel": {{"node": {m}}}}}"#);
    out.push((
        "tunnel.node".into(),
        node("{}"),
        Read::Differs(node(r#"{"name": "a"}"#)),
    ));
    let nw = |m: &str| node(&format!("{{{m}}}"));
    emit(
        out,
        "tunnel.node",
        &|m: &str| node(m),
        &[],
        vec![
            f("name", q("a"), q("b")),
            f("meta", r#"{"a": "1"}"#, r#"{"a": "2"}"#),
            f("hubs", "[]", uplink),
            f(
                "services",
                "[]",
                r#"[{"name": "s", "address": "127.0.0.1:22"}]"#,
            ),
            f("allow_hub_services", "true", "false"),
            f("dial_timeout", q("1s"), q("2s")),
            f("idle_timeout", q("1s"), q("2s")),
            f("max_tunnels", "1", "2"),
            f("console", console(7080), console(7081)),
        ],
    );
    uplinks(out, "tunnel.node", &nw);
    let svw = |m: &str| nw(&format!(r#""services": [{m}]"#));
    emit(
        out,
        "tunnel.node.services[]",
        &svw,
        &[("name", q("s")), ("address", q("127.0.0.1:22"))],
        vec![
            f("name", q("a"), q("b")),
            f("address", q("127.0.0.1:22"), q("127.0.0.1:23")),
            f("description", q("a"), q("b")),
        ],
    );
    console_and_access(out, "tunnel.node", &nw, false);

    // The proxy.
    let proxy = |m: &str| format!(r#"{{"tunnel": {{"proxy": {m}}}}}"#);
    out.push((
        "tunnel.proxy".into(),
        proxy("{}"),
        Read::Differs(proxy(r#"{"name": "a"}"#)),
    ));
    let pw = |m: &str| proxy(&format!("{{{m}}}"));
    emit(
        out,
        "tunnel.proxy",
        &|m: &str| proxy(m),
        &[],
        vec![
            f("name", q("a"), q("b")),
            f("meta", r#"{"a": "1"}"#, r#"{"a": "2"}"#),
            f("hubs", "[]", uplink),
            f(
                "forwards",
                "[]",
                r#"[{"listen": "127.0.0.1:1", "node": "n", "service": "s"}]"#,
            ),
            f(
                "web_proxies",
                "[]",
                r#"[{"listen": "127.0.0.1:2", "url": "ws://h:1/", "password": "p"}]"#,
            ),
            f("access", "{}", r#"{"allow": ["10.0.0.0/8"]}"#),
            f("open_timeout", q("1s"), q("2s")),
            f("idle_timeout", q("1s"), q("2s")),
            f("max_tunnels", "1", "2"),
            f("console", console(7080), console(7081)),
        ],
    );
    uplinks(out, "tunnel.proxy", &pw);
    let fw = |m: &str| {
        pw(&format!(
            r#""hubs": [{{"name": "a", "endpoints": [{{"address": "127.0.0.1:7000"}}], "password": "p"}}, {{"name": "b", "endpoints": [{{"address": "127.0.0.1:7001"}}], "password": "p"}}], "forwards": [{m}]"#
        ))
    };
    emit(
        out,
        "tunnel.proxy.forwards[]",
        &fw,
        &[
            ("listen", q("127.0.0.1:1")),
            ("node", q("n")),
            ("service", q("s")),
        ],
        vec![
            f("listen", q("127.0.0.1:1"), q("127.0.0.1:2")),
            f("node", q("a"), q("b")),
            f("service", q("a"), q("b")),
            f("hub", q("a"), q("b")),
        ],
    );
    let ww = |m: &str| pw(&format!(r#""web_proxies": [{m}]"#));
    emit(
        out,
        "tunnel.proxy.web_proxies[]",
        &ww,
        &[
            ("listen", q("127.0.0.1:2")),
            ("url", q("ws://h:1/")),
            ("password", q("p")),
        ],
        vec![
            f("name", q("a"), q("b")),
            f("listen", q("127.0.0.1:2"), q("127.0.0.1:3")),
            f("url", q("ws://h:1/"), q("ws://h:2/")),
            f("password", q("a"), q("b")),
            f("max_conns", "1", "2"),
        ],
    );
    console_and_access(out, "tunnel.proxy", &pw, true);
}

fn llm(out: &mut Vec<(String, String, Read)>) {
    out.push((
        "llm".into(),
        r#"{"llm": {}}"#.into(),
        Read::Differs(r#"{"llm": {"hub": {}}}"#.into()),
    ));
    for (role, peer_role, other_role) in [
        ("hub", "node", "proxy"),
        ("node", "hub", "proxy"),
        ("proxy", "hub", "node"),
    ] {
        let path = format!("llm.{role}");
        let whole = |m: &str| format!(r#"{{"llm": {{"{role}": {m}}}}}"#);
        out.push((
            path.clone(),
            whole("{}"),
            Read::Differs(whole(r#"{"name": "a"}"#)),
        ));
        let rw = |m: &str| whole(&format!("{{{m}}}"));
        let peer =
            format!(r#"[{{"address": "127.0.0.1:1", "password": "p", "role": "{peer_role}"}}]"#);
        let mut fields = vec![
            f("id", q("a"), q("b")),
            f("name", q("a"), q("b")),
            f("console", console(7080), console(7081)),
            f(
                "listen",
                r#"{"address": "127.0.0.1:9200"}"#,
                r#"{"address": "127.0.0.1:9201"}"#,
            ),
            f("access", "{}", r#"{"allow": ["10.0.0.0/8"]}"#),
            f(
                "tokens",
                "[]",
                format!(r#"[{{"id": "0123456789abcdef", "name": "a", "hash": "{H64}"}}]"#),
            ),
            f("peers", "[]", peer.as_str()),
            f("limits", "{}", r#"{"max_conns": 1}"#),
        ];
        match role {
            "hub" => fields.push(f("max_nodes", "1", "2")),
            "node" => fields.extend([
                f("description", q("a"), q("b")),
                f(
                    "models",
                    "[]",
                    r#"[{"name": "m", "api": "openai", "base_url": "http://h"}]"#,
                ),
                f("settings", "{}", r#"{"allow_hub_manage": true}"#),
                f("proxy", "{}", r#"{"timeout": "1m"}"#),
            ]),
            _ => fields.extend([
                f(
                    "api",
                    r#"{"address": "127.0.0.1:9300"}"#,
                    r#"{"address": "127.0.0.1:9301"}"#,
                ),
                f(
                    "api_keys",
                    "[]",
                    format!(r#"[{{"id": "0123456789abcdef", "name": "k", "hash": "{H64}"}}]"#),
                ),
            ]),
        }
        emit(out, &path, &|m: &str| whole(m), &[], fields);
        // listen: a role's own password is refused on its own listener.
        let lw = |m: &str| rw(&format!(r#""listen": {m}"#));
        let mut lf = vec![
            f("address", q("127.0.0.1:9200"), q("127.0.0.1:9201")),
            f("password", q("a"), q("b")),
            fx(
                "cert_file",
                q("a.pem"),
                q("b.pem"),
                &[("key_file", r#""k.pem""#)],
            ),
            fx(
                "key_file",
                q("a.pem"),
                q("b.pem"),
                &[("cert_file", r#""c.pem""#)],
            ),
            f("self_signed", "true", "false"),
            fx("cert", CERT_A, CERT_B, &[("key", KEY_A)]),
            fx("key", KEY_A, KEY_B, &[("cert", CERT_A)]),
        ];
        for r in ["node", "proxy", "hub"] {
            let key: &'static str = match r {
                "node" => "node_password",
                "proxy" => "proxy_password",
                _ => "hub_password",
            };
            if r == role {
                lf.push(refused(
                    key,
                    q("a"),
                    match role {
                        "hub" => "a hub takes no links from hubs",
                        "node" => "a node takes no links from nodes",
                        _ => "a proxy takes no links from proxys",
                    },
                ));
            } else {
                lf.push(f(key, q("a"), q("b")));
            }
        }
        emit(
            out,
            &format!("{path}.listen"),
            &lw,
            &[("address", q("127.0.0.1:9200"))],
            lf,
        );
        let pw = |m: &str| rw(&format!(r#""peers": [{m}]"#));
        emit(
            out,
            &format!("{path}.peers[]"),
            &pw,
            &[
                ("address", q("127.0.0.1:1")),
                ("password", q("p")),
                ("role", q(peer_role)),
            ],
            vec![
                f("address", q("127.0.0.1:1"), q("127.0.0.1:2")),
                fx("token", q(TOK_A), q(TOK_B), &[("password", "")]),
                f("password", q("a"), q("b")),
                f("role", q(peer_role), q(other_role)),
                f("pin", q(H64), q(C64)),
                f("ca_file", q("a.pem"), q("b.pem")),
                fx("server_name", q("a"), q("b"), &[("ca_file", r#""ca.pem""#)]),
            ],
        );
        let limw = |m: &str| rw(&format!(r#""limits": {m}"#));
        emit(
            out,
            &format!("{path}.limits"),
            &limw,
            &[],
            vec![
                f("max_conns", "1", "2"),
                f("max_inflight", "1", "2"),
                f("ping_interval", q("1s"), q("2s")),
                f("slow_timeout", q("1s"), q("2s")),
                f("retry_interval", q("1s"), q("2s")),
                f("stream_idle", q("1s"), q("2s")),
            ],
        );
        tokens(out, &path, &rw);
        console_and_access(out, &path, &rw, true);
        if role == "node" {
            let mw = |m: &str| rw(&format!(r#""models": [{m}]"#));
            emit(
                out,
                "llm.node.models[]",
                &mw,
                &[
                    ("name", q("m")),
                    ("api", q("openai")),
                    ("base_url", q("http://h")),
                ],
                vec![
                    f("name", q("a"), q("b")),
                    f("api", q("openai"), q("ollama")),
                    f("base_url", q("http://h"), q("http://i")),
                    f("upstream", q("a"), q("b")),
                    f("api_key", q("a"), q("b")),
                    fx(
                        "api_version",
                        q("v1"),
                        q("v2"),
                        &[("api", r#""azure_openai""#)],
                    ),
                    f("disabled", "true", "false"),
                    f("description", q("a"), q("b")),
                    f("max_inflight", "1", "2"),
                ],
            );
            let sw = |m: &str| rw(&format!(r#""settings": {m}"#));
            emit(
                out,
                "llm.node.settings",
                &sw,
                &[],
                vec![
                    f("allow_hub_manage", "true", "false"),
                    f("gpu_sudo", "true", "false"),
                    f("refresh_interval", q("10s"), q("20s")),
                    f("job_timeout", q("20s"), q("30s")),
                    f("gpu_data_dir", q("a"), q("b")),
                ],
            );
            let xw = |m: &str| rw(&format!(r#""proxy": {m}"#));
            emit(
                out,
                "llm.node.proxy",
                &xw,
                &[],
                vec![
                    f("connect_timeout", q("1s"), q("2s")),
                    f("first_byte_timeout", q("1s"), q("2s")),
                    f("idle_timeout", q("1s"), q("2s")),
                    f("timeout", q("1s"), q("2s")),
                    f("max_inflight", "1", "2"),
                    f("max_body_bytes", "1", "2"),
                ],
            );
        }
        if role == "proxy" {
            let aw = |m: &str| rw(&format!(r#""api": {m}"#));
            emit(
                out,
                "llm.proxy.api",
                &aw,
                &[("address", q("127.0.0.1:9300"))],
                vec![
                    f("address", q("127.0.0.1:9300"), q("127.0.0.1:9301")),
                    fx(
                        "cert_file",
                        q("a.pem"),
                        q("b.pem"),
                        &[("key_file", r#""k.pem""#)],
                    ),
                    fx(
                        "key_file",
                        q("a.pem"),
                        q("b.pem"),
                        &[("cert_file", r#""c.pem""#)],
                    ),
                    f("max_body_bytes", "1", "2"),
                    f("max_conns", "1", "2"),
                    f("max_inflight", "1", "2"),
                    f("first_byte_timeout", q("1s"), q("2s")),
                    f("timeout", q("1s"), q("2s")),
                ],
            );
            let kw = |m: &str| rw(&format!(r#""api_keys": [{m}]"#));
            emit(
                out,
                "llm.proxy.api_keys[]",
                &kw,
                &[
                    ("id", q("0123456789abcdef")),
                    ("name", q("k")),
                    ("hash", q(H64)),
                ],
                vec![
                    f("id", q("0123456789abcdef"), q("0123456789abcde0")),
                    f("name", q("a"), q("b")),
                    f("prefix", q("a"), q("b")),
                    f("hash", q(H64), q(C64)),
                    f("created_at", "1", "2"),
                ],
            );
        }
    }
}

/// The WebRTC roles' fields (I110), and the `whip`, `whep` and `webrtc_proxy` handlers' params.
fn webrtc(out: &mut Vec<(String, String, Read)>) {
    let turn_need = [
        ("listen", r#"["127.0.0.1:3478"]"#.to_owned()),
        ("realm", q("r")),
        ("relay_ip", q("10.0.0.5")),
        ("secret", q("s")),
    ];
    let turn_min =
        r#"{"listen": ["127.0.0.1:3478"], "realm": "r", "relay_ip": "10.0.0.5", "secret": "s"}"#;
    let w = |m: &str| format!(r#"{{"webrtc": {m}}}"#);
    out.push((
        "webrtc".into(),
        w("{}"),
        Read::Differs(w(r#"{"media": {"listen": "127.0.0.1:8189"}}"#)),
    ));
    emit(
        out,
        "webrtc",
        &w,
        &[],
        vec![
            f("turn", turn_min, turn_min.replace("\"r\"", "\"q\"")),
            f(
                "media",
                r#"{"listen": "127.0.0.1:8189"}"#,
                r#"{"listen": "127.0.0.1:8188"}"#,
            ),
            f(
                "proxy",
                r#"{"listen": "127.0.0.1:8190"}"#,
                r#"{"listen": "127.0.0.1:8191"}"#,
            ),
        ],
    );
    // The TURN server.
    let tw = |m: &str| w(&format!(r#"{{"turn": {m}}}"#));
    let tls = |cert: &str| {
        format!(r#"{{"certificates": [{{"cert_file": "{cert}", "key_file": "k.pem"}}]}}"#)
    };
    emit(
        out,
        "webrtc.turn",
        &tw,
        &turn_need,
        vec![
            f("listen", r#"["127.0.0.1:3478"]"#, r#"["127.0.0.1:3479"]"#),
            f("listen_tcp", "[]", r#"["127.0.0.1:3478"]"#),
            fx(
                "listen_tls",
                r#"["127.0.0.1:5349"]"#,
                r#"["127.0.0.1:5350"]"#,
                &[("tls", &tls("c.pem"))],
            ),
            f("mptcp", "true", "false"),
            fx(
                "tls",
                tls("a.pem"),
                tls("b.pem"),
                &[("listen_tls", r#"["127.0.0.1:5349"]"#)],
            ),
            f("access", "{}", r#"{"allow": ["10.0.0.0/8"]}"#),
            f("realm", q("r"), q("q")),
            f(
                "users",
                r#"[{"name": "a", "password": "p"}]"#,
                r#"[{"name": "b", "password": "p"}]"#,
            ),
            f("secret", q("s"), q("t")),
            f("relay_ip", q("10.0.0.5"), q("10.0.0.6")),
            f("public_ip", q("203.0.113.5"), q("203.0.113.6")),
            fx("port_min", "1000", "1001", &[("port_max", "2000")]),
            fx("port_max", "2000", "2001", &[("port_min", "1000")]),
            f("max_allocations", "1", "2"),
            f("max_allocations_per_user", "1", "2"),
            f("max_permissions", "1", "2"),
            f("max_channels", "1", "2"),
            f("max_stream_message", "4096", "4097"),
            f("stream_idle_timeout", q("1s"), q("2s")),
            f("user_bandwidth", "1", "2"),
            f("default_lifetime", q("1m"), q("2m")),
            f("max_lifetime", q("1h"), q("2h")),
            f("nonce_lifetime", q("1m"), q("2m")),
            f("nonce_secret", q("a"), q("b")),
            f("allowed_peers", r#"["10.0.0.0/8"]"#, r#"["10.0.0.0/9"]"#),
            f("denied_peers", r#"["10.0.0.0/8"]"#, r#"["10.0.0.0/9"]"#),
        ],
    );
    let base =
        r#""listen": ["127.0.0.1:3478"], "realm": "r", "relay_ip": "10.0.0.5", "secret": "s""#;
    let tlsw = |m: &str| {
        tw(&format!(
            r#"{{{base}, "listen_tls": ["127.0.0.1:5349"], "tls": {m}}}"#
        ))
    };
    emit(
        out,
        "webrtc.turn.tls",
        &tlsw,
        &[],
        vec![f(
            "certificates",
            r#"[{"cert_file": "a.pem", "key_file": "k.pem"}]"#,
            r#"[{"cert_file": "b.pem", "key_file": "k.pem"}]"#,
        )],
    );
    let cw = |m: &str| tlsw(&format!(r#"{{"certificates": [{m}]}}"#));
    emit(
        out,
        "webrtc.turn.tls.certificates[]",
        &cw,
        &[("cert_file", q("c.pem")), ("key_file", q("k.pem"))],
        vec![
            f("cert_file", q("a.pem"), q("b.pem")),
            f("key_file", q("a.pem"), q("b.pem")),
        ],
    );
    let uw = |m: &str| tw(&format!(r#"{{{base}, "users": [{m}]}}"#));
    emit(
        out,
        "webrtc.turn.users[]",
        &uw,
        &[("name", q("a")), ("password", q("p"))],
        vec![
            f("name", q("a"), q("b")),
            f("password", q("p"), q("q")),
            fx(
                "key",
                q("0123456789abcdef0123456789abcdef"),
                q("0123456789abcdef0123456789abcdee"),
                &[("password", "")],
            ),
        ],
    );
    let aw = |m: &str| tw(&format!(r#"{{{base}, "access": {m}}}"#));
    emit(out, "webrtc.turn.access", &aw, &[], access_fields());
    // The media server; webrtc.turn beside it, whose secret turn_urls needs.
    let mw = |m: &str| w(&format!(r#"{{"turn": {turn_min}, "media": {m}}}"#));
    emit(
        out,
        "webrtc.media",
        &mw,
        &[("listen", q("127.0.0.1:8189"))],
        vec![
            f("listen", q("127.0.0.1:8189"), q("127.0.0.1:8188")),
            f("public_ips", r#"["203.0.113.5"]"#, r#"["203.0.113.6"]"#),
            f("access", "{}", r#"{"allow": ["10.0.0.0/8"]}"#),
            f(
                "ice_servers",
                r#"[{"urls": ["stun:a"]}]"#,
                r#"[{"urls": ["stun:b"]}]"#,
            ),
            f("turn_urls", r#"["turn:a"]"#, r#"["turn:b"]"#),
            f("turn_credential_ttl", q("1m"), q("2m")),
            f("codecs", r#"["opus"]"#, r#"["VP8"]"#),
            f("max_peer_connections", "1", "2"),
            f("max_streams", "1", "2"),
            f("max_viewers", "1", "2"),
            f("max_bitrate", "1", "2"),
            f("max_sdp_bytes", "1", "2"),
            f("connect_timeout", q("1s"), q("2s")),
            f("ice_timeout", q("1s"), q("2s")),
            f("key_frame_interval", q("1s"), q("2s")),
        ],
    );
    let maw = |m: &str| mw(&format!(r#"{{"listen": "127.0.0.1:8189", "access": {m}}}"#));
    emit(out, "webrtc.media.access", &maw, &[], access_fields());
    let iw = |m: &str| {
        mw(&format!(
            r#"{{"listen": "127.0.0.1:8189", "ice_servers": [{m}]}}"#
        ))
    };
    emit(
        out,
        "webrtc.media.ice_servers[]",
        &iw,
        &[("urls", r#"["stun:a"]"#.to_owned())],
        vec![
            f("urls", r#"["stun:a"]"#, r#"["stun:b"]"#),
            f("username", q("a"), q("b")),
            f("credential", q("a"), q("b")),
        ],
    );
    // The proxy.
    let pw = |m: &str| w(&format!(r#"{{"proxy": {m}}}"#));
    emit(
        out,
        "webrtc.proxy",
        &pw,
        &[("listen", q("127.0.0.1:8190"))],
        vec![
            f("listen", q("127.0.0.1:8190"), q("127.0.0.1:8191")),
            f("public_ips", r#"["203.0.113.5"]"#, r#"["203.0.113.6"]"#),
            f("access", "{}", r#"{"allow": ["10.0.0.0/8"]}"#),
            f("max_sessions", "1", "2"),
            f("max_relays", "1", "2"),
            f("session_timeout", q("1s"), q("2s")),
            f("idle_timeout", q("1s"), q("2s")),
            f("relay_ip", q("10.0.0.5"), q("10.0.0.6")),
            fx("port_min", "1000", "1001", &[("port_max", "2000")]),
            fx("port_max", "2000", "2001", &[("port_min", "1000")]),
        ],
    );
    let paw = |m: &str| pw(&format!(r#"{{"listen": "127.0.0.1:8190", "access": {m}}}"#));
    emit(out, "webrtc.proxy.access", &paw, &[], access_fields());
    // The handlers.
    let route = |module: &str, params: &str| {
        format!(
            r#"{{"listeners": [{{"name": "web", "address": ":8080"}}], "routes": [{{"handler": {{"module": "{module}"{params}}}}}]}}"#
        )
    };
    for h in ["whip", "whep"] {
        let other = if h == "whip" { "whep" } else { "whip" };
        out.push((
            format!("routes[].handler<{h}>"),
            route(h, ""),
            Read::Differs(route(other, "")),
        ));
        out.push((
            format!("routes[].handler<{h}>.params"),
            route(h, ""),
            Read::Differs(route(h, r#", "params": {"stream": "a"}"#)),
        ));
        out.push((
            format!("routes[].handler<{h}>.params.stream"),
            route(h, r#", "params": {"stream": "a"}"#),
            Read::Differs(route(h, r#", "params": {"stream": "b"}"#)),
        ));
        // I126: a media_auth policy.
        let auth = |a: &str| {
            format!(
                r#"{{"listeners": [{{"name": "web", "address": ":8080"}}], "media_auth": [{{"name": "p", "public": ["play"]}}, {{"name": "q", "public": ["publish"]}}], "routes": [{{"handler": {{"module": "{h}", "params": {{"auth": "{a}"}}}}}}]}}"#
            )
        };
        out.push((
            format!("routes[].handler<{h}>.params.auth"),
            auth("p"),
            Read::Differs(auth("q")),
        ));
    }
    let wp = |m: &str| route("webrtc_proxy", &format!(r#", "params": {m}"#));
    out.push((
        "routes[].handler<webrtc_proxy>".into(),
        wp(r#"{"upstream": "m"}"#),
        Read::Differs(route("whip", "")),
    ));
    out.push((
        "routes[].handler<webrtc_proxy>.params".into(),
        wp(r#"{"upstream": "m"}"#),
        Read::Differs(wp(r#"{"upstream": "m", "timeout": "1s"}"#)),
    ));
    emit(
        out,
        "routes[].handler<webrtc_proxy>.params",
        &wp,
        &[("upstream", q("m"))],
        vec![
            f("upstream", q("m"), q("n")),
            f(
                "media_addrs",
                r#"{"10.0.0.2:8080": "10.0.0.2"}"#,
                r#"{"10.0.0.2:8080": "10.0.0.3"}"#,
            ),
            f("timeout", q("1s"), q("2s")),
            f("max_sdp_bytes", "1", "2"),
            f("action", q("publish"), q("play")),
        ],
    );
    // I126: `auth`, a media_auth policy, with its action.
    let pa = |a: &str| {
        format!(
            r#"{{"listeners": [{{"name": "web", "address": ":8080"}}], "upstreams": [{{"name": "m", "addresses": ["10.0.0.2:8080"]}}], "media_auth": [{{"name": "p", "public": ["play"]}}, {{"name": "q", "public": ["publish"]}}], "routes": [{{"handler": {{"module": "webrtc_proxy", "params": {{"upstream": "m", "action": "publish", "auth": "{a}"}}}}}}]}}"#
        )
    };
    out.push((
        "routes[].handler<webrtc_proxy>.params.auth".into(),
        pa("p"),
        Read::Differs(pa("q")),
    ));
}

/// I150: `bittorrent`'s client and tracker.
fn bittorrent(out: &mut Vec<(String, String, Read)>) {
    out.push((
        "bittorrent".into(),
        r#"{"bittorrent": {}}"#.into(),
        Read::Differs(r#"{"bittorrent": {"tracker": {}}}"#.into()),
    ));
    let client = |m: &str| format!(r#"{{"bittorrent": {{"client": {m}}}}}"#);
    out.push((
        "bittorrent.client".into(),
        r#"{"bittorrent": {"tracker": {}}}"#.into(),
        Read::Differs(client(r#"{"listen": "127.0.0.1:6881", "data_dir": "d"}"#)),
    ));
    let need = [("listen", q("127.0.0.1:6881")), ("data_dir", q("d"))];
    emit(
        out,
        "bittorrent.client",
        &|m: &str| client(m),
        &need,
        vec![
            f("listen", q("127.0.0.1:6881"), q("127.0.0.1:6882")),
            f("data_dir", q("a"), q("b")),
            f("download_dir", q("a"), q("b")),
            f("encryption", q("allow"), q("require")),
            f("dht", "true", "false"),
            f("pex", "true", "false"),
            f("utp", "true", "false"),
            f("lsd", "true", "false"),
            f("disk_workers", "1", "2"),
            f("save_interval", q("1s"), q("2s")),
            f("limits", "{}", r#"{"upload_rate": 1}"#),
            f("console", console(7080), console(7081)),
            f("categories", "[]", r#"[{"name": "a"}]"#),
            f("watch_dir", q("a"), q("b")),
            f("watch_interval", q("1s"), q("2s")),
            f("watch_max_files", "1", "2"),
            f("url_timeout", q("1s"), q("2s")),
            f("alt_speed", "{}", r#"{"upload_rate": 1}"#),
            f("publish", "[]", r#"[{"name": "a", "dir": "/srv/a"}]"#),
        ],
    );
    let pw = |m: &str| {
        client(&format!(
            r#"{{"listen": "127.0.0.1:6881", "data_dir": "d", "publish": [{m}]}}"#
        ))
    };
    emit(
        out,
        "bittorrent.client.publish[]",
        &pw,
        &[("name", q("a")), ("dir", q("/srv/a"))],
        vec![
            f("name", q("a"), q("b")),
            f("dir", q("/srv/a"), q("/srv/b")),
            f("mode", q("per_file"), q("whole_dir")),
            f("web_seed", q("http://x/a/"), q("http://y/a/")),
            f("announce", r#"["http://x/announce"]"#, r#"["udp://x:1"]"#),
            f("private", "true", "false"),
            f("piece_length", "16384", "32768"),
            f("files", r#"["*.iso"]"#, r#"["*.img"]"#),
            f("exclude", r#"["*.part"]"#, r#"["*.tmp"]"#),
            f("check_interval", q("1s"), q("2s")),
            f("max_files", "1", "2"),
            f("max_total_bytes", "1", "2"),
            f("max_hashing", "1", "2"),
        ],
    );
    let aw = |m: &str| {
        client(&format!(
            r#"{{"listen": "127.0.0.1:6881", "data_dir": "d", "alt_speed": {m}}}"#
        ))
    };
    emit(
        out,
        "bittorrent.client.alt_speed",
        &aw,
        &[],
        vec![
            f("upload_rate", "1", "2"),
            f("download_rate", "1", "2"),
            f("schedules", "[]", r#"[{"from": "01:00", "to": "02:00"}]"#),
            f("time_zone", q("utc"), q("local")),
        ],
    );
    let sw = |m: &str| {
        client(&format!(
            r#"{{"listen": "127.0.0.1:6881", "data_dir": "d", "alt_speed": {{"schedules": [{m}]}}}}"#
        ))
    };
    emit(
        out,
        "bittorrent.client.alt_speed.schedules[]",
        &sw,
        &[("from", q("01:00")), ("to", q("02:00"))],
        vec![
            f("days", r#"["mon"]"#, r#"["tue"]"#),
            f("from", q("01:00"), q("01:30")),
            f("to", q("02:00"), q("03:00")),
        ],
    );
    let clw = |m: &str| {
        client(&format!(
            r#"{{"listen": "127.0.0.1:6881", "data_dir": "d", "categories": [{{"name": "a", "limits": {m}}}]}}"#
        ))
    };
    emit(
        out,
        "bittorrent.client.categories[].limits",
        &clw,
        &[],
        vec![
            f("upload_rate", "1", "2"),
            f("download_rate", "1", "2"),
            f("seed_ratio", "1", "2"),
            f("seed_time", q("1h"), q("2h")),
        ],
    );
    let catw = |m: &str| {
        client(&format!(
            r#"{{"listen": "127.0.0.1:6881", "data_dir": "d", "categories": [{m}]}}"#
        ))
    };
    emit(
        out,
        "bittorrent.client.categories[]",
        &catw,
        &[("name", q("a"))],
        vec![
            f("name", q("a"), q("b")),
            f("dir", q("a"), q("b")),
            f("limits", "{}", r#"{"upload_rate": 1}"#),
        ],
    );
    let lw = |m: &str| {
        client(&format!(
            r#"{{"listen": "127.0.0.1:6881", "data_dir": "d", "limits": {m}}}"#
        ))
    };
    emit(
        out,
        "bittorrent.client.limits",
        &lw,
        &[],
        vec![
            f("upload_rate", "1", "2"),
            f("download_rate", "1", "2"),
            f("max_conns_per_torrent", "1", "2"),
            f("upload_slots", "1", "2"),
            f("seed_ratio", "1", "1.5"),
            f("seed_time", q("1h"), q("2h")),
            f("max_torrents", "1", "2"),
            f("max_active_downloads", "1", "2"),
            f("max_active_seeds", "1", "2"),
            f("min_free_bytes", "1", "2"),
        ],
    );
    let cw = |m: &str| {
        client(&format!(
            r#"{{"listen": "127.0.0.1:6881", "data_dir": "d", {m}}}"#
        ))
    };
    console_and_access(out, "bittorrent.client", &cw, false);
    // The tracker.
    let tracker = |m: &str| format!(r#"{{"bittorrent": {{"tracker": {m}}}}}"#);
    out.push((
        "bittorrent.tracker".into(),
        r#"{"bittorrent": {}}"#.into(),
        Read::Differs(tracker("{}")),
    ));
    let h40a = "ab".repeat(20);
    let h40b = "cd".repeat(20);
    emit(
        out,
        "bittorrent.tracker",
        &|m: &str| tracker(m),
        &[],
        vec![
            f("listen", q("127.0.0.1:6969"), q("127.0.0.1:6970")),
            f("interval", q("1m"), q("2m")),
            f("min_interval", q("1s"), q("2s")),
            f("peer_ttl", q("2h"), q("3h")),
            f("max_torrents", "1", "2"),
            f("max_peers_per_torrent", "1", "2"),
            f("max_numwant", "1", "2"),
            f("max_scrape", "1", "2"),
            f(
                "allow",
                format!("[{}]", q(&h40a)),
                format!("[{}]", q(&h40b)),
            ),
            f("allow_files", r#"["a"]"#, r#"["b"]"#),
            f("access", "{}", r#"{"allow": ["10.0.0.0/8"]}"#),
            f("console", console(7080), console(7081)),
        ],
    );
    // I159: a private tracker, with a policy it may name.
    let ta = |m: &str| {
        format!(
            r#"{{"media_auth": [{{"name": "p", "public": ["play"]}}], "bittorrent": {{"tracker": {m}}}}}"#
        )
    };
    emit(
        out,
        "bittorrent.tracker",
        &ta,
        &[],
        vec![f("auth", q("p"), q(""))],
    );
    let tw = |m: &str| tracker(&format!("{{{m}}}"));
    console_and_access(out, "bittorrent.tracker", &tw, true);
}

/// `media_auth` and `streaming` (I120).
fn media(out: &mut Vec<(String, String, Read)>) {
    const P: &str = r#"{"name": "p", "public": ["play"]}"#;
    out.push((
        "media_auth".into(),
        r#"{"media_auth": []}"#.into(),
        Read::Differs(format!(r#"{{"media_auth": [{P}]}}"#)),
    ));
    let pw = |m: &str| format!(r#"{{"media_auth": [{m}]}}"#);
    emit(
        out,
        "media_auth[]",
        &pw,
        &[("name", q("p")), ("public", r#"["play"]"#.into())],
        vec![
            f("name", q("a"), q("b")),
            f("public", r#"["play"]"#, r#"["publish"]"#),
            f("secret", q("0123456789abcdef"), q("0123456789abcdeg")),
            f("secret_file", q("a"), q("b")),
            f("token_ttl", q("1h"), q("2h")),
            f("tokens", "[]", r#"[{"token": "0123456789abcdef"}]"#),
            f("users", "[]", r#"[{"name": "u", "password": "p"}]"#),
        ],
    );
    let tw = |m: &str| pw(&format!(r#"{{"name": "p", "tokens": [{m}]}}"#));
    emit(
        out,
        "media_auth[].tokens[]",
        &tw,
        &[("token", q("0123456789abcdef"))],
        vec![
            f("name", q("a"), q("b")),
            f("token", q("0123456789abcdef"), q("0123456789abcdeg")),
            fx("token_sha256", q(H64), q(C64), &[("token", "")]),
            f("publish", r#"["a"]"#, r#"["b"]"#),
            f("play", r#"["a"]"#, r#"["b"]"#),
        ],
    );
    let uw = |m: &str| pw(&format!(r#"{{"name": "p", "users": [{m}]}}"#));
    emit(
        out,
        "media_auth[].users[]",
        &uw,
        &[("name", q("u")), ("password", q("p"))],
        vec![
            f("name", q("a"), q("b")),
            f("password", q("a"), q("b")),
            fx("password_hash", q(HASH_A), q(HASH_B), &[("password", "")]),
            f("publish", r#"["a"]"#, r#"["b"]"#),
            f("play", r#"["a"]"#, r#"["b"]"#),
        ],
    );
    // The role, with a policy its listeners may name.
    let sw = |m: &str| format!(r#"{{"media_auth": [{P}], "streaming": {m}}}"#);
    let rtmp = r#"{"listen": "127.0.0.1:1935"}"#;
    out.push((
        "streaming".into(),
        sw(&format!(r#"{{"rtmp": {rtmp}}}"#)),
        Read::Differs(sw(&format!(r#"{{"rtmp": {rtmp}, "max_streams": 1}}"#))),
    ));
    emit(
        out,
        "streaming",
        &sw,
        &[("rtmp", rtmp.into())],
        vec![
            f("max_streams", "1", "2"),
            f("max_players", "1", "2"),
            f("gop_cache_bytes", "1", "2"),
            f(
                "rtmp",
                r#"{"listen": "127.0.0.1:1935"}"#,
                r#"{"listen": "127.0.0.1:1936"}"#,
            ),
            f(
                "rtsp",
                r#"{"listen": "127.0.0.1:8554"}"#,
                r#"{"listen": "127.0.0.1:8555"}"#,
            ),
            f(
                "srt",
                r#"{"listen": "127.0.0.1:8890"}"#,
                r#"{"listen": "127.0.0.1:8891"}"#,
            ),
            f(
                "webrtc",
                r#"{"listen": "127.0.0.1:8188"}"#,
                r#"{"listen": "127.0.0.1:8189"}"#,
            ),
            f("access", "{}", r#"{"allow": ["10.0.0.0/8"]}"#),
        ],
    );
    let aw = |m: &str| sw(&format!(r#"{{"rtmp": {rtmp}, "access": {m}}}"#));
    emit(out, "streaming.access", &aw, &[], access_fields());
    for (name, port, extra) in [
        (
            "rtmp",
            1935_u16,
            vec![
                f("handshake_timeout", q("1s"), q("2s")),
                f("idle_timeout", q("1s"), q("2s")),
            ],
        ),
        ("rtsp", 8554, vec![f("session_timeout", q("1s"), q("2s"))]),
        (
            "srt",
            8890,
            vec![
                f("latency", q("1s"), q("2s")),
                f("passphrase", q("0123456789"), q("0123456780")),
                f("peer_idle_timeout", q("1s"), q("2s")),
            ],
        ),
    ] {
        let lw = |m: &str| sw(&format!(r#"{{"{name}": {m}}}"#));
        let mut fields = vec![
            f(
                "listen",
                q(&format!("127.0.0.1:{port}")),
                q(&format!("127.0.0.1:{}", port.saturating_add(1))),
            ),
            f("auth", q("p"), q("")),
            f("max_conns", "1", "2"),
            f("play_wait", q("1s"), q("2s")),
        ];
        fields.extend(extra);
        emit(
            out,
            &format!("streaming.{name}"),
            &lw,
            &[("listen", q(&format!("127.0.0.1:{port}")))],
            fields,
        );
    }
    let ww = |m: &str| sw(&format!(r#"{{"webrtc": {m}}}"#));
    emit(
        out,
        "streaming.webrtc",
        &ww,
        &[("listen", q("127.0.0.1:8188"))],
        vec![
            f("listen", q("127.0.0.1:8188"), q("127.0.0.1:8189")),
            f("public_ips", r#"["10.0.0.1"]"#, r#"["10.0.0.2"]"#),
            f("ice_servers", "[]", r#"[{"urls": ["stun:a"]}]"#),
            f("max_sessions", "1", "2"),
            f("connect_timeout", q("1s"), q("2s")),
            f("play_wait", q("1s"), q("2s")),
            f("max_offer_bytes", "1", "2"),
        ],
    );
    let iw = |m: &str| {
        sw(&format!(
            r#"{{"webrtc": {{"listen": "127.0.0.1:8188", "ice_servers": [{m}]}}}}"#
        ))
    };
    emit(
        out,
        "streaming.webrtc.ice_servers[]",
        &iw,
        &[("urls", r#"["stun:a"]"#.into())],
        vec![
            f("urls", r#"["stun:a"]"#, r#"["stun:b"]"#),
            f("username", q("a"), q("b")),
            f("credential", q("a"), q("b")),
        ],
    );
}

/// The `httpflv` and `hls` handlers' params (I123), `stream_whip` and `stream_whep`'s (I124).
fn media_handlers(out: &mut Vec<(String, String, Read)>) {
    const P: &str = r#"{"name": "p", "public": ["play"]}"#;
    for (module, fields) in [
        (
            "httpflv",
            vec![
                f("prefix", q("/a/"), q("/b/")),
                f("auth", q("p"), q("")),
                f("wait", q("1s"), q("2s")),
                f("cors", q("*"), q("https://a")),
            ],
        ),
        (
            "hls",
            vec![
                f("prefix", q("/a/"), q("/b/")),
                f("auth", q("p"), q("")),
                f("format", q("ts"), q("fmp4")),
                f("target_duration", q("1s"), q("2s")),
                f("part_duration", q("200ms"), q("300ms")),
                f("playlist_size", "3", "4"),
                f("max_bytes", "1", "2"),
                f("play_wait", q("1s"), q("2s")),
                f("idle_timeout", q("1s"), q("2s")),
                f("cors", q("*"), q("https://a")),
            ],
        ),
        // I124.
        (
            "stream_whip",
            vec![
                f("stream", q("live/a"), q("live/b")),
                f("prefix", q("/a/"), q("/b/")),
                f("auth", q("p"), q("")),
            ],
        ),
        (
            "stream_whep",
            vec![
                f("stream", q("live/a"), q("live/b")),
                f("prefix", q("/a/"), q("/b/")),
                f("auth", q("p"), q("")),
            ],
        ),
    ] {
        let w = |m: &str| {
            format!(
                r#"{{"listeners": [{{"name": "web", "address": ":8080"}}], "media_auth": [{P}], "routes": [{{"handler": {{"module": "{module}", "params": {m}}}}}]}}"#
            )
        };
        let wm = |m: &str| {
            format!(
                r#"{{"listeners": [{{"name": "web", "address": ":8080"}}], "routes": [{{"handler": {m}}}]}}"#
            )
        };
        out.push((
            format!("routes[].handler<{module}>"),
            wm(&format!(r#"{{"module": "{module}"}}"#)),
            Read::Differs(wm(r#"{"module": "respond"}"#)),
        ));
        out.push((
            format!("routes[].handler<{module}>.params"),
            w("{}"),
            Read::Differs(w(r#"{"prefix": "/x/"}"#)),
        ));
        emit(
            out,
            &format!("routes[].handler<{module}>.params"),
            &w,
            &[],
            fields,
        );
    }
}

/// The `media_pages` handler's params (I125).
fn media_pages(out: &mut Vec<(String, String, Read)>) {
    const P: &str = r#"{"name": "p", "public": ["play"]}"#;
    let w = |m: &str| {
        format!(
            r#"{{"listeners": [{{"name": "web", "address": ":8080"}}], "media_auth": [{P}], "routes": [{{"handler": {{"module": "media_pages", "params": {m}}}}}]}}"#
        )
    };
    let wm = |m: &str| {
        format!(
            r#"{{"listeners": [{{"name": "web", "address": ":8080"}}], "routes": [{{"handler": {m}}}]}}"#
        )
    };
    out.push((
        "routes[].handler<media_pages>".into(),
        wm(r#"{"module": "media_pages"}"#),
        Read::Differs(wm(r#"{"module": "respond"}"#)),
    ));
    out.push((
        "routes[].handler<media_pages>.params".into(),
        w("{}"),
        Read::Differs(w(r#"{"prefix": "/x"}"#)),
    ));
    emit(
        out,
        "routes[].handler<media_pages>.params",
        &w,
        &[],
        vec![
            f("prefix", q("/a"), q("/b")),
            f("title", q("a"), q("b")),
            f("auth", q("p"), q("")),
            f("login", "true", "false"),
            f("pages", "false", "true"),
            f("whip", q("/a/{stream}"), q("/b/{stream}")),
            f("whep", q("/a/{stream}"), q("/b/{stream}")),
            f("hls", q("/a/{stream}"), q("/b/{stream}")),
            f("links", "[]", r#"[{"name": "a", "url": "b"}]"#),
            f("ice_servers", "[]", r#"[{"urls": ["stun:a"]}]"#),
            f("max_rooms", "1", "2"),
            f("max_members", "1", "2"),
            f("max_message_bytes", "300", "400"),
            f("mic_default", "true", "false"),
            f("cam_default", "true", "false"),
            f("chat", "false", "true"),
            f("max_file_bytes", "2048", "4096"),
            f("member_ttl", q("3s"), q("4s")),
            f("stall_timeout", q("1s"), q("2s")),
        ],
    );
    let lw = |m: &str| w(&format!(r#"{{"links": [{m}]}}"#));
    emit(
        out,
        "routes[].handler<media_pages>.params.links[]",
        &lw,
        &[("name", q("a")), ("url", q("b"))],
        vec![f("name", q("a"), q("c")), f("url", q("b"), q("d"))],
    );
    let iw = |m: &str| w(&format!(r#"{{"ice_servers": [{m}]}}"#));
    emit(
        out,
        "routes[].handler<media_pages>.params.ice_servers[]",
        &iw,
        &[("urls", r#"["stun:a"]"#.into())],
        vec![
            f("urls", r#"["stun:a"]"#, r#"["stun:b"]"#),
            f("username", q("a"), q("b")),
            f("credential", q("a"), q("b")),
        ],
    );
}

/// Every sample of the roles' fields.
pub(crate) fn samples() -> Vec<(String, String, Read)> {
    let mut out = Vec::new();
    tunnel(&mut out);
    llm(&mut out);
    webrtc(&mut out);
    bittorrent(&mut out);
    media(&mut out);
    media_handlers(&mut out);
    media_pages(&mut out);
    media_library(&mut out);
    vod(&mut out);
    media_tokens(&mut out);
    out
}

/// `media_auth`'s `keys` and the `media_tokens` handler's params (I143), and `url_signing`
/// (I144).
fn media_tokens(out: &mut Vec<(String, String, Read)>) {
    let kw = |m: &str| format!(r#"{{"media_auth": [{{"name": "p", "keys": [{m}]}}]}}"#);
    out.push((
        "media_auth[].keys".into(),
        r#"{"media_auth": [{"name": "p", "keys": [{"key": "a", "secret": "0123456789abcdef"}]}]}"#.into(),
        Read::Differs(r#"{"media_auth": [{"name": "p", "keys": [{"key": "b", "secret": "0123456789abcdef"}]}]}"#.into()),
    ));
    emit(
        out,
        "media_auth[].keys[]",
        &kw,
        &[("key", q("a")), ("secret", q("0123456789abcdef"))],
        vec![
            f("key", q("a"), q("b")),
            f("secret", q("0123456789abcdef"), q("0123456789abcdeg")),
            fx("secret_file", q("a"), q("b"), &[("secret", "")]),
        ],
    );
    const P: &str = r#"{"name": "p", "secret_file": "/s"}"#;
    const MW: &str = r#"[{"module": "basic_auth", "params": {"users": [{"username": "u", "password_hash": "$2y$04$n.ddWQ.hfkNRsx7o3Y1w3eongqIP2oVF8Ka9OEQQwsoVnWY6O.jlW"}]}}]"#;
    let w = |m: &str| {
        format!(
            r#"{{"listeners": [{{"name": "web", "address": ":8080"}}], "media_auth": [{P}, {{"name": "q", "secret_file": "/t"}}], "routes": [{{"middleware": {MW}, "handler": {{"module": "media_tokens", "params": {m}}}}}]}}"#
        )
    };
    out.push((
        "routes[].handler<media_tokens>".into(),
        w(r#"{"auth": "p"}"#),
        Read::Differs(format!(
            r#"{{"listeners": [{{"name": "web", "address": ":8080"}}], "media_auth": [{P}], "routes": [{{"middleware": {MW}, "handler": {{"module": "respond"}}}}]}}"#
        )),
    ));
    out.push((
        "routes[].handler<media_tokens>.params".into(),
        w(r#"{"auth": "p"}"#),
        Read::Differs(w(r#"{"auth": "p", "prefix": "/x"}"#)),
    ));
    emit(
        out,
        "routes[].handler<media_tokens>.params",
        &w,
        &[("auth", q("p"))],
        vec![
            f("prefix", q("/a"), q("/b")),
            f("auth", q("p"), q("q")),
            f("prefixes", r#"["a-"]"#, r#"["b-"]"#),
            f("max_ttl", q("1h"), q("2h")),
            f("max_tokens", "1", "2"),
            f("max_body_bytes", "256", "512"),
            f("revoke_timeout", q("1s"), q("2s")),
            f(
                "url_base",
                q("https://a.example.com"),
                q("https://b.example.com"),
            ),
            f("url_prefixes", r#"["/a/"]"#, r#"["/b/"]"#),
        ],
    );
    // I144: url_signing.
    out.push((
        "media_auth[].url_signing".into(),
        r#"{"media_auth": [{"name": "p", "secret": "0123456789abcdef"}]}"#.into(),
        Read::Differs(
            r#"{"media_auth": [{"name": "p", "secret": "0123456789abcdef", "url_signing": {}}]}"#
                .into(),
        ),
    ));
    let u = |m: &str| {
        format!(
            r#"{{"media_auth": [{{"name": "p", "secret": "0123456789abcdef", "url_signing": {m}}}]}}"#
        )
    };
    emit(
        out,
        "media_auth[].url_signing",
        &u,
        &[],
        vec![
            f("ttl", q("1m"), q("2m")),
            f("bind_address", q("ip"), q("subnet")),
            f("play_grace", q("1m"), q("2m")),
            f("cookie", "true", "false"),
            f("max_sessions", "1", "2"),
            f("on_limit", q("refuse"), q("end_oldest")),
            f("session_idle", q("1m"), q("2m")),
            f("max_tracked_sessions", "1", "2"),
            f(
                "referers",
                r#"["https://a.example.com"]"#,
                r#"["https://b.example.com"]"#,
            ),
            f(
                "origins",
                r#"["https://a.example.com"]"#,
                r#"["https://b.example.com"]"#,
            ),
            f("trusted_proxies", r#"["10.0.0.1"]"#, r#"["10.0.0.2"]"#),
        ],
    );
}

/// The `vod` handler's params (I142).
fn vod(out: &mut Vec<(String, String, Read)>) {
    const P: &str = r#"{"name": "p", "public": ["play"]}"#;
    const L: &str = r#"[{"name": "l", "roots": ["/srv/a"]}, {"name": "m", "roots": ["/srv/b"]}]"#;
    let w = |m: &str| {
        format!(
            r#"{{"listeners": [{{"name": "web", "address": ":8080"}}], "media_auth": [{P}], "media_library": {L}, "routes": [{{"handler": {{"module": "vod", "params": {m}}}}}]}}"#
        )
    };
    out.push((
        "routes[].handler<vod>".into(),
        w(r#"{"library": "l"}"#),
        Read::Differs(format!(
            r#"{{"listeners": [{{"name": "web", "address": ":8080"}}], "media_auth": [{P}], "media_library": {L}, "routes": [{{"handler": {{"module": "respond"}}}}]}}"#
        )),
    ));
    out.push((
        "routes[].handler<vod>.params".into(),
        w(r#"{"library": "l"}"#),
        Read::Differs(w(r#"{"library": "l", "prefix": "/x/"}"#)),
    ));
    emit(
        out,
        "routes[].handler<vod>.params",
        &w,
        &[("library", q("l"))],
        vec![
            f("library", q("l"), q("m")),
            f("prefix", q("/a/"), q("/b/")),
            f("auth", q("p"), q("")),
            f("container", q("ts"), q("fmp4")),
            f("segment_duration", q("2s"), q("3s")),
            f("max_cache_bytes", "1048576", "2097152"),
            f("max_segment_bytes", "65536", "131072"),
            f("cache_control", q("public"), q("private")),
            f("cors", q("*"), q("https://a")),
            f("timeout", q("1s"), q("2s")),
        ],
    );
}

/// `media_library` (I140).
fn media_library(out: &mut Vec<(String, String, Read)>) {
    const L: &str = r#"{"name": "l", "roots": ["/srv/a"]}"#;
    out.push((
        "media_library".into(),
        r#"{"media_library": []}"#.into(),
        Read::Differs(format!(r#"{{"media_library": [{L}]}}"#)),
    ));
    let lw = |m: &str| format!(r#"{{"media_library": [{m}]}}"#);
    emit(
        out,
        "media_library[]",
        &lw,
        &[("name", q("l")), ("roots", r#"["/srv/a"]"#.into())],
        vec![
            f("name", q("a"), q("b")),
            f("roots", r#"["/srv/a"]"#, r#"["/srv/b"]"#),
            f("files", r#"["*.mp4"]"#, r#"["*.m4v"]"#),
            f("exclude", r#"["a"]"#, r#"["b"]"#),
            f("follow_symlinks", "false", "true"),
            f("scan_interval", q("1m"), q("2m")),
            f("max_items", "1", "2"),
            f("max_files", "1", "2"),
            f("max_catalog_bytes", "1", "2"),
            f("max_probes", "1", "2"),
            f("max_sidecar_bytes", "1", "2"),
            f("max_subtitle_bytes", "1", "2"),
            f("max_table_bytes", "1", "2"),
            f("file_workers", "1", "2"),
            f("file_queue", "1", "2"),
            f("file_timeout", q("1s"), q("2s")),
            f("secret", q("0123456789abcdef"), q("0123456789abcdeg")),
            f("secret_file", q("a"), q("b")),
        ],
    );
}
