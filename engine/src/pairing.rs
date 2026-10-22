use crate::model::{Invitation, PairRequest, now};
use anyhow::{Result, bail, ensure};
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::Sha256;
use std::collections::HashSet;

pub fn secret(bytes: usize) -> String {
    let mut value = vec![0; bytes];
    rand::thread_rng().fill_bytes(&mut value);
    hex::encode(value)
}

/// The hash a device commits to before it sees the other device's nonce.
pub fn commitment(nonce: &str) -> String {
    use sha2::Digest;
    hex::encode(Sha256::digest(
        format!("drift-commitment-v1:{nonce}").as_bytes(),
    ))
}

/// The six digits both screens show when a nearby device is tapped. Both
/// certificate fingerprints and both nonces go in; the initiator committed to
/// its nonce first, so a device in the middle cannot steer the two codes to
/// match (one chance in a million per attempt).
pub fn short_code(server: &str, client: &str, server_nonce: &str, client_nonce: &str) -> String {
    use sha2::Digest;
    let digest = Sha256::digest(
        serde_json::to_vec(&(
            "drift-short-code-v1",
            server,
            client,
            server_nonce,
            client_nonce,
        ))
        .unwrap(),
    );
    let value = u64::from_be_bytes(digest[..8].try_into().unwrap()) % 1_000_000;
    format!("{:03} {:03}", value / 1000, value % 1000)
}

pub fn proof(code: &str, role: &str, fingerprint: &str, nonce: &str, name: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(code.as_bytes()).unwrap();
    mac.update(&serde_json::to_vec(&(1, role, fingerprint, nonce, name)).unwrap());
    hex::encode(mac.finalize().into_bytes())
}

pub fn verify(
    code: &str,
    role: &str,
    fingerprint: &str,
    nonce: &str,
    name: &str,
    signature: &str,
) -> Result<()> {
    let mut mac = Hmac::<Sha256>::new_from_slice(code.as_bytes())?;
    mac.update(&serde_json::to_vec(&(1, role, fingerprint, nonce, name))?);
    mac.verify_slice(&hex::decode(signature)?)?;
    Ok(())
}

pub struct Pairing {
    pub invitation: Invitation,
    used: bool,
    seen: HashSet<String>,
    attempts: u32,
    window: u64,
}

impl Pairing {
    pub fn new(host: String, port: u16) -> Self {
        Self {
            invitation: Invitation {
                version: 1,
                host,
                port,
                code: secret(15),
                expires: now() + 300,
            },
            used: false,
            seen: HashSet::new(),
            attempts: 0,
            window: now(),
        }
    }

    pub fn challenge(&mut self, fingerprint: &str, nonce: &str) -> Result<String> {
        if now().saturating_sub(self.window) >= 60 {
            self.attempts = 0;
            self.window = now();
        }
        self.attempts += 1;
        ensure!(
            self.attempts <= 60,
            "Too many pairing attempts. Wait one minute."
        );
        ensure!(
            !self.used && now() < self.invitation.expires,
            "Invitation expired or used. Generate a new invitation."
        );
        ensure!(
            nonce.len() == 64 && hex::decode(nonce).is_ok(),
            "Invalid nonce"
        );
        ensure!(self.seen.len() < 600, "Invitation exhausted");
        self.seen.insert(nonce.to_owned());
        Ok(proof(
            &self.invitation.code,
            "server",
            fingerprint,
            nonce,
            "",
        ))
    }

    pub fn consume(&mut self, fingerprint: &str, request: &PairRequest) -> Result<()> {
        ensure!(
            !self.used && now() < self.invitation.expires,
            "Invitation expired or used"
        );
        ensure!(
            request.name.len() <= 100 && !request.name.trim().is_empty(),
            "Invalid device name"
        );
        if !self.seen.remove(&request.nonce) {
            bail!("Challenge is missing or already used");
        }
        verify(
            &self.invitation.code,
            "client",
            fingerprint,
            &request.nonce,
            &request.name,
            &request.proof,
        )?;
        self.used = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_codes_depend_on_every_input() {
        let code = short_code("a", "b", "n1", "n2");
        assert_eq!(code.len(), 7);
        assert!(code.chars().all(|c| c.is_ascii_digit() || c == ' '));
        assert_eq!(code, short_code("a", "b", "n1", "n2"));
        for other in [
            short_code("x", "b", "n1", "n2"),
            short_code("a", "x", "n1", "n2"),
            short_code("a", "b", "x", "n2"),
            short_code("a", "b", "n1", "x"),
            short_code("b", "a", "n1", "n2"),
        ] {
            assert_ne!(other, code);
        }
        assert_ne!(commitment("n1"), commitment("n2"));
        assert_eq!(commitment("n1").len(), 64);
    }
}
