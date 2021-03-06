use std::str::FromStr;

use crate::Error;
use crate::Result;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum CipherType {
    Chacha20IetfPoly1305,
    XChacha20IetfPoly1305,
    Aes256GCM,
    Aes192GCM,
    Aes128GCM,
    #[cfg(test)]
    None,
}

impl CipherType {
    pub fn spec(self) -> &'static CipherSpec {
        let ret = match self {
            CipherType::Chacha20IetfPoly1305 => &CHACHA20_IETF_POLY1305,
            CipherType::XChacha20IetfPoly1305 => &XCHACHA20_IETF_POLY1305,
            CipherType::Aes256GCM => &AES_256_GCM,
            CipherType::Aes192GCM => &AES_192_GCM,
            CipherType::Aes128GCM => &AES_128_GCM,
            #[cfg(test)]
            CipherType::None => &NONE,
        };
        assert_eq!(ret.cipher_type, self);
        ret
    }
}

impl FromStr for CipherType {
    type Err = Error;

    fn from_str(name: &str) -> Result<Self> {
        let cipher_type = match name {
            "chacha20-ietf-poly1305" => CipherType::Chacha20IetfPoly1305,
            "xchacha20-ietf-poly1305" => CipherType::XChacha20IetfPoly1305,
            "aes-256-gcm" => CipherType::Aes256GCM,
            "aes-192-gcm" => CipherType::Aes192GCM,
            "aes-128-gcm" => CipherType::Aes128GCM,
            _ => return Err(Error::UnknownCipher(name.into())),
        };
        Ok(cipher_type)
    }
}

impl CipherType {
    const POSSIBLE_CIPHERS: [&'static str; 5] = [
        "aes-128-gcm",
        "aes-192-gcm",
        "aes-256-gcm",
        "chacha20-ietf-poly1305",
        "xchacha20-ietf-poly1305",
    ];

    pub fn possible_ciphers() -> &'static [&'static str] {
        &Self::POSSIBLE_CIPHERS
    }
}

pub struct CipherSpec {
    pub cipher_type: CipherType,
    pub key_size: usize,
    pub salt_size: usize,
    pub nonce_size: usize,
    pub tag_size: usize,
}

pub static CHACHA20_IETF_POLY1305: CipherSpec = CipherSpec {
    cipher_type: CipherType::Chacha20IetfPoly1305,
    key_size: 32,
    salt_size: 32,
    nonce_size: 12,
    tag_size: 16,
};

pub static XCHACHA20_IETF_POLY1305: CipherSpec = CipherSpec {
    cipher_type: CipherType::XChacha20IetfPoly1305,
    key_size: 32,
    salt_size: 32,
    nonce_size: 24,
    tag_size: 16,
};

pub static AES_256_GCM: CipherSpec = CipherSpec {
    cipher_type: CipherType::Aes256GCM,
    key_size: 32,
    salt_size: 32,
    nonce_size: 12,
    tag_size: 16,
};

pub static AES_192_GCM: CipherSpec = CipherSpec {
    cipher_type: CipherType::Aes192GCM,
    key_size: 24,
    salt_size: 24,
    nonce_size: 12,
    tag_size: 16,
};

pub static AES_128_GCM: CipherSpec = CipherSpec {
    cipher_type: CipherType::Aes128GCM,
    key_size: 16,
    salt_size: 16,
    nonce_size: 12,
    tag_size: 16,
};

#[cfg(test)]
pub static NONE: CipherSpec = CipherSpec {
    cipher_type: CipherType::None,
    key_size: 0,
    salt_size: 0,
    nonce_size: 0,
    tag_size: 0,
};
