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
