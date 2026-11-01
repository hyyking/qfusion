use qfusion_networking_poc::protocol::{
    decode, encode, sequence_is_newer, Message, Packet, ProtocolError, MAX_DATAGRAM,
};
use qfusion_networking_poc::simulation::{step, AuthoritativeState, InputFrame};
use std::env;
use std::error::Error;
use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::process;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DEFAULT_SERVER_ADDR: &str = "127.0.0.1:27960";
const CONTROL_RETRY: Duration = Duration::from_millis(250);
const MAX_CATCH_UP_TICKS: usize = 8;

fn main() {
    if let Err(error) = run() {
        eprintln!("net-poc: {error}");
        process::exit(2);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("server") => run_server(parse_server_options(args.collect())?),
        Some("mock-client") => run_client(parse_client_options(args.collect())?),
        _ => Err(invalid_input(
            "usage: net-poc server [options] | net-poc mock-client <server-address> [options]",
        )
        .into()),
    }
}

struct ServerOptions {
    bind: SocketAddr,
    sim_hz: u32,
    snapshot_hz: u32,
    max_runtime: Option<Duration>,
    exit_on_disconnect: bool,
}

fn parse_server_options(args: Vec<String>) -> io::Result<ServerOptions> {
    let mut options = ServerOptions {
        bind: DEFAULT_SERVER_ADDR.parse().expect("valid default address"),
        sim_hz: 125,
        snapshot_hz: 125,
        max_runtime: None,
        exit_on_disconnect: false,
    };
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--bind" => options.bind = parse_value(&args, &mut index, "--bind")?,
            "--sim-hz" => {
                options.sim_hz = parse_rate(&args, &mut index, "--sim-hz")?;
            }
            "--snapshot-hz" => {
                options.snapshot_hz = parse_rate(&args, &mut index, "--snapshot-hz")?;
            }
            "--max-runtime-ms" => {
                let millis: u64 = parse_value(&args, &mut index, "--max-runtime-ms")?;
                if millis == 0 {
                    return Err(invalid_input("--max-runtime-ms must be positive"));
                }
                options.max_runtime = Some(Duration::from_millis(millis));
            }
            "--exit-on-disconnect" => {
                options.exit_on_disconnect = true;
                index += 1;
            }
            unknown => return Err(invalid_input(format!("unknown server option {unknown}"))),
        }
    }
    Ok(options)
}

struct ClientOptions {
    server: SocketAddr,
    duration: Duration,
    input_hz: u32,
    move_x: i16,
    move_y: i16,
}

fn parse_client_options(args: Vec<String>) -> io::Result<ClientOptions> {
    let Some(address) = args.first() else {
        return Err(invalid_input("mock-client requires a server address"));
    };
    let server = address
        .parse()
        .map_err(|_| invalid_input("invalid server address"))?;
    let mut options = ClientOptions {
        server,
        duration: Duration::from_millis(1000),
        input_hz: 125,
        move_x: 1000,
        move_y: 0,
    };
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--duration-ms" => {
                let millis: u64 = parse_value(&args, &mut index, "--duration-ms")?;
                if millis == 0 {
                    return Err(invalid_input("--duration-ms must be positive"));
                }
                options.duration = Duration::from_millis(millis);
            }
            "--input-hz" => options.input_hz = parse_rate(&args, &mut index, "--input-hz")?,
            "--move-x" => {
                options.move_x = parse_value(&args, &mut index, "--move-x")?;
            }
            "--move-y" => {
                options.move_y = parse_value(&args, &mut index, "--move-y")?;
            }
            unknown => return Err(invalid_input(format!("unknown client option {unknown}"))),
        }
    }
    if !(-1000..=1000).contains(&options.move_x) || !(-1000..=1000).contains(&options.move_y) {
        return Err(invalid_input("movement axes must be in -1000..=1000"));
    }
    Ok(options)
}

fn parse_value<T: std::str::FromStr>(
    args: &[String],
    index: &mut usize,
    option: &str,
) -> io::Result<T> {
    let value_index = *index + 1;
    let value = args
        .get(value_index)
        .ok_or_else(|| invalid_input(format!("{option} requires a value")))?;
    let parsed = value
        .parse()
        .map_err(|_| invalid_input(format!("invalid value for {option}")))?;
    *index += 2;
    Ok(parsed)
}

