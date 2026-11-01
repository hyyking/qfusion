# qfusion networking proof of concept

This standalone Rust crate demonstrates a headless UDP server, one mock client, and a deliberately tiny authoritative simulation. It is not a production multiplayer implementation and does not interoperate with qfusion/Warsow protocol clients or servers. The existing engine protocol remains untouched.

## Protocol v1

Every datagram is one complete protocol message. All multibyte values use big-endian (network) byte order.

| Offset | Size | Field |
|---|---:|---|
| 0 | 4 | Magic bytes `QFNP` |
| 4 | 1 | Version, currently `1` |
| 5 | 1 | Message type |
| 6 | 2 | Payload length in bytes |
| 8 | 4 | Direction-local sequence number |
| 12 | 4 | Tick number |
| 16 | variable | Message payload |

The encoded packet is exactly `16 + payload_length` bytes and may not exceed 1200 bytes. The current largest packet is 32 bytes. There is one client per server process; the server binds the accepted peer to the UDP source address and assigns client ID `1`.

| Type | Payload | Meaning |
|---|---|---|
| `HELLO` (`1`) | nonce: `u64` | Client starts a session with sequence `1`, tick `0`. |
| `WELCOME` (`2`) | client ID: `u32`, echoed nonce: `u64` | Server accepts the source address; the echo lets the client match this response to its request. |
| `INPUT` (`3`) | move X: `i16`, move Y: `i16`, buttons: `u16` | Latest input is applied on subsequent server simulation steps. Axes are limited to `-1000..=1000`; buttons must be zero. |
| `SNAPSHOT` (`4`) | client ID: `u32`, acknowledged input sequence: `u32`, X: `i32`, Y: `i32` | Full authoritative state, with the server tick in the header. |
| `DISCONNECT` (`5`) | client ID: `u32` | Client requests session close. |
| `DISCONNECTED` (`6`) | client ID: `u32` | Server acknowledges clean close. |

Client and server each maintain their own sequence stream. A receiver accepts a strictly newer sequence using 32-bit serial-number arithmetic; a difference of exactly `2^31` is not newer. The client starts at sequence `1` for `HELLO`, then increments for input and disconnect. The server starts at `1` for `WELCOME`, then increments for snapshots and the disconnect acknowledgement. Repeated `HELLO` requests with the original nonce are answered again; a duplicate disconnect request is acknowledged again while the server remains running.

The header tick is the sender's current tick: client ticks on inputs are informational and not trusted for simulation; snapshot/control ticks are the server's authoritative simulation tick. The server owns position and advances it with a deliberately artificial integer rule: add the latest input axes once per server tick. This is only test state progression, not qfusion/Warsow movement physics.

Decoders reject truncated, oversized, wrong-magic, unsupported-version, unknown-type, wrong-length, and invalid-field datagrams. The server drops malformed, stale, wrong-peer, and unexpected messages without replying; it does not terminate the process on invalid network data. There is no fragmentation, encryption, authentication, account identity, matchmaking, reliable stream, game mode, or external anti-cheat. The nonce is a handshake correlation value, not a security primitive.

## Run locally

From this directory:

```sh
cargo test
cargo run -- server --bind 127.0.0.1:27960
```

In another terminal:

```sh
cargo run -- mock-client 127.0.0.1:27960 --duration-ms 1000 --input-hz 125 --move-x 1000
```

Stop the server with Ctrl-C after the client reports `disconnect=clean`. The server and client accept explicit rates; simulation and snapshot rates are independent:

```sh
cargo run -- server --bind 127.0.0.1:27960 --sim-hz 125 --snapshot-hz 60
cargo run -- mock-client 127.0.0.1:27960 --duration-ms 1000 --input-hz 125 --move-x 1000
```

For a headless Linux server reachable on its network interfaces, bind explicitly (this unauthenticated POC should only be exposed in a controlled environment):

```sh
cargo run --release -- server --bind 0.0.0.0:27960 --sim-hz 125 --snapshot-hz 125
```

Rates are limited to `1..=1000` Hz. The defaults are 125 Hz for both server rates and client input, but defaults are configuration choices, not a measured throughput or timing claim. `--exit-on-disconnect` is intended for the integration test; `--max-runtime-ms` can bound a local server run. `cargo test` runs protocol/invalid-field unit tests and a local subprocess integration test covering handshake, input, server tick/state progression, independently configured rates, snapshots, and clean disconnect.

## Future shared-simulation seam

The network layer depends only on `simulation::InputFrame`, `simulation::AuthoritativeState`, and `simulation::step`. A future Rust simulation can implement/replace this narrow boundary: consume a validated input frame on its fixed tick and expose authoritative state for snapshot encoding. Keep socket addresses, packet sequences, and encoded message types outside that simulation interface; this prototype's integer step function must not be mistaken for the port's shared movement implementation.

## Measurements and limitations

The integration test proves local message exchange and state progression only. It does not benchmark sustained 125 Hz simulation or snapshots, bandwidth, multi-client scaling, 200 ms RTT, or 3% packet loss. It does not implement movement prediction/reconciliation, snapshot baselines, congestion control, per-client metrics, or game rules. Packet impairment has not been exercised; loss/latency behavior remains unvalidated.
