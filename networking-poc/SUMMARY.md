# Networking POC summary

## What this proof of concept demonstrates

This is a standalone, single-client UDP networking spike for the qfusion port. It defines a new, bounded protocol for handshake, client input, authoritative state snapshots, and clean disconnect. It does not change or interoperate with the existing qfusion/Warsow protocol.

The headless server owns a deliberately simple integer position. On each simulation tick it adds the most recently received input axes; snapshots report that server-owned state and tick. This is only a way to demonstrate input-to-authoritative-state-to-snapshot flow, not a gameplay simulation.

## What was tested

From `networking-poc/`:

- `cargo test --all-targets` passed 11 unit tests and one local subprocess integration test.
- Unit tests cover round trips for each message type, network byte order, sequence wraparound, and rejection of truncated, oversized, malformed, unknown-type, invalid-length, and invalid-field packets.
- The integration test starts a local server with simulation configured to 125 Hz and snapshots to 60 Hz, connects the mock client, sends inputs, checks that server tick and position advance, receives snapshots, and completes a clean disconnect.
- `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, and `git diff --check` passed.

A separate manual local run also completed the lifecycle: the client received 13 snapshots (latest observed tick 41, X=37000), and the server acknowledged a clean disconnect at tick 42, X=38000. These are example-run observations, not performance measurements. Validation was performed on Windows; Linux execution was not available in this environment.

## What was left out

- Production movement physics, collision, prediction, reconciliation, interpolation, game rules, and Bomb and Defuse.
- Multiple clients, session timeouts, admission control, congestion control, bandwidth/loss metrics, snapshot deltas/baselines, and general reliable-message delivery.
- Accounts, matchmaking, external anti-cheat, encryption, or security guarantees. The handshake nonce is only a correlation value.
- Interoperability with existing qfusion/Warsow clients or servers.
- Performance validation of sustained 125 Hz simulation or snapshots, bandwidth or scaling benchmarks, and the planned 200 ms RTT / 3% packet-loss envelope.
- Packet impairment testing. No impairment tooling was available, so latency and loss behavior remain unvalidated.

## Running it

```sh
cargo test
cargo run -- server --bind 127.0.0.1:27960 --sim-hz 125 --snapshot-hz 60
```

In another terminal:

```sh
cargo run -- mock-client 127.0.0.1:27960 --duration-ms 1000 --input-hz 125 --move-x 1000
```

See [`README.md`](README.md) for the protocol layout, validation rules, and additional run options.

## Future simulation integration seam

The POC isolates state stepping behind `simulation::InputFrame`, `AuthoritativeState`, and `step`. A future shared simulation can consume validated input at its fixed tick and expose authoritative state for network snapshot encoding. The transport should not own movement/gameplay rules; the placeholder integer stepping must be replaced rather than treated as the port's movement implementation.
