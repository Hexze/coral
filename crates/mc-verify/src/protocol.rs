use std::io::Cursor;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const VARINT_SEGMENT: u8 = 0x7F;
const VARINT_CONTINUE: u8 = 0x80;
const MAX_PACKET_SIZE: usize = 2 * 1024 * 1024;

pub async fn read_varint<R: AsyncRead + Unpin>(reader: &mut R) -> std::io::Result<i32> {
    let mut value: i32 = 0;
    let mut position: u32 = 0;

    loop {
        let byte = reader.read_u8().await?;
        value |= ((byte & VARINT_SEGMENT) as i32) << position;
        if byte & VARINT_CONTINUE == 0 {
            return Ok(value);
        }
        position += 7;
        if position >= 32 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "VarInt too large",
            ));
        }
    }
}

pub fn write_varint(buf: &mut Vec<u8>, value: i32) {
    let mut value = value as u32;
    loop {
        let mut byte = (value & VARINT_SEGMENT as u32) as u8;
        value >>= 7;
        if value != 0 {
            byte |= VARINT_CONTINUE;
        }
        buf.push(byte);
        if value == 0 {
            break;
        }
    }
}

pub fn varint_len(value: i32) -> usize {
    let mut value = value as u32;
    let mut len = 0;
    loop {
        len += 1;
        value >>= 7;
        if value == 0 {
            break;
        }
    }
    len
}

pub fn read_string(cursor: &mut Cursor<&[u8]>) -> std::io::Result<String> {
    let len = read_varint_sync(cursor)? as usize;
    let mut buf = vec![0u8; len];
    std::io::Read::read_exact(cursor, &mut buf)?;
    String::from_utf8(buf).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

pub fn read_varint_sync(cursor: &mut Cursor<&[u8]>) -> std::io::Result<i32> {
    let mut value: i32 = 0;
    let mut position: u32 = 0;

    loop {
        let mut byte = [0u8; 1];
        std::io::Read::read_exact(cursor, &mut byte)?;
        value |= ((byte[0] & VARINT_SEGMENT) as i32) << position;
        if byte[0] & VARINT_CONTINUE == 0 {
            return Ok(value);
        }
        position += 7;
        if position >= 32 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "VarInt too large",
            ));
        }
    }
}

pub fn write_string(buf: &mut Vec<u8>, s: &str) {
    write_varint(buf, s.len() as i32);
    buf.extend_from_slice(s.as_bytes());
}

pub fn read_uuid(cursor: &mut Cursor<&[u8]>) -> std::io::Result<u128> {
    let mut bytes = [0u8; 16];
    std::io::Read::read_exact(cursor, &mut bytes)?;
    Ok(u128::from_be_bytes(bytes))
}

pub async fn read_packet<R: AsyncRead + Unpin>(reader: &mut R) -> std::io::Result<(i32, Vec<u8>)> {
    let length = read_varint(reader).await? as usize;
    if length > MAX_PACKET_SIZE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "packet too large",
        ));
    }

    let mut data = vec![0u8; length];
    reader.read_exact(&mut data).await?;

    let mut cursor = Cursor::new(data.as_slice());
    let packet_id = read_varint_sync(&mut cursor)?;
    let pos = cursor.position() as usize;

    Ok((packet_id, data[pos..].to_vec()))
}

pub async fn write_packet<W: AsyncWrite + Unpin>(
    writer: &mut W,
    packet_id: i32,
    payload: &[u8],
) -> std::io::Result<()> {
    let id_len = varint_len(packet_id);
    let total_len = id_len + payload.len();

    let mut buf = Vec::with_capacity(varint_len(total_len as i32) + total_len);
    write_varint(&mut buf, total_len as i32);
    write_varint(&mut buf, packet_id);
    buf.extend_from_slice(payload);

    writer.write_all(&buf).await
}
