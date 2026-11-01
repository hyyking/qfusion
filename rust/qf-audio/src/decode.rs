//! Sound file decoding (wav and ogg vorbis) into interleaved `f32` PCM.

use std::fmt;
use std::io::Cursor;

/// Decoded audio: interleaved samples, one or two channels.
#[derive(Clone, Debug, PartialEq)]
pub struct Pcm {
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: Vec<f32>,
}

impl Pcm {
    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.channels.max(1))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    UnknownFormat,
    Truncated(&'static str),
    Unsupported(String),
    Corrupt(String),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownFormat => write!(f, "not a RIFF/WAVE or Ogg file"),
            Self::Truncated(what) => write!(f, "truncated {what}"),
            Self::Unsupported(what) => write!(f, "unsupported {what}"),
            Self::Corrupt(what) => write!(f, "corrupt data: {what}"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// Detects the container from its magic bytes and decodes it.
pub fn decode(data: &[u8]) -> Result<Pcm, DecodeError> {
    if data.starts_with(b"RIFF") {
        decode_wav(data)
    } else if data.starts_with(b"OggS") {
        decode_ogg(data)
    } else {
        Err(DecodeError::UnknownFormat)
    }
}

fn u16_at(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}

fn u32_at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

const FORMAT_PCM: u16 = 1;
const FORMAT_FLOAT: u16 = 3;
const FORMAT_EXTENSIBLE: u16 = 0xFFFE;

pub fn decode_wav(data: &[u8]) -> Result<Pcm, DecodeError> {
    if data.len() < 12 || &data[8..12] != b"WAVE" {
        return Err(DecodeError::UnknownFormat);
    }
    let mut format: Option<(u16, u16, u32, u16)> = None;
    let mut pcm: Option<&[u8]> = None;
    let mut offset = 12;
    while offset + 8 <= data.len() {
        let id = &data[offset..offset + 4];
        let size = u32_at(data, offset + 4) as usize;
        let body = offset + 8;
        // Tolerate a data chunk whose declared size overruns the file.
        let end = body.saturating_add(size).min(data.len());
        match id {
            b"fmt " => {
                if end - body < 16 {
                    return Err(DecodeError::Truncated("fmt chunk"));
                }
                let mut tag = u16_at(data, body);
                if tag == FORMAT_EXTENSIBLE {
                    if end - body < 26 {
                        return Err(DecodeError::Truncated("extensible fmt chunk"));
                    }
                    tag = u16_at(data, body + 24);
                }
                format = Some((
                    tag,
                    u16_at(data, body + 2),
                    u32_at(data, body + 4),
                    u16_at(data, body + 14),
                ));
            }
            b"data" => {
                pcm = Some(&data[body..end]);
                break;
            }
            _ => {}
        }
        offset = body.saturating_add(size).saturating_add(size & 1);
    }
    let (tag, channels, sample_rate, bits) = format.ok_or(DecodeError::Truncated("fmt chunk"))?;
    let bytes = pcm.ok_or(DecodeError::Truncated("data chunk"))?;
    if !(1..=2).contains(&channels) {
        return Err(DecodeError::Unsupported(format!("{channels} channels")));
    }
    if sample_rate == 0 {
        return Err(DecodeError::Corrupt("zero sample rate".into()));
    }
    let width = usize::from(bits / 8);
    let samples: Vec<f32> = match (tag, bits) {
        (FORMAT_PCM, 8) => bytes
            .iter()
            .map(|&b| (f32::from(b) - 128.0) / 128.0)
            .collect(),
        (FORMAT_PCM, 16) => bytes
            .chunks_exact(width)
            .map(|c| f32::from(i16::from_le_bytes([c[0], c[1]])) / 32768.0)
            .collect(),
        (FORMAT_PCM, 24) => bytes
            .chunks_exact(width)
            .map(|c| (i32::from_le_bytes([0, c[0], c[1], c[2]]) >> 8) as f32 / 8_388_608.0)
            .collect(),
        (FORMAT_PCM, 32) => bytes
            .chunks_exact(width)
            .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f32 / 2_147_483_648.0)
            .collect(),
        (FORMAT_FLOAT, 32) => bytes
            .chunks_exact(width)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect(),
        _ => {
            return Err(DecodeError::Unsupported(format!(
                "wav format tag {tag} with {bits} bits"
            )));
        }
    };
    let whole = samples.len() / usize::from(channels) * usize::from(channels);
    let mut samples = samples;
    samples.truncate(whole);
    Ok(Pcm {
        sample_rate,
        channels,
        samples,
    })
}

pub fn decode_ogg(data: &[u8]) -> Result<Pcm, DecodeError> {
    let mut reader = lewton::inside_ogg::OggStreamReader::new(Cursor::new(data))
        .map_err(|e| DecodeError::Corrupt(e.to_string()))?;
    let channels = u16::from(reader.ident_hdr.audio_channels);
    if !(1..=2).contains(&channels) {
        return Err(DecodeError::Unsupported(format!("{channels} channels")));
    }
    let sample_rate = reader.ident_hdr.audio_sample_rate;
    let mut samples = Vec::new();
    loop {
        match reader.read_dec_packet_itl() {
            Ok(Some(packet)) => samples.extend(packet.iter().map(|&s| f32::from(s) / 32768.0)),
            Ok(None) => break,
            Err(e) => return Err(DecodeError::Corrupt(e.to_string())),
        }
    }
    Ok(Pcm {
        sample_rate,
        channels,
        samples,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Builds a canonical 16-bit PCM wav file.
    pub(crate) fn make_wav(rate: u32, channels: u16, samples: &[i16]) -> Vec<u8> {
        let data_len = (samples.len() * 2) as u32;
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
        out.extend_from_slice(&(channels * 2).to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        for s in samples {
            out.extend_from_slice(&s.to_le_bytes());
        }
        out
    }

    #[test]
    fn decodes_16_bit_mono() {
        let wav = make_wav(22_050, 1, &[0, 16384, -32768, 32767]);
        let pcm = decode(&wav).unwrap();
        assert_eq!(
            (pcm.sample_rate, pcm.channels, pcm.frames()),
            (22_050, 1, 4)
        );
        assert_eq!(pcm.samples[1], 0.5);
        assert_eq!(pcm.samples[2], -1.0);
    }

    #[test]
    fn skips_unknown_chunks_and_odd_padding() {
        let mut wav = make_wav(44_100, 2, &[100, -100, 200, -200]);
        let extra = [b"LIST".as_slice(), &3u32.to_le_bytes(), b"abc\0"].concat();
        wav.splice(36..36, extra);
        let pcm = decode(&wav).unwrap();
        assert_eq!((pcm.channels, pcm.frames()), (2, 2));
    }

    #[test]
    fn eight_bit_is_centered() {
        let mut wav = make_wav(8000, 1, &[0, 0]);
        wav[34] = 8;
        wav[40..44].copy_from_slice(&2u32.to_le_bytes());
        wav.truncate(46);
        wav[44] = 128;
        wav[45] = 255;
        let pcm = decode(&wav).unwrap();
        assert_eq!(pcm.samples[0], 0.0);
        assert!(pcm.samples[1] > 0.99);
    }

    #[test]
    fn rejects_garbage_and_truncated_input() {
        assert_eq!(decode(b"hello world!"), Err(DecodeError::UnknownFormat));
        assert!(decode(b"RIFF\0\0\0\0WAVE").is_err());
        let mut wav = make_wav(8000, 1, &[1, 2]);
        wav[20] = 2; // ADPCM
        assert!(matches!(decode(&wav), Err(DecodeError::Unsupported(_))));
        assert!(decode(b"OggSnot really vorbis").is_err());
    }

    #[test]
    fn rejects_more_than_two_channels() {
        let wav = make_wav(8000, 6, &[0; 12]);
        assert!(matches!(decode(&wav), Err(DecodeError::Unsupported(_))));
    }
}
