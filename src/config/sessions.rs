use crate::ssh::session::{AuthType, ProxyConfig, SessionConfig};
use std::path::PathBuf;

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct StoredSessions {
    sessions: Vec<StoredSession>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct StoredSession {
    id: String,
    name: String,
    host: String,
    port: u16,
    username: String,
    auth_type: StoredAuthType,
    #[serde(default)]
    proxy_host: Option<String>,
    #[serde(default)]
    proxy_port: Option<u16>,
}

#[derive(serde::Serialize, serde::Deserialize)]
enum StoredAuthType {
    Password,
    KeyFile(String),
    Agent,
}

fn config_path() -> PathBuf {
    let dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ssherald");
    std::fs::create_dir_all(&dir).ok();
    dir.join("sessions.json")
}

pub fn load_sessions() -> Vec<SessionConfig> {
    let path = config_path();
    let data = match std::fs::read_to_string(&path) {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };
    let stored: StoredSessions = match serde_json::from_str(&data) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    stored
        .sessions
        .into_iter()
        .map(|s| {
            let auth_type = match s.auth_type {
                StoredAuthType::Password => AuthType::Password(String::new()),
                StoredAuthType::KeyFile(path) => AuthType::KeyFile(path),
                StoredAuthType::Agent => AuthType::Agent,
            };
            let proxy = match (s.proxy_host, s.proxy_port) {
                (Some(host), Some(port)) if !host.is_empty() => {
                    Some(ProxyConfig { host, port })
                }
                _ => None,
            };
            SessionConfig {
                id: s.id,
                name: s.name,
                host: s.host,
                port: s.port,
                username: s.username,
                auth_type,
                proxy,
            }
        })
        .collect()
}

pub fn save_sessions(sessions: &[SessionConfig]) {
    let stored = StoredSessions {
        sessions: sessions
            .iter()
            .map(|s| {
                let auth_type = match &s.auth_type {
                    AuthType::Password(_) => StoredAuthType::Password,
                    AuthType::KeyFile(path) => StoredAuthType::KeyFile(path.clone()),
                    AuthType::Agent => StoredAuthType::Agent,
                };
                let (proxy_host, proxy_port) = match &s.proxy {
                    Some(p) => (Some(p.host.clone()), Some(p.port)),
                    None => (None, None),
                };
                StoredSession {
                    id: s.id.clone(),
                    name: s.name.clone(),
                    host: s.host.clone(),
                    port: s.port,
                    username: s.username.clone(),
                    auth_type,
                    proxy_host,
                    proxy_port,
                }
            })
            .collect(),
    };

    let path = config_path();
    if let Ok(json) = serde_json::to_string_pretty(&stored) {
        let _ = std::fs::write(path, json);
    }
}
