pub use cipher_spec::lookup_cipher;
pub use cipher_spec::CipherSpec;
pub use cipher_spec::CipherType;

use crate::Error;
use crate::Result;
use cipher_spec::AES_256_GCM;

mod cipher_spec;
mod hkdf;
mod key_type;
mod openssl_crypter;
mod sodium_crypter;

// Crypto-related modules as described in https://shadowsocks.org/en/spec/AEAD-Ciphers.

pub enum NonceType {
    Sequential,
    #[allow(dead_code)]
    Zero,
}

const TAG_BYTES: usize = 16;
const NONCE_BYTES: usize = 12;

// A wrapper of underlying crypto algorithm that encrypts and decrypts bytes.
pub trait Crypter: Send {
    fn encrypt(&mut self, data: &[u8]) -> Result<Vec<u8>>;
    fn decrypt(&mut self, data: &[u8]) -> Result<Vec<u8>>;
    fn expected_ciphertext_length(&self, plaintext_length: usize) -> usize;
}

pub fn create_crypter(
    key_bytes: &[u8],
    nonce_type: NonceType,
    cipher_type: CipherType,
) -> Box<dyn Crypter> {
    if cipher_type == CipherType::Chacha20IetfPoly1305 {
        let crypter =
            sodium_crypter::Chacha20IetfPoly1305Crypter::create_crypter(
                key_bytes, nonce_type,
            );
        let spec = cipher_type.spec();
        assert_eq!(
            spec.key_size,
            sodium_crypter::Chacha20IetfPoly1305Crypter::KEY_BYTES
        );
        assert_eq!(
            spec.nonce_size,
            sodium_crypter::Chacha20IetfPoly1305Crypter::NONCE_BYTES
        );
        return Box::new(crypter);
    }

    let cipher = match cipher_type {
        CipherType::Aes256GCM => openssl::symm::Cipher::aes_256_gcm(),
        CipherType::Aes192GCM => openssl::symm::Cipher::aes_192_gcm(),
        CipherType::Aes128GCM => openssl::symm::Cipher::aes_128_gcm(),
        _ => {
            log::warn!(
                "Unsupported cipher {:?}, falling back to aes-256-gcm",
                cipher_type
            );
            openssl::symm::Cipher::aes_256_gcm()
        }
    };

    let spec = cipher_type.spec();
    assert_eq!(spec.key_size, cipher.key_len());
    // An iv_len of None indicates that the cipher does not support IV.
    assert_eq!(spec.nonce_size, cipher.iv_len().unwrap_or(0));

    Box::new(openssl_crypter::OpensslCrypter::create(
        cipher, key_bytes, nonce_type,
    ))
}

// Recommended way of deriving a key from a password. Incompatible with the method used in the
// original Shadowsocks Python version.
// PBKDF2 is defined in RFC2898, as a supersede version of PBKDF1 implemented by EVP_BytesToKey() in
// OpenSSL and BoringSSL.
const RECOMMENDED_ITERATION_COUNT: u32 = 1000; // Iteration count recommended by RFC2898.

#[cfg(feature = "ring-crypto")]
pub fn derive_master_key_pbkdf2(
    password: &[u8],
    salt: &[u8],
    key_size: usize,
) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; key_size];
    ring::pbkdf2::derive(
        ring::pbkdf2::PBKDF2_HMAC_SHA256,
        std::num::NonZeroU32::new(RECOMMENDED_ITERATION_COUNT)
            .expect("Count should be greater than zero"),
        salt,
        password,
        buf.as_mut_slice(),
    );
    Ok(buf)
}

#[cfg(not(feature = "ring-crypto"))]
pub fn derive_master_key_pbkdf2(
    password: &[u8],
    salt: &[u8],
    key_size: usize,
) -> Result<Vec<u8>> {
    let mut buf = vec![0u8; key_size];
    let derive_result = openssl::pkcs5::pbkdf2_hmac(
        password,
        salt,
        RECOMMENDED_ITERATION_COUNT as usize,
        openssl::hash::MessageDigest::sha256(),
        buf.as_mut_slice(),
    );
    match derive_result {
        Ok(_) => Ok(buf),
        Err(e) => {
            log::error!("Error deriving key {}", e);
            Err(Error::KeyDerivationError)
        }
    }
}

