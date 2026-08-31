use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use rand::{rngs::OsRng, RngCore};
use sha1::{Digest, Sha1};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::{ChildStdin, ChildStdout};

const MAX_HANDSHAKE_BYTES: usize = 64 * 1024;
const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;
const WEBSOCKET_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

pub(crate) enum IncomingMessage {
    Text(String),
    Closed,
}

pub(crate) fn client_handshake(
    stdin: &mut ChildStdin,
    stdout: ChildStdout,
) -> io::Result<BufReader<ChildStdout>> {
    let mut nonce = [0_u8; 16];
    OsRng.fill_bytes(&mut nonce);
    let key = BASE64.encode(nonce);
    let request = format!(
        "GET /rpc HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    stdin.write_all(request.as_bytes())?;
    stdin.flush()?;

    let mut reader = BufReader::new(stdout);
    validate_handshake_response(&mut reader, &key)?;
    Ok(reader)
}

fn validate_handshake_response<R: BufRead>(reader: &mut R, key: &str) -> io::Result<()> {
    let mut total = 0_usize;
    let status = read_header_line(reader, &mut total)?;
    let status = status.trim_end_matches(['\r', '\n']);
    if !(status.starts_with("HTTP/1.1 101 ") || status.starts_with("HTTP/1.0 101 ")) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Codex proxy WebSocket 握手返回无效状态：{status}"),
        ));
    }

    let mut upgrade = false;
    let mut connection_upgrade = false;
    let mut accept = None;
    loop {
        let line = read_header_line(reader, &mut total)?;
        if line == "\r\n" || line == "\n" {
            break;
        }
        let (name, value) = line.split_once(':').ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Codex proxy 返回了无效 HTTP 响应头",
            )
        })?;
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim();
        match name.as_str() {
            "upgrade" => upgrade = value.eq_ignore_ascii_case("websocket"),
            "connection" => {
                connection_upgrade = value
                    .split(',')
                    .any(|token| token.trim().eq_ignore_ascii_case("upgrade"));
            }
            "sec-websocket-accept" => accept = Some(value.to_string()),
            _ => {}
        }
    }

    let expected = websocket_accept(key);
    if !upgrade || !connection_upgrade || accept.as_deref() != Some(expected.as_str()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Codex proxy WebSocket 握手校验失败",
        ));
    }
    Ok(())
}

fn read_header_line<R: BufRead>(reader: &mut R, total: &mut usize) -> io::Result<String> {
    let mut line = String::new();
    let read = reader.read_line(&mut line)?;
    if read == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "Codex proxy 在 WebSocket 握手完成前退出",
        ));
    }
    *total = total.saturating_add(read);
    if *total > MAX_HANDSHAKE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Codex proxy WebSocket 握手响应过大",
        ));
    }
    Ok(line)
}

fn websocket_accept(key: &str) -> String {
    let mut digest = Sha1::new();
    digest.update(key.as_bytes());
    digest.update(WEBSOCKET_GUID.as_bytes());
    BASE64.encode(digest.finalize())
}

pub(crate) fn write_text(writer: &mut ChildStdin, text: &str) -> io::Result<()> {
    write_frame(writer, 0x1, text.as_bytes())
}

pub(crate) fn write_control(writer: &mut ChildStdin, opcode: u8, payload: &[u8]) -> io::Result<()> {
    if payload.len() > 125 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "WebSocket 控制帧负载超过 125 字节",
        ));
    }
    write_frame(writer, opcode, payload)
}

fn write_frame<W: Write>(writer: &mut W, opcode: u8, payload: &[u8]) -> io::Result<()> {
    let mut header = Vec::with_capacity(14);
    header.push(0x80 | (opcode & 0x0f));
    match payload.len() {
        length @ 0..=125 => header.push(0x80 | length as u8),
        length @ 126..=65_535 => {
            header.push(0x80 | 126);
            header.extend_from_slice(&(length as u16).to_be_bytes());
        }
        length => {
            header.push(0x80 | 127);
            header.extend_from_slice(&(length as u64).to_be_bytes());
        }
    }
    let mut mask = [0_u8; 4];
    OsRng.fill_bytes(&mut mask);
    header.extend_from_slice(&mask);
    writer.write_all(&header)?;
    for (index, chunk) in payload.chunks(8 * 1024).enumerate() {
        let offset = index * 8 * 1024;
        let mut masked = chunk.to_vec();
        for (chunk_index, byte) in masked.iter_mut().enumerate() {
            *byte ^= mask[(offset + chunk_index) % mask.len()];
        }
        writer.write_all(&masked)?;
    }
    writer.flush()
}

pub(crate) fn read_message<R: Read, F>(
    reader: &mut R,
    mut write_control_frame: F,
) -> io::Result<IncomingMessage>
where
    F: FnMut(u8, &[u8]) -> io::Result<()>,
{
    let mut fragmented_opcode = None;
    let mut message = Vec::new();
    loop {
        let frame = read_frame(reader)?;
        match frame.opcode {
            0x0 => {
                if fragmented_opcode.is_none() {
                    return Err(protocol_error("收到没有起始帧的 WebSocket continuation"));
                }
                append_payload(&mut message, &frame.payload)?;
                if frame.fin {
                    return decode_message(fragmented_opcode.take().unwrap(), message);
                }
            }
            opcode @ (0x1 | 0x2) => {
                if fragmented_opcode.is_some() {
                    return Err(protocol_error("分片 WebSocket 消息尚未完成就收到新消息"));
                }
                if frame.fin {
                    return decode_message(opcode, frame.payload);
                }
                fragmented_opcode = Some(opcode);
                append_payload(&mut message, &frame.payload)?;
            }
            0x8 => {
                let _ = write_control_frame(0x8, &frame.payload);
                return Ok(IncomingMessage::Closed);
            }
            0x9 => write_control_frame(0xA, &frame.payload)?,
            0xA => {}
            _ => return Err(protocol_error("收到不支持的 WebSocket opcode")),
        }
    }
}

