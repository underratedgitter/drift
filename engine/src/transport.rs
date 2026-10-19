use crate::{
    model::{Invitation, PairRequest},
    pairing::{proof, secret, verify},
};
use anyhow::{Context, Result, ensure};
use reqwest::{Certificate, Client, Response};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::time::Duration;

#[derive(Clone)]
pub struct Connection {
    client: Client,
    base: String,
    pub session: String,
    pub name: String,
    pub confirmed: bool,
    capability: String,
}

#[derive(Serialize, Deserialize)]
pub struct PairReply {
    pub session: String,
    pub capability: String,
    pub name: String,
}

#[derive(Serialize, Deserialize)]
pub struct ProofReply {
    pub proof: String,
}

async fn checked(response: Response) -> Result<Response> {
    if !response.status().is_success() {
        let status = response.status();
        let bytes = bounded_body(response).await?;
        anyhow::bail!(
            "Peer returned {status}: {}",
            String::from_utf8_lossy(&bytes[..bytes.len().min(500)])
        );
    }
    Ok(response)
}

async fn bounded_body(mut response: Response) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= crate::model::MAX_METADATA,
            "Peer response exceeds metadata limit"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn decode<T: DeserializeOwned>(response: Response) -> Result<T> {
    Ok(serde_json::from_slice(
        &bounded_body(checked(response).await?).await?,
    )?)
}