// The key derivation method used by the original Shadowsocks Python version.
// The derived key should be identical to the one generated by the Python version. The derived IV is
// different from the Python version. Fortunately IV is used as salt and set by the party that
// initiates a connection, thus could really be any string we want.
pub fn derive_master_key_compatible(
    password: &[u8],
    key_size: usize,
) -> Result<Vec<u8>> {
    let longest_key_size: usize = AES_256_GCM.key_size;
    if key_size > longest_key_size {
        panic!("Cannot derive a key longer than {}", longest_key_size);
    }

    let derive_result = openssl::pkcs5::bytes_to_key(
        // AES 256 GCM has the longest key size known. We'll only use a prefix of it.
        openssl::symm::Cipher::aes_256_gcm(),
        openssl::hash::MessageDigest::md5(),
        password,
        None,
        1,
    );
    match derive_result {
        // IV is discarded.
        // This is really a hack: taking the first a few bits of a longer key to form a shorter key.
        // Due to the nature of EVP_bytes_to_key(), it produces exactly the same result as passing
        // in the shorter key size.
        Ok(key_iv_pair) => Ok(key_iv_pair.key[..key_size].to_vec()),
        Err(e) => {
            log::error!("Error deriving key {}", e);
            Err(Error::KeyDerivationError)
        }
    }
}

const SHADOW_INFO: &'static [u8] = b"ss-subkey";

#[cfg(feature = "ring-crypto")]
fn derive_subkey_with_algorithm(
    master_key: &[u8],
    salt: &[u8],
    key_size: usize,
    use_sha1: bool,
) -> Vec<u8> {
    let algorithm = if use_sha1 {
        ring::hkdf::HKDF_SHA1_FOR_LEGACY_USE_ONLY
    } else {
        ring::hkdf::HKDF_SHA256
    };
    let salt = ring::hkdf::Salt::new(algorithm, salt);
    let prk = salt.extract(master_key);
    let info = &[SHADOW_INFO];
    let okm = prk
        .expand(info, key_type::KeyType(key_size))
        .expect("Should not expand key to too long");
    let mut ret = vec![0u8; key_size];
    okm.fill(&mut ret).expect("Should not fill key to too long");
    ret
}

#[cfg(not(feature = "ring-crypto"))]
fn derive_subkey_with_algorithm(
    master_key: &[u8],
    salt: &[u8],
    key_size: usize,
    use_sha1: bool,
) -> Vec<u8> {
    #[cfg(feature = "ring-digest-in-hkdf")]
    let algorithm = if use_sha1 {
        hkdf::SHA1_FOR_COMPATIBILITY
    } else {
        hkdf::SHA256
    };
    #[cfg(not(feature = "ring-digest-in-hkdf"))]
    let algorithm = if use_sha1 {
        hkdf::OpensslSha::sha1()
    } else {
        hkdf::OpensslSha::sha256()
    };
    let hkdf = hkdf::Hkdf::extract(Some(salt), master_key, algorithm);
    hkdf.expand(SHADOW_INFO, key_size)
}

// Subkey is derived from the master key using HKDF method. See crypto/hkdf.rs for more details.
// SHA1 is deemed insecure in modern computing. SHA256 is recommended instead.
pub fn derive_subkey(
    master_key: &[u8],
    salt: &[u8],
    key_size: usize,
    compatible_mode: bool,
) -> Vec<u8> {
    derive_subkey_with_algorithm(
        master_key,
        salt,
        key_size,
        /* use_sha1= */ compatible_mode,
    )
}

#[cfg(test)]
#[rustfmt::skip::macros(crypto_array, crypto_vec)]
mod test {
    use crate::crypto::cipher_spec::{
        AES_128_GCM, AES_192_GCM, CHACHA20_IETF_POLY1305,
    };

    use super::*;

