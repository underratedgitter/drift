use serde::{Deserialize, Serialize};

// Amortize TLS/HTTP and durable filesystem/DB barriers without unbounded queues.
pub const CHUNK: usize = 8 * 1024 * 1024;
/// Negotiated durable window; legacy peers retain the 8 MiB chunk route.
pub const WINDOW: usize = 16 * 1024 * 1024;
pub const MAX_METADATA: usize = 2 * 1024 * 1024;
pub const MAX_FILES: usize = 10_000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub size: u64,
    pub blake3: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Invitation {
    pub version: u8,
    pub host: String,
    pub port: u16,
    pub code: String,
    pub expires: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PairRequest {
    pub nonce: String,
    pub name: String,
    pub proof: String,
    /// Present when the pairing device wants to be remembered and receive too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceHello>,
}

/// What a pairing device tells the receiver so it can call back later. Sent
/// only inside TLS pinned to the receiver, after its code-bound proof checked.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceHello {
    /// SHA-256 fingerprint of the sender's persistent TLS certificate.
    pub id: String,
    pub port: u16,
    /// Capability the receiver presents when it sends to this device.
    pub capability: String,
    pub os: String,
}

impl DeviceHello {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.id.len() == 64 && hex::decode(&self.id).is_ok(),
            "Invalid device identity"
        );
        anyhow::ensure!(
            self.capability.len() == 64 && hex::decode(&self.capability).is_ok(),
            "Invalid device capability"
        );
        anyhow::ensure!(self.port > 0, "Invalid device port");
        anyhow::ensure!(
            self.os.len() <= 40 && self.os.chars().all(|c| c.is_ascii_alphanumeric()),
            "Invalid device platform"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub name: String,
    pub approved: bool,
    pub confirmed: bool,
    pub created: u64,
    #[serde(skip_serializing, default)]
    pub capability: String,
    /// The pairing device's identity and address, kept until approval.
    #[serde(skip, default)]
    pub device: Option<(DeviceHello, String)>,
    /// For a tapped nearby device: the six digits both screens must show.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// First step of tap-to-pair: who is asking, and a commitment to its nonce.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Introduction {
    pub name: String,
    pub device: DeviceHello,
    pub commitment: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IntroductionReply {
    pub id: String,
    pub nonce: String,
    pub name: String,
    pub os: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Reveal {
    pub nonce: String,
}

/// A paired device remembered across restarts. Each side issued one capability
/// to the other; only a hash of the one presented to us is stored.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrustedDevice {
    /// The device's certificate fingerprint; connections are pinned to it.
    pub id: String,
    pub name: String,
    pub os: String,
    pub host: String,
    pub port: u16,
    /// Capability this device issued to us, presented when we call it.
    pub outgoing: String,
    /// SHA-256 of the capability we issued to it.
    pub incoming: String,
    /// Files from this device are accepted without asking.
    pub own: bool,
    pub added: u64,
}

pub fn capability_hash(capability: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(capability.as_bytes()))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Transfer {
    pub id: String,
    pub entries: Vec<Entry>,
    pub offsets: Vec<u64>,
    pub status: String,
    pub peer: String,
    pub updated: u64,
    pub error: Option<String>,
    pub direction: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Peer {
    pub id: String,
    pub name: String,
    pub os: String,
    pub host: String,
    pub port: u16,
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn manifest_id(entries: &[Entry]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(serde_json::to_vec(entries).unwrap()))
}
