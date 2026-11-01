use std::fmt;

pub const MAGIC: [u8; 4] = *b"QFNP";
pub const VERSION: u8 = 1;
pub const HEADER_LEN: usize = 16;
pub const MAX_DATAGRAM: usize = 1200;

const HELLO: u8 = 1;
const WELCOME: u8 = 2;
const INPUT: u8 = 3;
const SNAPSHOT: u8 = 4;
const DISCONNECT: u8 = 5;
const DISCONNECTED: u8 = 6;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Packet {
    pub sequence: u32,
    pub tick: u32,
    pub message: Message,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Message {
    Hello {
        nonce: u64,
    },
    Welcome {
        client_id: u32,
        nonce: u64,
    },
    Input {
        move_x: i16,
        move_y: i16,
        buttons: u16,
    },
    Snapshot {
        client_id: u32,
        acknowledged_input: u32,
        x: i32,
        y: i32,
    },
    Disconnect {
        client_id: u32,
    },
    Disconnected {
        client_id: u32,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProtocolError {
    DatagramTooShort,
    DatagramTooLarge,
    BadMagic,
    UnsupportedVersion(u8),
    UnknownMessage(u8),
    InvalidPayloadLength { message: u8, length: usize },
    InvalidField(&'static str),
    PayloadLengthOverflow,
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DatagramTooShort => write!(formatter, "datagram is shorter than the header"),
            Self::DatagramTooLarge => write!(formatter, "datagram exceeds {MAX_DATAGRAM} bytes"),
            Self::BadMagic => write!(formatter, "invalid protocol magic"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported protocol version {version}")
            }
            Self::UnknownMessage(message) => write!(formatter, "unknown message type {message}"),
            Self::InvalidPayloadLength { message, length } => {
                write!(
                    formatter,
                    "invalid payload length {length} for message {message}"
                )
            }
            Self::InvalidField(field) => write!(formatter, "invalid field: {field}"),
            Self::PayloadLengthOverflow => write!(formatter, "payload length does not fit header"),
        }
    }
}

impl std::error::Error for ProtocolError {}

pub fn encode(packet: &Packet) -> Result<Vec<u8>, ProtocolError> {
    let (kind, payload) = encode_payload(&packet.message)?;
    if payload.len() > u16::MAX as usize {
        return Err(ProtocolError::PayloadLengthOverflow);
    }
    let total_len = HEADER_LEN + payload.len();
    if total_len > MAX_DATAGRAM {
        return Err(ProtocolError::DatagramTooLarge);
    }

    let mut bytes = Vec::with_capacity(total_len);
    bytes.extend_from_slice(&MAGIC);
    bytes.push(VERSION);
    bytes.push(kind);
    bytes.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    bytes.extend_from_slice(&packet.sequence.to_be_bytes());
    bytes.extend_from_slice(&packet.tick.to_be_bytes());
    bytes.extend_from_slice(&payload);
    Ok(bytes)
}

pub fn decode(bytes: &[u8]) -> Result<Packet, ProtocolError> {
    if bytes.len() < HEADER_LEN {
        return Err(ProtocolError::DatagramTooShort);
    }
    if bytes.len() > MAX_DATAGRAM {
        return Err(ProtocolError::DatagramTooLarge);
    }
    if bytes[0..4] != MAGIC {
        return Err(ProtocolError::BadMagic);
    }
    if bytes[4] != VERSION {
        return Err(ProtocolError::UnsupportedVersion(bytes[4]));
    }

    let kind = bytes[5];
    let payload_len = usize::from(u16::from_be_bytes([bytes[6], bytes[7]]));
    if bytes.len() != HEADER_LEN + payload_len {
        return Err(ProtocolError::InvalidPayloadLength {
            message: kind,
            length: payload_len,
        });
    }

    let sequence = read_u32(&bytes[8..12]);
    let tick = read_u32(&bytes[12..16]);
    let payload = &bytes[HEADER_LEN..];
    let message = decode_payload(kind, payload)?;
    Ok(Packet {
        sequence,
        tick,
        message,
    })
}

pub fn sequence_is_newer(candidate: u32, previous: u32) -> bool {
    let difference = candidate.wrapping_sub(previous);
    difference != 0 && difference < (1 << 31)
}

fn encode_payload(message: &Message) -> Result<(u8, Vec<u8>), ProtocolError> {
    let mut payload = Vec::new();
    let kind = match message {
        Message::Hello { nonce } => {
            if *nonce == 0 {
                return Err(ProtocolError::InvalidField("nonce"));
            }
            payload.extend_from_slice(&nonce.to_be_bytes());
            HELLO
        }
        Message::Welcome { client_id, nonce } => {
            validate_client_id(*client_id)?;
            if *nonce == 0 {
                return Err(ProtocolError::InvalidField("nonce"));
            }
            payload.extend_from_slice(&client_id.to_be_bytes());
            payload.extend_from_slice(&nonce.to_be_bytes());
            WELCOME
        }
        Message::Input {
            move_x,
            move_y,
            buttons,
        } => {
            validate_input(*move_x, *move_y, *buttons)?;
            payload.extend_from_slice(&move_x.to_be_bytes());
            payload.extend_from_slice(&move_y.to_be_bytes());
            payload.extend_from_slice(&buttons.to_be_bytes());
            INPUT
        }
        Message::Snapshot {
            client_id,
            acknowledged_input,
            x,
            y,
        } => {
            validate_client_id(*client_id)?;
            payload.extend_from_slice(&client_id.to_be_bytes());
            payload.extend_from_slice(&acknowledged_input.to_be_bytes());
            payload.extend_from_slice(&x.to_be_bytes());
            payload.extend_from_slice(&y.to_be_bytes());
            SNAPSHOT
        }
        Message::Disconnect { client_id } => {
            validate_client_id(*client_id)?;
            payload.extend_from_slice(&client_id.to_be_bytes());
            DISCONNECT
        }
        Message::Disconnected { client_id } => {
            validate_client_id(*client_id)?;
            payload.extend_from_slice(&client_id.to_be_bytes());
            DISCONNECTED
        }
    };
    Ok((kind, payload))
}

fn decode_payload(kind: u8, payload: &[u8]) -> Result<Message, ProtocolError> {
    let expected_len = match kind {
        HELLO => 8,
        WELCOME => 12,
        INPUT => 6,
        SNAPSHOT => 16,
        DISCONNECT | DISCONNECTED => 4,
        other => return Err(ProtocolError::UnknownMessage(other)),
    };
    if payload.len() != expected_len {
        return Err(ProtocolError::InvalidPayloadLength {
            message: kind,
            length: payload.len(),
        });
    }

    match kind {
        HELLO => {
            let nonce = read_u64(payload);
            if nonce == 0 {
                return Err(ProtocolError::InvalidField("nonce"));
            }
            Ok(Message::Hello { nonce })
        }
        WELCOME => {
            let client_id = read_u32(&payload[0..4]);
            let nonce = read_u64(&payload[4..12]);
            validate_client_id(client_id)?;
            if nonce == 0 {
                return Err(ProtocolError::InvalidField("nonce"));
            }
            Ok(Message::Welcome { client_id, nonce })
        }
        INPUT => {
            let move_x = i16::from_be_bytes([payload[0], payload[1]]);
            let move_y = i16::from_be_bytes([payload[2], payload[3]]);
            let buttons = u16::from_be_bytes([payload[4], payload[5]]);
            validate_input(move_x, move_y, buttons)?;
            Ok(Message::Input {
                move_x,
                move_y,
                buttons,
            })
        }
        SNAPSHOT => {
            let client_id = read_u32(&payload[0..4]);
            validate_client_id(client_id)?;
            Ok(Message::Snapshot {
                client_id,
                acknowledged_input: read_u32(&payload[4..8]),
                x: read_i32(&payload[8..12]),
                y: read_i32(&payload[12..16]),
            })
        }
        DISCONNECT => {
            let client_id = read_u32(payload);
            validate_client_id(client_id)?;
            Ok(Message::Disconnect { client_id })
        }
        DISCONNECTED => {
            let client_id = read_u32(payload);
            validate_client_id(client_id)?;
            Ok(Message::Disconnected { client_id })
        }
        _ => unreachable!("message kind checked above"),
    }
}

fn validate_input(move_x: i16, move_y: i16, buttons: u16) -> Result<(), ProtocolError> {
    if !(-1000..=1000).contains(&move_x) {
        return Err(ProtocolError::InvalidField("move_x"));
    }
    if !(-1000..=1000).contains(&move_y) {
        return Err(ProtocolError::InvalidField("move_y"));
    }
    if buttons != 0 {
        return Err(ProtocolError::InvalidField("reserved buttons"));
    }
    Ok(())
}

fn validate_client_id(client_id: u32) -> Result<(), ProtocolError> {
    if client_id == 0 {
        return Err(ProtocolError::InvalidField("client_id"));
    }
    Ok(())
}

fn read_u32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn read_u64(bytes: &[u8]) -> u64 {
    u64::from_be_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ])
}

