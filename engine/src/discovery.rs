use crate::model::Peer;
use anyhow::Result;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

pub struct Discovery {
    daemon: ServiceDaemon,
    fullname: String,
    pub peers: Arc<Mutex<HashMap<String, Peer>>>,
}

impl Discovery {
    pub fn start(id: &str, name: &str, host: &str, port: u16) -> Result<Self> {
        let daemon = ServiceDaemon::new()?;
        let properties = [
            ("name", name),
            ("os", std::env::consts::OS),
            ("id", id),
            ("v", "1"),
        ];
        // DNS labels stop at 63 bytes; a fingerprint is 64 hex characters. Its
        // first 128 bits name the service, and the TXT record carries it whole.
        let label = &id[..id.len().min(32)];
        let info = ServiceInfo::new(
            "_drift._tcp.local.",
            label,
            &format!("drift-{label}.local."),
            host,
            port,
            &properties[..],
        )?;
        let fullname = info.get_fullname().to_string();
        daemon.register(info)?;
        let receiver = daemon.browse("_drift._tcp.local.")?;
        let peers = Arc::new(Mutex::new(HashMap::new()));
        let output = peers.clone();
        let own = id.to_owned();
        std::thread::spawn(move || {
            while let Ok(event) = receiver.recv() {
                match event {
                    ServiceEvent::ServiceResolved(info) => {
                        let id = info.get_property_val_str("id").unwrap_or("");
                        if id == own || id.is_empty() {
                            continue;
                        }
                        if let Some(host) = info.get_addresses().iter().find(|a| a.is_ipv4()) {
                            let mut peers = output.lock().unwrap();
                            if peers.len() >= 128 && !peers.contains_key(info.get_fullname()) {
                                continue;
                            }
                            peers.insert(
                                info.get_fullname().to_owned(),
                                Peer {
                                    id: id.chars().take(100).collect(),
                                    name: info
                                        .get_property_val_str("name")
                                        .unwrap_or("Nearby device")
                                        .chars()
                                        .take(100)
                                        .collect(),
                                    os: info
                                        .get_property_val_str("os")
                                        .unwrap_or("unknown")
                                        .chars()
                                        .take(40)
                                        .collect(),
                                    host: host.to_string(),
                                    port: info.get_port(),
                                },
                            );
                        }
                    }
                    ServiceEvent::ServiceRemoved(_, fullname) => {
                        output.lock().unwrap().remove(&fullname);
                    }
                    _ => {}
                }
            }
        });
        Ok(Self {
            daemon,
            fullname,
            peers,
        })
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}