impl Connection {
    pub(crate) async fn window_size(&self) -> Result<usize> {
        let response = self
            .client
            .get(format!("{}/features", self.base))
            .bearer_auth(&self.capability)
            .send()
            .await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(crate::model::CHUNK);
        }
        #[derive(Deserialize)]
        struct Features {
            version: u8,
            max_window_bytes: usize,
            durable_window_ack: bool,
        }
        let features: Features = decode(response).await?;
        ensure!(
            features.version == 1 && features.durable_window_ack,
            "Unsupported peer durability contract"
        );
        // Never allocate more than our own fixed budget based on peer metadata.
        Ok(if features.max_window_bytes >= crate::model::WINDOW {
            crate::model::WINDOW
        } else {
            crate::model::CHUNK
        })
    }

    pub async fn pair(invitation: &Invitation, name: &str) -> Result<Self> {
        ensure!(
            invitation.version == 1 && invitation.port > 0,
            "Unsupported invitation"
        );
        ensure!(
            invitation.code.len() == 30 && hex::decode(&invitation.code).is_ok(),
            "Pairing code must contain 30 hexadecimal characters"
        );
        let ip: std::net::Ipv4Addr = invitation
            .host
            .parse()
            .context("Use the device's local IPv4 address")?;
        ensure!(
            ip.is_private() || ip.is_loopback() || ip.is_link_local(),
            "LAN mode accepts local addresses only"
        );
        let base = format!("https://{}:{}", invitation.host, invitation.port);
        let bootstrap = Client::builder()
            .danger_accept_invalid_certs(true)
            .tls_info(true)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(Duration::from_secs(15))
            .build()?;
        let nonce = secret(32);
        let response = checked(
            bootstrap
                .get(format!("{base}/proof?nonce={nonce}"))
                .send()
                .await?,
        )
        .await?;
        let der = response
            .extensions()
            .get::<reqwest::tls::TlsInfo>()
            .and_then(|i| i.peer_certificate())
            .context("TLS certificate missing")?
            .to_vec();
        let fingerprint = hex::encode(Sha256::digest(&der));
        let reply: ProofReply = decode(response).await?;
        verify(
            &invitation.code,
            "server",
            &fingerprint,
            &nonce,
            "",
            &reply.proof,
        )
        .context("Pairing authentication failed. Check the code and device address.")?;
        // From here onward, only this authenticated certificate is trusted.
        let client = Client::builder()
            .tls_built_in_root_certs(false)
            .add_root_certificate(Certificate::from_der(&der)?)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(Duration::from_secs(30))
            .build()?;
        let request = PairRequest {
            nonce: nonce.clone(),
            name: name.to_owned(),
            proof: proof(&invitation.code, "client", &fingerprint, &nonce, name),
        };
        let reply: PairReply = decode(
            client
                .post(format!("{base}/pair"))
                .json(&request)
                .send()
                .await?,
        )
        .await?;
        Ok(Self {
            client,
            base,
            session: reply.session,
            capability: reply.capability,
            name: reply.name,
            confirmed: false,
        })
    }

    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        decode(
            self.client
                .get(format!("{}{path}", self.base))
                .bearer_auth(&self.capability)
                .send()
                .await?,
        )
        .await
    }

    pub async fn post<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T> {
        decode(
            self.client
                .post(format!("{}{path}", self.base))
                .bearer_auth(&self.capability)
                .json(body)
                .send()
                .await?,
        )
        .await
    }

    pub async fn chunk(
        &self,
        id: &str,
        index: usize,
        offset: u64,
        bytes: impl Into<axum::body::Bytes>,
    ) -> Result<()> {
        let bytes = bytes.into();
        self.upload(id, index, offset, bytes, false).await
    }

    pub async fn window_chunk(
        &self,
        id: &str,
        index: usize,
        offset: u64,
        bytes: axum::body::Bytes,
    ) -> Result<()> {
        self.upload(id, index, offset, bytes, true).await
    }

    async fn upload(
        &self,
        id: &str,
        index: usize,
        offset: u64,
        bytes: axum::body::Bytes,
        window: bool,
    ) -> Result<()> {
        let expected = offset + bytes.len() as u64;
        let suffix = if window { "/window" } else { "" };
        let acknowledgement: serde_json::Value = decode(
            self.client
                .put(format!(
                    "{}/transfer/{id}/{index}{suffix}?offset={offset}",
                    self.base
                ))
                .bearer_auth(&self.capability)
                .timeout(Duration::from_secs(if window { 60 } else { 30 }))
                .body(bytes)
                .send()
                .await?,
        )
        .await?;
        ensure!(
            acknowledgement["offset"].as_u64() == Some(expected),
            "Invalid chunk acknowledgement"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn window_negotiation_is_bounded_and_falls_back_only_for_legacy_peers() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        use axum::{Json, Router, http::StatusCode, routing::get};
        for (status, body, expected) in [
            (
                StatusCode::NOT_FOUND,
                serde_json::json!({}),
                Some(crate::model::CHUNK),
            ),
            (
                StatusCode::OK,
                serde_json::json!({"version":1,"durable_window_ack":true,"max_window_bytes":usize::MAX}),
                Some(crate::model::WINDOW),
            ),
            (
                StatusCode::OK,
                serde_json::json!({"version":1,"durable_window_ack":true,"max_window_bytes":crate::model::CHUNK}),
                Some(crate::model::CHUNK),
            ),
            (
                StatusCode::OK,
                serde_json::json!({"version":1,"durable_window_ack":false,"max_window_bytes":crate::model::WINDOW}),
                None,
            ),
            (StatusCode::UNAUTHORIZED, serde_json::json!({}), None),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let router = Router::new().route(
                "/features",
                get(move || {
                    let body = body.clone();
                    async move { (status, Json(body)) }
                }),
            );
            let task = tokio::spawn(async move {
                axum::serve(listener, router).await.unwrap();
            });
            // Test-only HTTP fixture for metadata handling, not a production TLS bypass.
            let connection = Connection {
                client: Client::builder().no_proxy().build().unwrap(),
                base: format!("http://{address}"),
                session: String::new(),
                name: "test".into(),
                confirmed: true,
                capability: "test-only".into(),
            };
            let result = connection.window_size().await;
            match expected {
                Some(size) => assert_eq!(result.unwrap(), size),
                None => assert!(result.is_err()),
            }
            task.abort();
        }
    }
}
