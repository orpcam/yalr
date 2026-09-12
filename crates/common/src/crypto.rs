//! Verschluesselung fuer Provider-API-Keys und Virtual-Key-Plaintexts (at rest).
//!
//! Konstruktion: random Nonce (16 bytes) + HMAC-SHA256-keystream (CTR-aehnlich)
//! + Encrypt-then-MAC mit independenter HMAC-Subkey. Format:
//! `v2:<nonce_b64>:<tag_b64>:<cipher_b64>`
//!
//! Die alte v1-XOR-Format bleibt lesbar (Provider-Keys in bestehenden DBs).

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use sha2::{Digest, Sha256};

/// Schluesselableitung: zwei unabhaengige Subkeys (enc/mac) aus dem master-secret.
fn derive_subkeys(key: &str) -> ([u8; 32], [u8; 32]) {
    let mut enc_hash = Sha256::new();
    enc_hash.update(key.as_bytes());
    enc_hash.update(b"|yalr-enc");
    let enc: [u8; 32] = enc_hash.finalize().into();

    let mut mac_hash = Sha256::new();
    mac_hash.update(key.as_bytes());
    mac_hash.update(b"|yalr-mac");
    let mac: [u8; 32] = mac_hash.finalize().into();

    (enc, mac)
}

/// HMAC-SHA256 (minimal, ohne deps).
fn hmac(key: &[u8], data: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        let d = Sha256::digest(key);
        k[..32].copy_from_slice(&d);
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let ipad: Vec<u8> = k.iter().map(|b| b ^ 0x36).collect();
    let opad: Vec<u8> = k.iter().map(|b| b ^ 0x5c).collect();

    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(data);
    let inner_digest: [u8; 32] = inner.finalize().into();

    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_digest);
    outer.finalize().into()
}

/// Deterministischer keystream aus (subkey, nonce) — jede (key, nonce)-Kombi
/// erzeugt einen eindeutigen stream, nonce darf nur einmal pro key verwendet werden.
fn keystream(subkey: &[u8; 32], nonce: &[u8], len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    let mut counter: u64 = 0;
    while out.len() < len {
        let mut h = Sha256::new();
        h.update(subkey);
        h.update(nonce);
        h.update(&counter.to_be_bytes());
        let digest: [u8; 32] = h.finalize().into();
        out.extend_from_slice(&digest);
        counter += 1;
    }
    out.truncate(len);
    out
}

/// Verschluesselt plaintext mit dem master-secret (AEAD-aehnlich: nonce + tag).
pub fn encrypt(plain: &str, key: &str) -> String {
    let (enc_key, mac_key) = derive_subkeys(key);

    // 16 bytes random nonce
    let mut nonce = [0u8; 16];
    use rand::RngCore;
    rand::thread_rng().fill_bytes(&mut nonce);

    let stream = keystream(&enc_key, &nonce, plain.len());
    let cipher: Vec<u8> = plain
        .bytes()
        .zip(stream)
        .map(|(b, k)| b ^ k)
        .collect();

    // tag ueber nonce + ciphertext (encrypt-then-mac)
    let mut mac_input = Vec::with_capacity(nonce.len() + cipher.len());
    mac_input.extend_from_slice(&nonce);
    mac_input.extend_from_slice(&cipher);
    let tag = hmac(&mac_key, &mac_input);

    format!(
        "v2:{}:{}:{}",
        B64.encode(nonce),
        B64.encode(tag),
        B64.encode(cipher)
    )
}

