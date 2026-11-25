# 010: The eBPF sockmap relay, asked for by a listener's `sockmap`, an akit extension

[English](010-sockmap-relay.md) | [简体中文](010-sockmap-relay.zh-CN.md)

Status: accepted in I51b (2026-11-25); the counters and the splice through akio's own API since
2026-11-26 (akio API request A170, akio's ADR-156).

## Context

akio can splice two TCP sockets in the kernel with an eBPF sockmap (`akio::ebpf::Relay`): once
spliced, what one socket receives is sent on the other without waking userspace. That is what a
`tcp` service does with each connection after its upstream connects, and the roadmap counts it
among the gains akio gives akit over fit. The relay needs `CAP_BPF` and `CAP_NET_ADMIN` (or
`CAP_SYS_ADMIN`) and Linux 6.15 or later (earlier kernels can lose relayed bytes when a send buffer
fills); its sockets' bytes never reach the proxy, so the proxy sees them only through the sockets'
TCP counters.

fit has no such relay and no field for it: fib, its engine, has no sockmap, and fit's `services`
and `listeners` take none. [Decision 002](002-config-compatibility.md) has fit lead the schema: akit
adds no field fit would not accept, and a field for something akio can do and fib cannot is proposed
to fit, which may accept it and ignore it.

## Decision

- **`listeners[].sockmap`, a boolean, false when left out, is an akit extension.** A `tcp`
  listener with it has its services' connections spliced in the kernel where the relay loads, and
  relayed in userspace, as before, where it does not. It is the listener's, not the service's: the
  roadmap has the listener ask for it, a service may take several listeners, and whether to splice
  is a property of the port (its clients, its trust) more than of the upstream. It is refused on a
  listener whose protocols do not include `tcp`. A reload that changes it applies to connections
  accepted after it, without binding the listener again.
- **It is not in fit's inventory.** akit-config lists it apart from fit's fields
  (`LISTENER_AKIT_ONLY`), the `.conf` reader and writer know its type from a list of akit's
  extensions, and the compatibility gate checks that no field so listed is one fit has: should fit
  adopt it, the field moves to fit's list. The field is to be proposed to fit, as decision 002 asks,
  where fib would accept it and relay in userspace, as akit does without the relay; until fit takes
  it, a config that sets it is akit's alone and fit refuses it as an unknown field.
- **Never a failure.** A relay that does not load (no privileges, an old kernel, not Linux) is
  said once on standard error, and the listeners relay in userspace; so does each connection the
  relay cannot take. The config does not change meaning with the host: the bytes, their order, the
  timeouts and the closes are the same either way.
- **akio's relay, read by its counters.** The relay is akio's, loaded once per process where
  listeners are bound or reloaded, never on an I/O thread. A connection is spliced with
  `Relay::tunnel`, which writes what goes first (the service's PROXY header, what the client sent
  before) ahead of the relayed bytes; what it could not write goes through the engine. The
  `Tunnel` it gives reads the sockets' TCP counters against its base: for the idle timeout
  (`Tunnel::moved`, read by the service's sweep; akio's own idle timeout, which never sees the
  bytes, is turned off for a spliced connection) and to close a side only once the kernel has
  forwarded what the other received (`Tunnel::close_after_forward`, on akio-ebpf's drain thread).
  Until 2026-11-26 akit read the counters through the doc-hidden `Conn::inner`, a stopgap
  [akio API request A170](../akio-api-requests.md#a170-a-connections-tcp-counters-and-a-splice-with-a-first-write)
  replaced.

## Consequences

- On Linux 6.15+ with the capabilities, a `tcp` service's bytes stop passing through akit: no
  copies, no wake-ups, no userspace backpressure; the kernel's relay backlog is bounded by the
  sockets' buffers, not by akit's watermarks.
- A config with `sockmap` is not a fit config until fit adopts the field; one without it is
  unchanged. Moving between fit and akit stays a binary swap for configs that do not use it.
- What a spliced connection does is only seen through counters: its idle timeout is checked by the
  service's sweep (a quarter of the timeout, at most a second), not by each byte. Connections
  relayed in userspace are closed when idle by akio itself (A78), the sweep running only while a
  service has spliced connections.
- Connections whose PROXY header the listener read are spliced too: the tunnel's base counts what
  userspace read of the client's socket, the header included. As akio's ordering contract asks, a
  connection is spliced only after userspace read all of what had arrived or none of it: its
  reads are held from the accept, or from the read that brought its ClientHello, and akio reads
  all a socket holds unless a read fills its buffer (64 KiB).