fn read_i32(bytes: &[u8]) -> i32 {
    i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(message: Message) -> Packet {
        Packet {
            sequence: 0x0102_0304,
            tick: 0x0506_0708,
            message,
        }
    }

    #[test]
    fn round_trips_every_message_type() {
        let messages = [
            Message::Hello {
                nonce: 0x0102_0304_0506_0708,
            },
            Message::Welcome {
                client_id: 7,
                nonce: 0x0102_0304_0506_0708,
            },
            Message::Input {
                move_x: -1000,
                move_y: 1000,
                buttons: 0,
            },
            Message::Snapshot {
                client_id: 7,
                acknowledged_input: u32::MAX,
                x: -123456,
                y: 789012,
            },
            Message::Disconnect { client_id: 7 },
            Message::Disconnected { client_id: 7 },
        ];

        for message in messages {
            let expected = packet(message);
            assert_eq!(decode(&encode(&expected).unwrap()).unwrap(), expected);
        }
    }

    #[test]
    fn encodes_header_in_network_byte_order() {
        let bytes = encode(&packet(Message::Hello { nonce: 1 })).unwrap();
        assert_eq!(&bytes[..8], b"QFNP\x01\x01\x00\x08");
        assert_eq!(&bytes[8..16], &[1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn rejects_truncated_and_oversized_datagrams() {
        assert_eq!(
            decode(&[0; HEADER_LEN - 1]),
            Err(ProtocolError::DatagramTooShort)
        );
        assert_eq!(
            decode(&vec![0; MAX_DATAGRAM + 1]),
            Err(ProtocolError::DatagramTooLarge)
        );
    }

    #[test]
    fn rejects_bad_magic_and_versions() {
        let mut bytes = encode(&packet(Message::Hello { nonce: 1 })).unwrap();
        bytes[0] = b'X';
        assert_eq!(decode(&bytes), Err(ProtocolError::BadMagic));
        bytes[0..4].copy_from_slice(&MAGIC);
        bytes[4] = VERSION + 1;
        assert_eq!(
            decode(&bytes),
            Err(ProtocolError::UnsupportedVersion(VERSION + 1))
        );
    }

    #[test]
    fn rejects_unknown_types_and_mismatched_payload_lengths() {
        let mut bytes = encode(&packet(Message::Hello { nonce: 1 })).unwrap();
        bytes[5] = 99;
        assert_eq!(decode(&bytes), Err(ProtocolError::UnknownMessage(99)));

        let mut bytes = encode(&packet(Message::Hello { nonce: 1 })).unwrap();
        bytes[7] = 7;
        assert!(matches!(
            decode(&bytes),
            Err(ProtocolError::InvalidPayloadLength { .. })
        ));
    }

    #[test]
    fn rejects_invalid_decoded_fields() {
        let mut bytes = encode(&packet(Message::Input {
            move_x: 0,
            move_y: 0,
            buttons: 0,
        }))
        .unwrap();
        bytes[HEADER_LEN..HEADER_LEN + 2].copy_from_slice(&1001i16.to_be_bytes());
        assert_eq!(decode(&bytes), Err(ProtocolError::InvalidField("move_x")));

        let mut bytes = encode(&packet(Message::Hello { nonce: 1 })).unwrap();
        bytes[HEADER_LEN..].fill(0);
        assert_eq!(decode(&bytes), Err(ProtocolError::InvalidField("nonce")));
    }

    #[test]
    fn rejects_reserved_buttons_and_zero_client_ids() {
        let mut bytes = encode(&packet(Message::Input {
            move_x: 0,
            move_y: 0,
            buttons: 0,
        }))
        .unwrap();
        bytes[HEADER_LEN + 5] = 1;
        assert_eq!(
            decode(&bytes),
            Err(ProtocolError::InvalidField("reserved buttons"))
        );

        let mut bytes = encode(&packet(Message::Welcome {
            client_id: 1,
            nonce: 1,
        }))
        .unwrap();
        bytes[HEADER_LEN..HEADER_LEN + 4].fill(0);
        assert_eq!(
            decode(&bytes),
            Err(ProtocolError::InvalidField("client_id"))
        );
    }

    #[test]
    fn rejects_invalid_fields_on_encode_too() {
        assert_eq!(
            encode(&packet(Message::Input {
                move_x: 0,
                move_y: -1001,
                buttons: 0,
            })),
            Err(ProtocolError::InvalidField("move_y"))
        );
        assert_eq!(
            encode(&packet(Message::Disconnect { client_id: 0 })),
            Err(ProtocolError::InvalidField("client_id"))
        );
    }

    #[test]
    fn compares_wrapping_sequences_in_serial_number_order() {
        assert!(sequence_is_newer(1, u32::MAX));
        assert!(!sequence_is_newer(3, 3));
        assert!(!sequence_is_newer(u32::MAX, 1));
    }
}
