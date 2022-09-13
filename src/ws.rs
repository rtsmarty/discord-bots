use base64::{
    encode_config_slice,
    STANDARD,
};
use ring::digest::{
    SHA1_FOR_LEGACY_USE_ONLY,
    digest,
};
use rand::{
    rngs::OsRng,
    RngCore,
};
use std::{
    fmt,
    hash,
    io,
    str::{
        self,
        FromStr,
    },
};

const MAX_REQUEST_KEY_LEN: usize = (16 / 3) * 4 + 4;
const MAGIC_GUID_LEN: usize = 36;
const MAGIC_GUID: &[u8; MAGIC_GUID_LEN] = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const MAX_CONCAT_LEN: usize = MAX_REQUEST_KEY_LEN + MAGIC_GUID_LEN;
const MAX_RESPONSE_KEY_LEN: usize = (20 / 3) * 4 + 4;

mod header;
pub mod message;

#[doc(inline)]
pub use self::message::Message;

#[derive(Clone, Copy, Eq)]
pub struct RequestKey {
    bytes: [u8; MAX_REQUEST_KEY_LEN],
    size: u8
}
impl RequestKey {
    pub fn generate() -> io::Result<RequestKey> {
        let mut bytes = [0; 16];
        OsRng.try_fill_bytes(&mut bytes)?;

        let mut key = [0; MAX_REQUEST_KEY_LEN];
        let size = encode_config_slice(&bytes as &[u8], STANDARD, &mut key);
        Ok(RequestKey {
            bytes: key,
            size: size as u8
        })
    }
    pub fn verify(&self, response: ResponseKey) -> bool {
        let expected = ResponseKey::from(*self);
        expected == response
    }
}
impl fmt::Debug for RequestKey {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("RequestKey").field("key", &self.as_ref()).finish()
    }
}
impl PartialEq for RequestKey {
    fn eq(&self, other: &Self) -> bool {
        self.as_ref() == other.as_ref()
    }
}
impl hash::Hash for RequestKey {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        self.as_ref().hash(state)
    }
}
impl AsRef<str> for RequestKey {
    fn as_ref(&self) -> &str {
        // base64 encoding is utf8 safe
        unsafe {
            str::from_utf8_unchecked(self.bytes.get_unchecked(..self.size as usize))
        }
    }
}
impl FromStr for RequestKey {
    type Err = ();
    fn from_str(string: &str) -> Result<RequestKey, Self::Err> {
        let string = string.trim();
        if string.len() > MAX_REQUEST_KEY_LEN {
            Err(())
        } else {
            let mut key = [0; MAX_REQUEST_KEY_LEN];
            key[..string.len()].copy_from_slice(string.as_bytes());
            Ok(RequestKey {
                bytes: key,
                size: string.len() as u8
            })
        }
    }
}

#[derive(Clone, Copy, Eq)]
pub struct ResponseKey {
    bytes: [u8; MAX_RESPONSE_KEY_LEN],
    size: u8
}
impl From<RequestKey> for ResponseKey {
    fn from(req: RequestKey) -> ResponseKey {
        let keystr = req.as_ref();
        let len = keystr.len() + MAGIC_GUID_LEN;

        let mut concat: [u8; MAX_CONCAT_LEN] = [0; MAX_CONCAT_LEN];
        concat[..keystr.len()].copy_from_slice(keystr.as_bytes());
        concat[keystr.len()..len].copy_from_slice(MAGIC_GUID);

        let digest = digest(&SHA1_FOR_LEGACY_USE_ONLY, &concat[..len]);

        let mut key = [0; MAX_RESPONSE_KEY_LEN];
        let size = encode_config_slice(digest.as_ref(), STANDARD, &mut key);
        ResponseKey {
            bytes: key,
            size: size as u8
        }
    }
}
impl fmt::Debug for ResponseKey {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("ResponseKey").field("key", &self.as_ref()).finish()
    }
}
impl PartialEq for ResponseKey {
    fn eq(&self, other: &Self) -> bool {
        self.as_ref() == other.as_ref()
    }
}
impl hash::Hash for ResponseKey {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        self.as_ref().hash(state)
    }
}
impl AsRef<str> for ResponseKey {
    fn as_ref(&self) -> &str {
        // base64 encoding is utf8 safe
        unsafe {
            str::from_utf8_unchecked(self.bytes.get_unchecked(..self.size as usize))
        }
    }
}
impl FromStr for ResponseKey {
    type Err = ();
    fn from_str(string: &str) -> Result<ResponseKey, Self::Err> {
        let string = string.trim();
        if string.len() > MAX_RESPONSE_KEY_LEN {
            Err(())
        } else {
            let mut key = [0; MAX_RESPONSE_KEY_LEN];
            key[..string.len()].copy_from_slice(string.as_bytes());
            Ok(ResponseKey {
                bytes: key,
                size: string.len() as u8
            })
        }
    }
}