struct Frame {
    fin: bool,
    opcode: u8,
    payload: Vec<u8>,
}

fn read_frame<R: Read>(reader: &mut R) -> io::Result<Frame> {
    let mut first = [0_u8; 2];
    reader.read_exact(&mut first)?;
    let fin = first[0] & 0x80 != 0;
    let reserved = first[0] & 0x70;
    let opcode = first[0] & 0x0f;
    let masked = first[1] & 0x80 != 0;
    if reserved != 0 {
        return Err(protocol_error("Codex proxy 使用了未协商的 WebSocket 扩展"));
    }
    if masked {
        return Err(protocol_error("Codex proxy 返回了不允许的 masked 服务端帧"));
    }
    let short_length = (first[1] & 0x7f) as u64;
    let length = match short_length {
        126 => read_u16(reader)? as u64,
        127 => {
            let length = read_u64(reader)?;
            if length & (1_u64 << 63) != 0 {
                return Err(protocol_error("WebSocket 帧长度最高位必须为 0"));
            }
            length
        }
        length => length,
    };
    let control = opcode & 0x08 != 0;
    if control && (!fin || length > 125) {
        return Err(protocol_error("WebSocket 控制帧必须完整且不超过 125 字节"));
    }
    let length =
        usize::try_from(length).map_err(|_| protocol_error("WebSocket 帧长度无法在本机表示"))?;
    if length > MAX_MESSAGE_BYTES {
        return Err(protocol_error("Codex proxy WebSocket 消息超过 64 MiB 上限"));
    }
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload)?;
    Ok(Frame {
        fin,
        opcode,
        payload,
    })
}

fn append_payload(message: &mut Vec<u8>, payload: &[u8]) -> io::Result<()> {
    if message.len().saturating_add(payload.len()) > MAX_MESSAGE_BYTES {
        return Err(protocol_error("Codex proxy WebSocket 消息超过 64 MiB 上限"));
    }
    message.extend_from_slice(payload);
    Ok(())
}

fn decode_message(opcode: u8, payload: Vec<u8>) -> io::Result<IncomingMessage> {
    if opcode != 0x1 {
        return Err(protocol_error("Codex proxy 返回了非文本 RPC 消息"));
    }
    String::from_utf8(payload)
        .map(IncomingMessage::Text)
        .map_err(|_| protocol_error("Codex proxy 返回的文本消息不是 UTF-8"))
}

fn read_u16<R: Read>(reader: &mut R) -> io::Result<u16> {
    let mut bytes = [0_u8; 2];
    reader.read_exact(&mut bytes)?;
    Ok(u16::from_be_bytes(bytes))
}

fn read_u64<R: Read>(reader: &mut R) -> io::Result<u64> {
    let mut bytes = [0_u8; 8];
    reader.read_exact(&mut bytes)?;
    Ok(u64::from_be_bytes(bytes))
}

fn protocol_error(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn validates_rfc6455_handshake_fixture() {
        let key = "dGhlIHNhbXBsZSBub25jZQ==";
        let response = concat!(
            "HTTP/1.1 101 Switching Protocols\r\n",
            "Upgrade: websocket\r\n",
            "Connection: Upgrade\r\n",
            "Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n",
            "\r\n"
        );
        validate_handshake_response(&mut Cursor::new(response), key).unwrap();
    }

    #[test]
    fn rejects_handshake_with_wrong_accept_value() {
        let response = concat!(
            "HTTP/1.1 101 Switching Protocols\r\n",
            "Upgrade: websocket\r\n",
            "Connection: Upgrade\r\n",
            "Sec-WebSocket-Accept: wrong\r\n",
            "\r\n"
        );
        assert!(validate_handshake_response(&mut Cursor::new(response), "key").is_err());
    }

    #[test]
    fn reads_fragmented_text_and_answers_ping() {
        let bytes = [
            0x01, 0x03, b'{', b'"', b'a', 0x89, 0x01, b'?', 0x80, 0x03, b'"', b'}', b'\n',
        ];
        let mut controls = Vec::new();
        let message = read_message(&mut Cursor::new(bytes), |opcode, payload| {
            controls.push((opcode, payload.to_vec()));
            Ok(())
        })
        .unwrap();
        assert!(matches!(message, IncomingMessage::Text(value) if value == "{\"a\"}\n"));
        assert_eq!(controls, vec![(0xA, vec![b'?'])]);
    }

    #[test]
    fn client_frames_are_masked_and_round_trip_payload() {
        let mut frame = Vec::new();
        write_frame(&mut frame, 0x1, b"hello").unwrap();
        assert_eq!(frame[0], 0x81);
        assert_eq!(frame[1] & 0x80, 0x80);
        let mask = &frame[2..6];
        let decoded = frame[6..]
            .iter()
            .enumerate()
            .map(|(index, byte)| byte ^ mask[index % 4])
            .collect::<Vec<_>>();
        assert_eq!(decoded, b"hello");
    }
}