/// Entschluesselt v2 (nonce+tag) oder v1 (legacy-XOR) format.
pub fn decrypt(cipher_text: &str, key: &str) -> anyhow::Result<String> {
    if let Some(rest) = cipher_text.strip_prefix("v2:") {
        let mut parts = rest.split(':');
        let nonce_b64 = parts.next().ok_or_else(|| anyhow::anyhow!("bad format"))?;
        let tag_b64 = parts.next().ok_or_else(|| anyhow::anyhow!("bad format"))?;
        let cipher_b64 = parts.next().ok_or_else(|| anyhow::anyhow!("bad format"))?;

        let nonce = B64.decode(nonce_b64)?;
        let tag = B64.decode(tag_b64)?;
        let cipher = B64.decode(cipher_b64)?;

        let (enc_key, mac_key) = derive_subkeys(key);

        // tag pruefen VOR entschluesselung
        let mut mac_input = Vec::with_capacity(nonce.len() + cipher.len());
        mac_input.extend_from_slice(&nonce);
        mac_input.extend_from_slice(&cipher);
        let expected = hmac(&mac_key, &mac_input);
        if !constant_time_eq(&tag, &expected) {
            anyhow::bail!("authentication failed");
        }

        let stream = keystream(&enc_key, &nonce, cipher.len());
        let plain: Vec<u8> = cipher
            .into_iter()
            .zip(stream)
            .map(|(b, k)| b ^ k)
            .collect();
        return Ok(String::from_utf8(plain)?);
    }

    // v1 legacy (XOR-keystream ohne nonce/tag)
    decrypt_v1(cipher_text, key)
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

fn decrypt_v1(cipher_b64: &str, key: &str) -> anyhow::Result<String> {
    let bytes = B64.decode(cipher_b64)?;
    let stream = keystream_v1(key, bytes.len());
    let plain: Vec<u8> = bytes
        .into_iter()
        .zip(stream)
        .map(|(b, k)| b ^ k)
        .collect();
    Ok(String::from_utf8(plain)?)
}

fn keystream_v1(key: &str, len: usize) -> Vec<u8> {
    let mut seed = format!("{key}:yalr-keystream").into_bytes();
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        let digest = Sha256::digest(&seed);
        out.extend_from_slice(&digest);
        seed = digest.to_vec();
    }
    out.truncate(len);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let key = "test-secret";
        let plain = "sk-my-api-key-123";
        let cipher = encrypt(plain, key);
        assert_ne!(cipher, plain);
        assert!(cipher.starts_with("v2:"));
        assert_eq!(decrypt(&cipher, key).unwrap(), plain);
    }

    #[test]
    fn test_random_nonce_unique_ciphertexts() {
        let key = "test-secret";
        let plain = "sk-same-plaintext";
        let c1 = encrypt(plain, key);
        let c2 = encrypt(plain, key);
        assert_ne!(c1, c2, "nonce muss ciphertexts unterscheiden");
    }

    #[test]
    fn test_tamper_detection() {
        let key = "test-secret";
        let plain = "sk-my-api-key-123";
        let cipher = encrypt(plain, key);
        // ciphertext-byte flippen
        let mut parts: Vec<&str> = cipher.split(':').collect();
        let mut cbytes = B64.decode(parts[3]).unwrap();
        cbytes[0] ^= 0xff;
        parts[3] = "placeholder";
        let tampered = format!("v2:{}:{}:{}", parts[1], parts[2], B64.encode(cbytes));
        assert!(decrypt(&tampered, key).is_err());
    }

    #[test]
    fn test_wrong_key_fails() {
        let cipher = encrypt("sk-my-api-key-123", "correct-key");
        assert!(decrypt(&cipher, "wrong-key").is_err());
    }

    #[test]
    fn test_v1_legacy_still_decrypts() {
        // v1-format (XOR ohne prefix) muss weiter lesbar bleiben
        let key = "test-secret";
        let plain = "legacy-provider-key";
        let stream = keystream_v1(key, plain.len());
        let cipher: Vec<u8> = plain
            .bytes()
            .zip(stream)
            .map(|(b, k)| b ^ k)
            .collect();
        let v1 = B64.encode(cipher);
        assert_eq!(decrypt(&v1, key).unwrap(), plain);
    }

    #[test]
    fn test_sha256_known_vector() {
        let digest = Sha256::digest(b"abc");
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