    fn derive_subkey_compatible(
        master_key: &[u8],
        salt: &[u8],
        key_size: usize,
    ) -> Vec<u8> {
        derive_subkey(master_key, salt, key_size, true)
    }

    // Expected keys are copied from the output of the Python version.
    #[test]
    fn test_master_key_derivation_compatibility() -> Result<()> {
        let key = derive_master_key_compatible(b"key", 32)?;
        assert_eq!(
            key,
            &crypto_array![
                0x3C, 0x6E, 0x0B, 0x8A, 0x9C, 0x15, 0x22, 0x4A,
                0x82, 0x28, 0xB9, 0xA9, 0x8C, 0xA1, 0x53, 0x1D,
                0xD1, 0xE2, 0xA3, 0x5F, 0xBA, 0x50, 0x9B, 0x64,
                0x32, 0xED, 0xB9, 0x6D, 0x85, 0x0E, 0x11, 0x9F
            ]
        );

        let key = derive_master_key_compatible(b"a short password", 32)?;
        assert_eq!(
            key,
            &crypto_array![
                0xD2, 0x9F, 0xA4, 0x2C, 0x9B, 0xEC, 0xEA, 0x63,
                0xC6, 0xBD, 0xE5, 0x40, 0xA2, 0x4A, 0x99, 0x52,
                0x1E, 0x3E, 0xD7, 0x67, 0xF2, 0x52, 0x19, 0x84,
                0x6D, 0x61, 0x2B, 0x2A, 0x48, 0x04, 0x99, 0xA4
            ]
        );

        let key = derive_master_key_compatible(b"deadbeef", 32)?;
        assert_eq!(
            key,
            &crypto_array![
                0x4F, 0x41, 0x24, 0x38, 0x47, 0xDA, 0x69, 0x3A,
                0x4F, 0x35, 0x6C, 0x04, 0x86, 0x11, 0x4B, 0xC6,
                0x10, 0xB8, 0x5C, 0x02, 0x5B, 0xF1, 0xCF, 0x25,
                0xD9, 0x5F, 0x41, 0x3C, 0x0D, 0xED, 0x7C, 0x70
            ]
        );

        let key = derive_master_key_compatible(
            b"sodiumoxide::crypto::aead::chacha20poly1305_ietf",
            32,
        )?;
        assert_eq!(
            key,
            &crypto_array![
                0x4D, 0xC8, 0xA1, 0xA7, 0xBC, 0x06, 0x74, 0x4D,
                0x9C, 0x6B, 0x4F, 0xB3, 0x27, 0xFF, 0x52, 0x69,
                0x3C, 0x44, 0xF1, 0xBD, 0x94, 0xD2, 0x7D, 0xD4,
                0xD6, 0xE1, 0x90, 0xAF, 0x65, 0x71, 0x99, 0x7D
            ]
        );
        Ok(())
    }

    #[test]
    fn test_master_key_derivation_compatibility_short_keys() -> Result<()> {
        let key = derive_master_key_compatible(b"deadbeef", 24)?;
        assert_eq!(
            key,
            &crypto_array![
                0x4F, 0x41, 0x24, 0x38, 0x47, 0xDA, 0x69, 0x3A,
                0x4F, 0x35, 0x6C, 0x04, 0x86, 0x11, 0x4B, 0xC6,
                0x10, 0xB8, 0x5C, 0x02, 0x5B, 0xF1, 0xCF, 0x25,
            ]
        );

        let key = derive_master_key_compatible(
            b"sodiumoxide::crypto::aead::chacha20poly1305_ietf",
            16,
        )?;
        assert_eq!(
            key,
            &crypto_array![
                0x4D, 0xC8, 0xA1, 0xA7, 0xBC, 0x06, 0x74, 0x4D,
                0x9C, 0x6B, 0x4F, 0xB3, 0x27, 0xFF, 0x52, 0x69,
            ]
        );
        Ok(())
    }

