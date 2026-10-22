//! Persistent device identity. One TLS key pair per installation lets paired
//! devices recognise each other across restarts and address changes: a peer is
//! known by its certificate fingerprint, never by its name or IP address.
use anyhow::{Context, Result, ensure};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{io::Write, path::Path};

const FILE: &str = "identity.json";

#[derive(Serialize, Deserialize)]
struct Saved {
    version: u8,
    certificate: String,
    key: String,
    #[serde(default)]
    port: u16,
}

pub struct Identity {
    pub certificate: Vec<u8>,
    pub key: Vec<u8>,
    pub fingerprint: String,
    /// The listener port used last time, so peers can reach this device at the
    /// same port after a restart even when discovery is blocked.
    pub port: u16,
}

pub fn fingerprint(der: &[u8]) -> String {
    hex::encode(Sha256::digest(der))
}

impl Identity {
    pub fn load_or_create(root: &Path) -> Result<Self> {
        let path = root.join(FILE);
        match std::fs::read(&path) {
            Ok(bytes) => {
                let saved: Saved = serde_json::from_slice(&bytes)
                    .context("The device identity file is damaged. Move it away to create a new identity; paired devices will need to pair again.")?;
                ensure!(saved.version == 1, "Unsupported device identity version");
                let base64 = base64::engine::general_purpose::STANDARD;
                let certificate = base64.decode(saved.certificate)?;
                let key = base64.decode(saved.key)?;
                Ok(Self {
                    fingerprint: fingerprint(&certificate),
                    certificate,
                    key,
                    port: saved.port,
                })
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // Names are not identity; the fingerprint is. Hostname checks are
                // replaced by fingerprint pinning (see transport::pinned_client).
                let generated = rcgen::generate_simple_self_signed(vec!["drift.local".into()])?;
                let identity = Self {
                    certificate: generated.cert.der().to_vec(),
                    key: generated.key_pair.serialize_der(),
                    fingerprint: fingerprint(generated.cert.der()),
                    port: 0,
                };
                identity.save(root)?;
                Ok(identity)
            }
            Err(error) => Err(error).context("Could not read the device identity"),
        }
    }

    pub fn remember_port(&mut self, root: &Path, port: u16) -> Result<()> {
        if self.port != port {
            self.port = port;
            self.save(root)?;
        }
        Ok(())
    }

    // Write-then-rename keeps the old identity intact if the process stops midway.
    fn save(&self, root: &Path) -> Result<()> {
        let base64 = base64::engine::general_purpose::STANDARD;
        let saved = Saved {
            version: 1,
            certificate: base64.encode(&self.certificate),
            key: base64.encode(&self.key),
            port: self.port,
        };
        let temporary = root.join(format!("{FILE}.tmp"));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(&serde_json::to_vec(&saved)?)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, root.join(FILE))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_survives_restart_and_is_private() {
        let root = tempfile::tempdir().unwrap();
        let mut first = Identity::load_or_create(root.path()).unwrap();
        first.remember_port(root.path(), 50123).unwrap();
        let second = Identity::load_or_create(root.path()).unwrap();
        assert_eq!(first.fingerprint, second.fingerprint);
        assert_eq!(second.key, first.key);
        assert_eq!(second.port, 50123);
        assert_eq!(second.fingerprint.len(), 64);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(root.path().join(FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "identity readable by other users");
        }
        let other = tempfile::tempdir().unwrap();
        assert_ne!(
            Identity::load_or_create(other.path()).unwrap().fingerprint,
            first.fingerprint
        );
    }

    #[test]
    fn damaged_identity_is_reported_not_replaced() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join(FILE), b"not json").unwrap();
        let error = Identity::load_or_create(root.path()).err().unwrap();
        assert!(error.to_string().contains("damaged"));
        assert_eq!(std::fs::read(root.path().join(FILE)).unwrap(), b"not json");
    }
}
