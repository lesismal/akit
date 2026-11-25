# HTTPS and certificates

[English](https.md) | [简体中文](https.zh-CN.md)

akit serves HTTPS with certificates from files, certificates it obtains and renews with ACME (Let's
Encrypt by default), or both. A certificate is chosen for each handshake by the name the client asks
for; certificate files are loaded again when they change, and ACME's certificates renewed, without
closing a listener. Obtaining a certificate, and a handshake that waits for one, never hold one of
the threads that serve connections. The [reference](../reference/config.md#acme) has every field.

## HTTPS by default

A config that writes no listeners, and whose routes match host names, is served over HTTPS on 443,
with a certificate for each name, and over HTTP on 80, which redirects to HTTPS (308) and answers
ACME's HTTP-01 challenge:

```fit
acme { email ops@example.com }

site example.com www.example.com {
	route /* {
		handler static { root public }
	}
}
```

The names' DNS must point at the host, and ports 80 and 443 reach it. Certificates are kept in
`acme.storage`, `~/.local/share/akit` by default, and renewed by their lifetime or the CA's renewal
information (ARI), their OCSP responses stapled. Names no public CA certifies (`localhost`, IP
addresses, `*.internal`) get none. `auto_https` moves the ports, turns the redirects off, or turns it
all off:

```fit
auto_https { http_port 8080; https_port 8443 }

route { match { host example.com }; handler respond { body hello } }
```

Try a CA's staging directory first, whose limits are loose:
`acme { directory https://acme-staging-v02.api.letsencrypt.org/directory }`.

## Listeners of your own

Write a listener with `tls` to choose its address and protocols. Its `acme` lists the names to obtain
certificates for; `certificates` names files; an empty `tls {}` gets the certificates of the host
names the listener's routes match.

```fit
listener https :443 http/1.1 h2 h3 {
	tls {
		acme example.com www.example.com
		certificates { cert_file certs/example.com.pem; key_file certs/example.com.key }
	}
}

listener http :80

route {
	listeners https
	handler static { root public }
}

route {
	listeners http
	middleware redirect { to https://{host}{uri}; status 308 }
	handler respond
}
```

`h3` adds HTTP/3 on the same port in UDP, announced to HTTP/1.1 and HTTP/2 clients with `Alt-Svc`.
`max_version 1.2` keeps a listener at TLS 1.2.

## Certificates on demand

For names not known ahead of time, as a platform's customers' domains, `on_demand` obtains a
certificate during the first handshake that asks for one, if the `ask` URL allows the name (a 2xx
answer), at most `rate` every `interval`:

```fit
acme {
	email ops@example.com
	on_demand {
		ask http://127.0.0.1:5555/allowed
		rate 10
		interval 1m
	}
}

listener https :443 { tls { on_demand true } }

route { handler respond { body hello } }
```

## Several instances

Instances on several hosts share their certificates through `shared_storage`, in Redis, etcd or
Consul: one obtains a certificate, holding a lock, and the others take it; each challenge's answer is
kept there too, so that whichever instance the CA reaches answers it.

```fit
acme {
	email ops@example.com
	shared_storage {
		etcd { endpoints http://etcd-1:2379 http://etcd-2:2379 }
	}
}

route { match { host example.com }; handler respond { body hello } }
```

## Client certificates

`client_auth` asks clients for certificates and checks them against the CAs in `ca_files` (mutual
TLS): `require` refuses a client without one in the handshake; `verify_if_given` lets it in, and the
`client_cert` middleware asks for one on the routes that need it, passing its subject on.

```fit
listener internal :8443 http/1.1 h2 {
	tls {
		certificates { cert_file certs/example.com.pem; key_file certs/example.com.key }
		client_auth { ca_files certs/ca.pem; mode require }
	}
}

route { handler respond { body "hello, certified client\n" } }
```
