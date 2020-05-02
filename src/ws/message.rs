use bytes::{
    BufMut,
    Bytes,
    BytesMut,
};
use smallvec::SmallVec;
use std::{
    io,
    marker::Unpin,
    str
};
use tokio::{
    io::{
        AsyncRead,
        AsyncReadExt,
        AsyncWrite,
        AsyncWriteExt,
    },
};

use super::header::{
    self,
    Header,
    Kind as HeaderKind,
    MaskingKey
};

#[derive(Debug, thiserror::Error)]
#[error("Failed to parse header: {kind}")]
pub struct Error {
    #[from]
    kind: header::Error
}

#[derive(Debug)]
pub struct Owned {
    kind: HeaderKind,
    data: Bytes
}
impl Owned {
    fn new(kind: HeaderKind, data: Bytes) -> Result<Self, Error> {
        match kind {
            HeaderKind::Text => match str::from_utf8(&data) {
                Ok(_) => (),
                Err(_) => return Err(header::Error::NonUtf8Text.into())
            },
            HeaderKind::Close => {
                if data.len() > 2 {
                    match str::from_utf8(&data[2..]) {
                        Ok(_) => (),
                        Err(_) => return Err(header::Error::NonUtf8Text.into())
                    }
                }
            }
            HeaderKind::Continuation => return Err(header::Error::InvalidDataFrame.into()),
            _ => ()
        }

        Ok(Self { kind, data, })
    }
    pub async fn read<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Self, Error> {
        let mut header = Header::read(reader).await?;
        let message_kind = header.kind;

        let mut payload = BytesMut::with_capacity(0);
        loop {
            payload.reserve(header.payload_len as usize);

            let start = payload.len();
            unsafe {
                let buf = &mut BufMut::bytes_mut(&mut payload)[..header.payload_len as usize];
                reader.prepare_uninitialized_buffer(buf);
                let real_buf = std::mem::MaybeUninit::slice_get_mut(buf);
                let size = reader.read_exact(real_buf).await.map_err(header::Error::Io)?;
                payload.advance_mut(size);
            };

            if let Some(ref key) = header.masking_key {
                key.apply(&mut payload[start..]);
            }

            match header.kind {
                HeaderKind::Continuation => if header.is_final {
                    break;
                }
                HeaderKind::Binary | HeaderKind::Text => {
                    if payload.len() != header.payload_len as usize {
                        return Err(header::Error::InvalidDataFrame.into())
                    } else if header.is_final {
                        break;
                    } else {
                        header = Header::read(reader).await?;
                    }
                }
                HeaderKind::Close | HeaderKind::Ping | HeaderKind::Pong => {
                    if !header.is_final || payload.len() != header.payload_len as usize {
                        return Err(header::Error::InvalidDataFrame.into())
                    }
                    break;
                }
            }
        }
        Self::new(message_kind, payload.freeze())
    }
    pub fn buf(&self) -> &Bytes {
        &self.data
    }
    pub fn message(&self) -> Message {
        match self.kind {
            header::Kind::Continuation => unreachable!(),
            header::Kind::Text => unsafe {
                Message::Text(str::from_utf8_unchecked(&self.data))
            },
            header::Kind::Binary => Message::Binary(&self.data),
            header::Kind::Close => {
                if self.data.len() < 2 {
                    Message::Close(None)
                } else {
                    let code = ((self.data[0] as u16) << 8) | self.data[1] as u16;
                    if self.data.len() > 2 {
                        unsafe {
                            Message::Close(Some((code, str::from_utf8_unchecked(&self.data[2..]))))
                        }
                    } else {
                        Message::Close(Some((code, "")))
                    }
                }
            },
            header::Kind::Ping => Message::Ping(&self.data),
            header::Kind::Pong => Message::Pong(&self.data)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Context {
    Client, Server
}


#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Message<'a> {
    Text(&'a str),
    Binary(&'a [u8]),
    Close(Option<(u16, &'a str)>),
    Ping(&'a [u8]),
    Pong(&'a [u8])
}
impl<'a> Message<'a> {
    pub async fn write<W: AsyncWrite + Unpin>(self, writer: &mut W, ctx: Context) -> Result<(), io::Error> {
        let len = match self {
            Message::Text(s) => s.len(),
            Message::Binary(b)
            | Message::Ping(b)
            | Message::Pong(b) => b.len(),
            Message::Close(Some((_, s))) => s.len() + 2,
            Message::Close(None) => 0,
        };
        if len > 0 {
            let mask = match ctx {
                Context::Client => Some(MaskingKey::new()?),
                Context::Server => None
            };
            let header = Header {
                is_final: true,
                extensions: [false, false, false],
                kind: match self {
                    Message::Text(_) => HeaderKind::Text,
                    Message::Binary(_) => HeaderKind::Binary,
                    Message::Close(_) => HeaderKind::Close,
                    Message::Ping(_) => HeaderKind::Ping,
                    Message::Pong(_) => HeaderKind::Pong
                },
                payload_len: len as u64,
                masking_key: mask
            };
            let hbytes = header.bytes();
            writer.write_all(hbytes.as_ref()).await?;

            let mut data: SmallVec<[u8; 2048]>;
            let tmp_buf: [u8; 2];
            let bufs: (&[u8], &[u8]) = if let Some(key) = mask {
                data = SmallVec::with_capacity(len);
                match self {
                    Message::Text(s) => data.extend_from_slice(s.as_bytes()),
                    Message::Binary(b)
                    | Message::Ping(b)
                    | Message::Pong(b) => data.extend_from_slice(b),
                    Message::Close(Some((c, s))) => {
                        data.push((c >> 8 & 0xff) as u8);
                        data.push((c      & 0xff) as u8);
                        data.extend_from_slice(s.as_bytes());
                    }
                    Message::Close(None) => (),
                }
                key.apply(&mut data);
                (&*data, &[])
            } else {
                match self {
                    Message::Text(s) => (s.as_bytes(), &[]),
                    Message::Binary(b)
                    | Message::Ping(b)
                    | Message::Pong(b) => (b, &[]),
                    Message::Close(Some((c, s))) => {
                        tmp_buf = [(c >> 8 & 0xff) as u8, (c & 0xff) as u8];
                        (&tmp_buf, s.as_bytes())
                    }
                    Message::Close(None) => (&[], &[])
                }
            };

            if !bufs.0.is_empty() {
                writer.write_all(bufs.0).await?;
            }
            if !bufs.1.is_empty() {
                writer.write_all(bufs.1).await?;
            }
        }
        Ok(())
    }
}