    #[test]
    fn test_derive_subkey_compatibility_short() -> Result<()> {
        let subkey = derive_subkey_compatible(
            &crypto_array![
                0x4F, 0x41, 0x24, 0x38, 0x47, 0xDA, 0x69, 0x3A,
                0x4F, 0x35, 0x6C, 0x04, 0x86, 0x11, 0x4B, 0xC6,
                0x10, 0xB8, 0x5C, 0x02, 0x5B, 0xF1, 0xCF, 0x25,
                0xD9, 0x5F, 0x41, 0x3C, 0x0D, 0xED, 0x7C, 0x70
            ],
            &crypto_array![
                0xA0, 0x78, 0xAD, 0xDA, 0xA6, 0x66, 0xAB, 0x30,
                0x40, 0x20, 0x41, 0x22, 0x29, 0x53, 0x6E, 0x89,
                0x6E, 0x8A, 0x9E, 0x81, 0x9C, 0x61, 0x1B, 0xAF,
                0xDF, 0xBF, 0x6D, 0x53, 0x68, 0x3B, 0xDB, 0xF4
            ],
            32,
        );
        assert_eq!(
            subkey,
            &crypto_array![
                0xC1, 0xC9, 0x96, 0xB1, 0x6A, 0xFB, 0xE9, 0xDC,
                0xBB, 0xAF, 0xAD, 0xCD, 0xEC, 0x9C, 0x21, 0x9C,
                0x9B, 0x9A, 0x45, 0x53, 0xEB, 0xF9, 0x28, 0x63,
                0xEB, 0xBE, 0x28, 0x6C, 0x65, 0x2C, 0xC6, 0x42
            ]
        );
        Ok(())
    }

    #[test]
    fn test_derive_subkey_compatibility_long() -> Result<()> {
        let subkey = derive_subkey_compatible(
            &crypto_array![
                0x4D, 0xC8, 0xA1, 0xA7, 0xBC, 0x06, 0x74, 0x4D,
                0x9C, 0x6B, 0x4F, 0xB3, 0x27, 0xFF, 0x52, 0x69,
                0x3C, 0x44, 0xF1, 0xBD, 0x94, 0xD2, 0x7D, 0xD4,
                0xD6, 0xE1, 0x90, 0xAF, 0x65, 0x71, 0x99, 0x7D
            ],
            &crypto_array![
                0x52, 0x9D, 0x82, 0xCA, 0x84, 0xC0, 0x18, 0x65,
                0xC5, 0xC6, 0xC7, 0xC4, 0xBC, 0xC0, 0xF8, 0xDB,
                0x2C, 0x92, 0xE1, 0xB2, 0xA0, 0x19, 0x0D, 0xC5,
                0x7B, 0xD9, 0xFF, 0x9F, 0x10, 0x7B, 0x60, 0x77
            ],
            32,
        );
        assert_eq!(
            subkey,
            &crypto_array![
                0x6D, 0x5A, 0x78, 0x56, 0x83, 0xB0, 0x8D, 0x6B,
                0x8B, 0xCD, 0xC5, 0x61, 0x48, 0x3F, 0x44, 0x1D,
                0x7E, 0x12, 0xAA, 0x45, 0x9C, 0x18, 0x71, 0xAA,
                0x5C, 0xBB, 0xEB, 0x50, 0x0E, 0x72, 0xAB, 0x2C
            ]
        );
        Ok(())
    }

    // See the comment of function derive_master_key_compatible() about how shorter keys are
    // derived. A run time error will be thrown if those conditions are not met.
    #[test]
    fn test_key_size_not_too_long() {
        assert!(AES_256_GCM.key_size >= CHACHA20_IETF_POLY1305.key_size);
        assert!(AES_256_GCM.key_size >= AES_192_GCM.key_size);
        assert!(AES_256_GCM.key_size >= AES_128_GCM.key_size);
    }

    // The assumptions made during the implementation of related libraries.
    #[test]
    fn test_size_assumptions() {
        for spec in &[
            &AES_128_GCM,
            &AES_192_GCM,
            &AES_256_GCM,
            &CHACHA20_IETF_POLY1305,
        ] {
            assert_eq!(spec.key_size, spec.salt_size);
            assert_eq!(spec.nonce_size, NONCE_BYTES);
            assert_eq!(spec.tag_size, TAG_BYTES);
        }
    }
}