fn parse_rate(args: &[String], index: &mut usize, option: &str) -> io::Result<u32> {
    let rate: u32 = parse_value(args, index, option)?;
    if !(1..=1000).contains(&rate) {
        return Err(invalid_input(format!("{option} must be in 1..=1000")));
    }
    Ok(rate)
}

fn invalid_input(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

#[derive(Debug)]
struct ActiveClient {
    address: SocketAddr,
    client_id: u32,
    nonce: u64,
    last_sequence: u32,
    acknowledged_input: u32,
    server_sequence: u32,
    move_x: i16,
    move_y: i16,
}

#[derive(Clone, Copy, Debug)]
struct RecentDisconnect {
    address: SocketAddr,
    client_id: u32,
    client_sequence: u32,
    response_sequence: u32,
    tick: u32,
}

fn run_server(options: ServerOptions) -> Result<(), Box<dyn Error>> {
    let socket = UdpSocket::bind(options.bind)?;
    socket.set_read_timeout(Some(Duration::from_millis(1)))?;
    let local_address = socket.local_addr()?;
    println!(
        "READY {local_address} sim_hz={} snapshot_hz={}",
        options.sim_hz, options.snapshot_hz
    );

    let sim_period = Duration::from_secs_f64(1.0 / f64::from(options.sim_hz));
    let snapshot_period = Duration::from_secs_f64(1.0 / f64::from(options.snapshot_hz));
    let started = Instant::now();
    let mut next_sim = started + sim_period;
    let mut next_snapshot = started + snapshot_period;
    let mut state = AuthoritativeState::default();
    let mut client: Option<ActiveClient> = None;
    let mut recent_disconnect = None;
    let mut receive_buffer = [0u8; MAX_DATAGRAM + 1];

    loop {
        if options
            .max_runtime
            .is_some_and(|max_runtime| started.elapsed() >= max_runtime)
        {
            break;
        }

        match socket.recv_from(&mut receive_buffer) {
            Ok((length, address)) => {
                if let Ok(packet) = decode(&receive_buffer[..length]) {
                    let should_exit = process_server_packet(
                        &socket,
                        address,
                        packet,
                        &mut client,
                        &mut recent_disconnect,
                        state,
                    )?;
                    if should_exit && options.exit_on_disconnect {
                        break;
                    }
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(error) => return Err(error.into()),
        }

        let now = Instant::now();
        let mut catch_up = 0;
        while now >= next_sim && catch_up < MAX_CATCH_UP_TICKS {
            let input = client
                .as_ref()
                .map_or(InputFrame::default(), |active| InputFrame {
                    move_x: active.move_x,
                    move_y: active.move_y,
                });
            step(&mut state, input);
            next_sim += sim_period;
            catch_up += 1;
        }
        if now >= next_sim {
            next_sim = now + sim_period;
        }

        if now >= next_snapshot {
            if let Some(active) = &mut client {
                active.server_sequence = active.server_sequence.wrapping_add(1);
                let bytes = encode(&Packet {
                    sequence: active.server_sequence,
                    tick: state.tick,
                    message: Message::Snapshot {
                        client_id: active.client_id,
                        acknowledged_input: active.acknowledged_input,
                        x: state.x,
                        y: state.y,
                    },
                })?;
                socket.send_to(&bytes, active.address)?;
            }
            while next_snapshot <= now {
                next_snapshot += snapshot_period;
            }
        }
    }

    println!(
        "SERVER stopped tick={} x={} y={}",
        state.tick, state.x, state.y
    );
    Ok(())
}

fn process_server_packet(
    socket: &UdpSocket,
    address: SocketAddr,
    packet: Packet,
    client: &mut Option<ActiveClient>,
    recent_disconnect: &mut Option<RecentDisconnect>,
    state: AuthoritativeState,
) -> io::Result<bool> {
    if client.is_none() {
        if let Some(recent) = recent_disconnect {
            if address == recent.address
                && packet.sequence == recent.client_sequence
                && matches!(
                    &packet.message,
                    Message::Disconnect { client_id } if *client_id == recent.client_id
                )
            {
                let bytes = encode(&Packet {
                    sequence: recent.response_sequence,
                    tick: recent.tick,
                    message: Message::Disconnected {
                        client_id: recent.client_id,
                    },
                })
                .map_err(protocol_io_error)?;
                socket.send_to(&bytes, address)?;
                return Ok(false);
            }
        }
        if packet.sequence != 1
            || packet.tick != 0
            || !matches!(packet.message, Message::Hello { .. })
        {
            return Ok(false);
        }
        let Message::Hello { nonce } = packet.message else {
            unreachable!("hello type verified")
        };
        let active = ActiveClient {
            address,
            client_id: 1,
            nonce,
            last_sequence: packet.sequence,
            acknowledged_input: packet.sequence,
            server_sequence: 1,
            move_x: 0,
            move_y: 0,
        };
        let bytes = encode(&Packet {
            sequence: 1,
            tick: state.tick,
            message: Message::Welcome {
                client_id: active.client_id,
                nonce,
            },
        })
        .map_err(protocol_io_error)?;
        socket.send_to(&bytes, address)?;
        println!(
            "SERVER connected client_id={} tick={}",
            active.client_id, state.tick
        );
        *recent_disconnect = None;
        *client = Some(active);
        return Ok(false);
    }

    let Some(active) = client.as_mut() else {
        return Ok(false);
    };
    if active.address != address {
        return Ok(false);
    }

    if let Message::Hello { nonce } = packet.message {
        if packet.sequence == 1 && packet.tick == 0 && nonce == active.nonce {
            let bytes = encode(&Packet {
                sequence: 1,
                tick: state.tick,
                message: Message::Welcome {
                    client_id: active.client_id,
                    nonce,
                },
            })
            .map_err(protocol_io_error)?;
            socket.send_to(&bytes, address)?;
        }
        return Ok(false);
    }

    if !sequence_is_newer(packet.sequence, active.last_sequence) {
        return Ok(false);
    }
    match packet.message {
        Message::Input {
            move_x,
            move_y,
            buttons: 0,
        } => {
            active.last_sequence = packet.sequence;
            active.acknowledged_input = packet.sequence;
            active.move_x = move_x;
            active.move_y = move_y;
        }
        Message::Disconnect { client_id } if client_id == active.client_id => {
            active.last_sequence = packet.sequence;
            active.server_sequence = active.server_sequence.wrapping_add(1);
            let bytes = encode(&Packet {
                sequence: active.server_sequence,
                tick: state.tick,
                message: Message::Disconnected { client_id },
            })
            .map_err(protocol_io_error)?;
            socket.send_to(&bytes, address)?;
            *recent_disconnect = Some(RecentDisconnect {
                address,
                client_id,
                client_sequence: packet.sequence,
                response_sequence: active.server_sequence,
                tick: state.tick,
            });
            println!(
                "SERVER disconnect=clean client_id={client_id} tick={} x={} y={}",
                state.tick, state.x, state.y
            );
            *client = None;
            return Ok(true);
        }
        _ => {}
    }
    Ok(false)
}

fn protocol_io_error(error: ProtocolError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

fn run_client(options: ClientOptions) -> Result<(), Box<dyn Error>> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.set_read_timeout(Some(Duration::from_millis(1)))?;
    let nonce = (SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos() as u64
        ^ u64::from(process::id()))
    .max(1);
    let mut receive_buffer = [0u8; MAX_DATAGRAM + 1];

    let handshake_started = Instant::now();
    let mut next_hello = Instant::now();
    let client_id = loop {
        let now = Instant::now();
        if now.duration_since(handshake_started) > Duration::from_secs(5) {
            return Err(
                io::Error::new(io::ErrorKind::TimedOut, "server handshake timed out").into(),
            );
        }
        if now >= next_hello {
            send_packet(
                &socket,
                options.server,
                &Packet {
                    sequence: 1,
                    tick: 0,
                    message: Message::Hello { nonce },
                },
            )?;
            next_hello = now + CONTROL_RETRY;
        }
        match receive_packet(&socket, &mut receive_buffer)? {
            Some((
                address,
                Packet {
                    message:
                        Message::Welcome {
                            client_id,
                            nonce: echoed,
                        },
                    ..
                },
            )) if address == options.server && echoed == nonce => {
                break client_id;
            }
            _ => {}
        }
    };
    println!("CLIENT connected client_id={client_id}");

    let input_period = Duration::from_secs_f64(1.0 / f64::from(options.input_hz));
    let run_started = Instant::now();
    let mut next_input = run_started;
    let mut next_client_tick = 0u32;
    let mut outgoing_sequence = 1u32;
    let mut last_snapshot_sequence = 0u32;
    let mut snapshot_count = 0u32;
    let mut latest_snapshot_tick = 0u32;
    let mut latest_x = 0i32;
    let mut latest_y = 0i32;
    let mut has_snapshot = false;

    while run_started.elapsed() < options.duration {
        let now = Instant::now();
        if now >= next_input {
            outgoing_sequence = outgoing_sequence.wrapping_add(1);
            next_client_tick = next_client_tick.wrapping_add(1);
            send_packet(
                &socket,
                options.server,
                &Packet {
                    sequence: outgoing_sequence,
                    tick: next_client_tick,
                    message: Message::Input {
                        move_x: options.move_x,
                        move_y: options.move_y,
                        buttons: 0,
                    },
                },
            )?;
            next_input = now + input_period;
        }

        if let Some((address, packet)) = receive_packet(&socket, &mut receive_buffer)? {
            if address == options.server {
                if let Message::Snapshot {
                    client_id: snapshot_client_id,
                    x,
                    y,
                    ..
                } = packet.message
                {
                    if snapshot_client_id == client_id
                        && (!has_snapshot
                            || sequence_is_newer(packet.sequence, last_snapshot_sequence))
                    {
                        last_snapshot_sequence = packet.sequence;
                        snapshot_count = snapshot_count.saturating_add(1);
                        latest_snapshot_tick = packet.tick;
                        latest_x = x;
                        latest_y = y;
                        has_snapshot = true;
                    }
                }
            }
        }
        thread::yield_now();
    }

    let disconnect_started = Instant::now();
    let mut next_disconnect = Instant::now();
    let disconnect_sequence = outgoing_sequence.wrapping_add(1);
    let mut disconnected = false;
    while disconnect_started.elapsed() < Duration::from_secs(3) {
        let now = Instant::now();
        if now >= next_disconnect {
            send_packet(
                &socket,
                options.server,
                &Packet {
                    sequence: disconnect_sequence,
                    tick: next_client_tick,
                    message: Message::Disconnect { client_id },
                },
            )?;
            next_disconnect = now + CONTROL_RETRY;
        }
        if let Some((address, packet)) = receive_packet(&socket, &mut receive_buffer)? {
            if address == options.server {
                if let Message::Disconnected {
                    client_id: disconnected_id,
                } = packet.message
                {
                    if disconnected_id == client_id {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        thread::yield_now();
    }
    if !disconnected {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "disconnect acknowledgement timed out",
        )
        .into());
    }

    println!(
        "CLIENT snapshots={snapshot_count} final_tick={latest_snapshot_tick} final_x={latest_x} final_y={latest_y} disconnect=clean"
    );
    if snapshot_count == 0 {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "received no snapshots").into());
    }
    Ok(())
}

fn send_packet(socket: &UdpSocket, destination: SocketAddr, packet: &Packet) -> io::Result<()> {
    let bytes = encode(packet).map_err(protocol_io_error)?;
    socket.send_to(&bytes, destination)?;
    Ok(())
}

fn receive_packet(
    socket: &UdpSocket,
    buffer: &mut [u8; MAX_DATAGRAM + 1],
) -> io::Result<Option<(SocketAddr, Packet)>> {
    match socket.recv_from(buffer) {
        Ok((length, address)) => match decode(&buffer[..length]) {
            Ok(packet) => Ok(Some((address, packet))),
            Err(_) => Ok(None),
        },
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}
