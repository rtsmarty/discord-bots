use rand::{
    rngs::OsRng,
    RngCore,
};
use smallvec::SmallVec;
use std::{
    io,
    iter,
    marker::Unpin,
};
use tokio::{
    io::{
        AsyncRead,
        AsyncReadExt,
    },
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Encountered control opcode with too long payload")]
    InvalidLength,
    #[error("Dataframe is invalid")]
    InvalidDataFrame,
    #[error("Unsupported opcode")]
    ReservedOpcode,
    #[error("Text field is not utf8")]
    NonUtf8Text,
    #[error("Input stream ended prematurely")]
    PrematureFinish,
    #[error("An IO Error occured")]
    Io(#[from] io::Error),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Kind {
    Continuation,
    Text,
    Binary,
    Close,
    Ping,
    Pong
}
impl Kind {
    fn is_control(&self) -> bool {
        match *self {
            Kind::Continuation |
            Kind::Text         |
            Kind::Binary => false,

            Kind::Close |
            Kind::Ping  |
            Kind::Pong => true
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct MaskingKey {
    key: [u8; 4]
}
impl MaskingKey {
    pub fn new() -> io::Result<MaskingKey> {
        let mut key = [0u8; 4];
        OsRng.try_fill_bytes(&mut key)?;
        Ok(MaskingKey { key })
    }
    pub fn apply(&self, payload: &mut [u8]) {
        for (ct, item) in payload.iter_mut().enumerate() {
            *item ^= self.key[ct % 4];
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Header {
    pub is_final: bool,
    pub extensions: [bool; 3],
    pub kind: Kind,
    pub payload_len: u64,
    pub masking_key: Option<MaskingKey>
}
pub struct HeaderBytes {
    bytes: [u8; 14],
    len: u8
}
impl AsRef<[u8]> for HeaderBytes {
    fn as_ref(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }
}

impl Header {
    pub async fn read<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Header, Error> {
        let mut bytes = [0; 2];
        reader.read_exact(&mut bytes).await?;
        let first  = bytes[0];
        let second = bytes[1];

        let is_final = first & 0b1000_0000 == 0b1000_0000;
        let extensions = [
            first & 0b0100_0000 == 0b0100_0000,
            first & 0b0010_0000 == 0b0010_0000,
            first & 0b0001_0000 == 0b0001_0000
        ];
        let kind = match first & 0b0000_1111 {
            0       => Kind::Continuation,
            1       => Kind::Text,
            2       => Kind::Binary,
            3..=7   => return Err(Error::ReservedOpcode),
            8       => Kind::Close,
            9       => Kind::Ping,
            10      => Kind::Pong,
            11..=15 => return Err(Error::ReservedOpcode),
            _       => unreachable!()
        };

        let has_mask    =  second & 0b1000_0000 == 0b1000_0000;
        let payload_len = (second & 0b0111_1111) as u64;

        if kind.is_control() {
            if payload_len >= 126 {
                return Err(Error::InvalidLength);
            }
            if !is_final {
                return Err(Error::InvalidDataFrame);
            }
        }

        // A small vec big enough to hold the rest of the bytes for the
        // header
        let mut bytes = SmallVec::<[u8; 12]>::new();

        match payload_len {
            0..=125 => (),
            126     => bytes.extend(iter::repeat(0).take(2)),
            127     => bytes.extend(iter::repeat(0).take(8)),
            _       => unreachable!()
        }
        if has_mask {
            bytes.extend(iter::repeat(0).take(4));
        }
        let mut header = Header {
            is_final,
            extensions,
            kind,
            payload_len,
            masking_key: if bytes.is_empty() || !has_mask {
                None
            } else {
                Some(MaskingKey { key: [0; 4] })
            }
        };

        if bytes.is_empty() {
            Ok(header)
        } else {

            reader.read_exact(&mut bytes).await?;

            let start = match header.payload_len {
                0..=125 => 0,
                126 => 2,
                127 => 8,
                _ => unreachable!()
            };
            header.payload_len = match header.payload_len {
                0..=125 => header.payload_len,
                126     => ((bytes[0] as u64) << 8) |
                             bytes[1] as u64,
                127     => ((bytes[0] as u64) << 56) |
                           ((bytes[1] as u64) << 48) |
                           ((bytes[2] as u64) << 40) |
                           ((bytes[3] as u64) << 32) |
                           ((bytes[4] as u64) << 24) |
                           ((bytes[5] as u64) << 16) |
                           ((bytes[6] as u64) << 8)  |
                             bytes[7] as u64,
                _       => unreachable!()
            };
            if let Some(ref mut mask) = header.masking_key {
                mask.key[0] = bytes[start];
                mask.key[1] = bytes[start + 1];
                mask.key[2] = bytes[start + 2];
                mask.key[3] = bytes[start + 3];
            }
            Ok(header)
        }
    }
    pub fn bytes(self) -> HeaderBytes {
        let mut bytes = [0; 14];
        let mut len = 2;
        bytes[0] = {
            let mut item = 0;
            if self.is_final { item |= 0b1000_0000; }
            if self.extensions[0] { item |= 0b0100_0000; }
            if self.extensions[1] { item |= 0b0010_0000; }
            if self.extensions[2] { item |= 0b0001_0000; }
            match self.kind {
                Kind::Continuation => (),
                Kind::Text => item |= 1,
                Kind::Binary => item |= 2,
                Kind::Close => item |= 8,
                Kind::Ping => item |= 9,
                Kind::Pong => item |= 10
            }
            item
        };
        bytes[1] = {
            let mut item = 0;
            if self.masking_key.is_some() { item |= 0b1000_0000; }
            if self.payload_len > u16::max_value() as u64 { item |= 127 }
            else if self.payload_len > 125 { item |= 126 }
            else { item |= self.payload_len as u8 }
            item
        };
        len += if self.payload_len > u16::max_value() as u64 {
            bytes[2] = (self.payload_len >> 56 & 0xFF) as u8;
            bytes[3] = (self.payload_len >> 48 & 0xFF) as u8;
            bytes[4] = (self.payload_len >> 40 & 0xFF) as u8;
            bytes[5] = (self.payload_len >> 32 & 0xFF) as u8;

            bytes[6] = (self.payload_len >> 24 & 0xFF) as u8;
            bytes[7] = (self.payload_len >> 16 & 0xFF) as u8;
            bytes[8] = (self.payload_len >>  8 & 0xFF) as u8;
            bytes[9] = (self.payload_len       & 0xFF) as u8;
            8
        } else if self.payload_len > 125 {
            bytes[2] = (self.payload_len >>  8 & 0xFF) as u8;
            bytes[3] = (self.payload_len       & 0xFF) as u8;
            2
        } else {
            0
        };
        len += match self.masking_key {
            Some(key) => {
                bytes[len]     = key.key[0];
                bytes[len + 1] = key.key[1];
                bytes[len + 2] = key.key[2];
                bytes[len + 3] = key.key[3];
                4
            }
            None => 0
        };
        HeaderBytes {
            bytes,
            len: len as u8
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tokio::io::ReadBuf;

    struct SyncRead<T> {
        inner: T
    }
    impl<T: std::io::Read + std::marker::Unpin> tokio::io::AsyncRead for SyncRead<T> {
        fn poll_read(self: std::pin::Pin<&mut Self>, _: &mut std::task::Context, buf: &mut ReadBuf<'_>) -> std::task::Poll<std::io::Result<()>> {
            let read = self.get_mut().inner.read(buf.initialized_mut())?;
            buf.advance(read);
            std::task::Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn test() {
        let header = Header {
            is_final: true,
            extensions: [false; 3],
            kind: Kind::Text,
            payload_len: 10,
            masking_key: Some(MaskingKey::new().unwrap()),
        };
        let bytes = header.bytes();
        let mut read = SyncRead { inner: Cursor::new(bytes.as_ref().to_vec()) };
        let nheader = Header::read(&mut read).await.unwrap();
        assert_eq!(header, nheader)
    }

    #[tokio::test]
    async fn test2() {
        let input = b"\x81\xfe\0\xeb8\xda\x018C\xf8uWS\xbfo\x1a\x02\xf8LBy\xadOB[\xadO|q\xeaOBy\xebLB_\xeaO|i\xee/`l\xbeeoy\xf4KaN\xb8nMz\x9fmW\x01\x83Qnw\xaed]I\xed,i\x08\xe3mA\0\xf8-\x1aH\xa8nH]\xa8uQ]\xa9#\x02C\xf8%WK\xf8;\x1aT\xb3oM@\xf8-\x1a\x1c\xb8sWO\xa9dJ\x1a\xe0#LW\xb1hW\x1a\xf6#\x1c\\\xbfwQ[\xbf#\x02\x1a\xa9dJN\xbfs\x1aE\xf6#[W\xb7qJ]\xa9r\x1a\x02\xbc`TK\xbf-\x1aT\xbbs_]\x85uPJ\xbfrPW\xb6e\x1a\x02\xb4tTT\xf6#KP\xbbs\\\x1a\xe0oMT\xb6-\x1aH\xa8dK]\xb4b]\x1a\xe0oMT\xb6-\x1a_\xafhT\\\x85rMZ\xa9bJQ\xaauQW\xb4r\x1a\x02\xbc`TK\xbf|";
        let mut read = SyncRead { inner: Cursor::new(input.as_ref().to_vec()) };
        crate::ws::message::Owned::read(&mut read).await.unwrap();
    }
}

