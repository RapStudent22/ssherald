use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;

use russh::client;
use russh::keys::{self, PrivateKeyWithHashAlg};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct ProxyConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_type: AuthType,
    #[serde(default)]
    pub proxy: Option<ProxyConfig>,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum AuthType {
    Password(String),
    KeyFile(String),
    Agent,
}

pub enum SshCommand {
    Data(Vec<u8>),
    Resize { cols: u32, rows: u32 },
}

pub struct SshConnection {
    pub input_tx: mpsc::Sender<SshCommand>,
    pub output_rx: mpsc::Receiver<Vec<u8>>,
    pub alive: Arc<AtomicBool>,
    pub error: Arc<parking_lot::Mutex<Option<String>>>,
}

// ── russh client handler ──

pub struct SshHandler {
    /// Канал для forwarded-tcpip (Remote Port Forward).
    /// None для обычных shell/sftp/local/dynamic соединений.
    pub forwarded_tx:
        Option<tokio::sync::mpsc::UnboundedSender<russh::Channel<russh::client::Msg>>>,
}

impl SshHandler {
    pub fn new() -> Self {
        SshHandler {
            forwarded_tx: None,
        }
    }

    pub fn with_forwarded_tx(
        tx: tokio::sync::mpsc::UnboundedSender<russh::Channel<russh::client::Msg>>,
    ) -> Self {
        SshHandler {
            forwarded_tx: Some(tx),
        }
    }
}

impl client::Handler for SshHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &keys::PublicKey,
    ) -> Result<bool, Self::Error> {
        Ok(true) // Принимаем все ключи сервера
    }

    fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: russh::Channel<russh::client::Msg>,
        _connected_address: &str,
        _connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        _session: &mut client::Session,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send {
        if let Some(tx) = &self.forwarded_tx {
            let _ = tx.send(channel);
        }
        async { Ok(()) }
    }
}

// ── SshConnection — публичный интерфейс (не меняется) ──

impl SshConnection {
    pub fn new(config: &SessionConfig) -> Self {
        let (input_tx, input_rx) = mpsc::channel::<SshCommand>();
        let (output_tx, output_rx) = mpsc::channel::<Vec<u8>>();
        let alive = Arc::new(AtomicBool::new(true));
        let error: Arc<parking_lot::Mutex<Option<String>>> =
            Arc::new(parking_lot::Mutex::new(None));

        let config = config.clone();
        let alive_clone = alive.clone();
        let error_clone = error.clone();

        std::thread::spawn(move || {
            let rt = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(e) => {
                    *error_clone.lock() = Some(format!("Не удалось создать tokio runtime: {}", e));
                    alive_clone.store(false, Ordering::Relaxed);
                    return;
                }
            };
            if let Err(e) =
                rt.block_on(run_session_async(&config, input_rx, output_tx, &alive_clone))
            {
                *error_clone.lock() = Some(e.to_string());
            }
            alive_clone.store(false, Ordering::Relaxed);
        });

        SshConnection {
            input_tx,
            output_rx,
            alive,
            error,
        }
    }

    pub fn send(&self, data: &[u8]) {
        let _ = self.input_tx.send(SshCommand::Data(data.to_vec()));
    }

    pub fn resize(&self, cols: u32, rows: u32) {
        let _ = self.input_tx.send(SshCommand::Resize { cols, rows });
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    pub fn take_error(&self) -> Option<String> {
        self.error.lock().take()
    }
}

impl Drop for SshConnection {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Relaxed);
    }
}

// ── Основной async-цикл SSH-сессии ──

async fn run_session_async(
    config: &SessionConfig,
    input_rx: mpsc::Receiver<SshCommand>,
    output_tx: mpsc::Sender<Vec<u8>>,
    alive: &AtomicBool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let session = create_russh_session(config, SshHandler::new()).await?;

    let mut channel = session.channel_open_session().await?;
    channel
        .request_pty(true, "xterm-256color", 80, 24, 0, 0, &[])
        .await?;
    channel.request_shell(true).await?;

    loop {
        if !alive.load(Ordering::Relaxed) {
            break;
        }

        tokio::select! {
            msg = channel.wait() => {
                match msg {
                    Some(russh::ChannelMsg::Data { ref data }) => {
                        if output_tx.send(data.to_vec()).is_err() {
                            break;
                        }
                    }
                    Some(russh::ChannelMsg::ExtendedData { ref data, .. }) => {
                        let _ = output_tx.send(data.to_vec());
                    }
                    Some(russh::ChannelMsg::Eof)
                    | Some(russh::ChannelMsg::Close)
                    | None => {
                        break;
                    }
                    _ => {}
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(5)) => {
                while let Ok(cmd) = input_rx.try_recv() {
                    match cmd {
                        SshCommand::Data(data) => {
                            channel.data(&data[..]).await?;
                        }
                        SshCommand::Resize { cols, rows } => {
                            channel.window_change(cols, rows, 0, 0).await?;
                        }
                    }
                }
            }
        }
    }

    let _ = channel.close().await;
    let _ = session
        .disconnect(russh::Disconnect::ByApplication, "", "")
        .await;

    Ok(())
}

// ── Создание аутентифицированной russh-сессии ──
// Используется всеми модулями: shell, sftp, forward.

pub async fn create_russh_session(
    config: &SessionConfig,
    handler: SshHandler,
) -> Result<client::Handle<SshHandler>, Box<dyn std::error::Error + Send + Sync>> {
    let ssh_config = Arc::new(client::Config::default());

    let mut session = match &config.proxy {
        Some(proxy) => {
            let tcp = connect_tcp_async(&proxy.host, proxy.port).await?;
            let tcp = socks5_connect_async(tcp, &config.host, config.port).await?;
            client::connect_stream(ssh_config, tcp, handler).await?
        }
        None => {
            let addr = format!("{}:{}", config.host, config.port);
            client::connect(ssh_config, &*addr, handler).await?
        }
    };

    // Аутентификация
    match &config.auth_type {
        AuthType::Password(pwd) => {
            let auth = session
                .authenticate_password(&config.username, pwd)
                .await?;
            if !matches!(auth, client::AuthResult::Success) {
                return Err("Аутентификация не удалась".into());
            }
        }
        AuthType::KeyFile(path) => {
            let expanded = expand_tilde(path);
            let key = keys::load_secret_key(&expanded, None)
                .map_err(|e| format!("Ошибка загрузки ключа {}: {}", expanded, e))?;
            let key_with_alg = PrivateKeyWithHashAlg::new(Arc::new(key), None);
            let auth = session
                .authenticate_publickey(&config.username, key_with_alg)
                .await?;
            if !matches!(auth, client::AuthResult::Success) {
                return Err("Аутентификация по ключу не удалась".into());
            }
        }
        AuthType::Agent => {
            return Err("SSH Agent пока не поддерживается (будет добавлено позже)".into());
        }
    }

    Ok(session)
}

// ── Вспомогательные функции ──

fn expand_tilde(path: &str) -> String {
    if path.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(&path[2..]).to_string_lossy().to_string();
        }
    }
    path.to_string()
}

async fn connect_tcp_async(
    host: &str,
    port: u16,
) -> Result<tokio::net::TcpStream, Box<dyn std::error::Error + Send + Sync>> {
    let addr = format!("{}:{}", host, port);
    Ok(tokio::net::TcpStream::connect(&addr).await?)
}

/// SOCKS5 CONNECT через уже установленное TCP-соединение с прокси.
async fn socks5_connect_async(
    mut stream: tokio::net::TcpStream,
    target_host: &str,
    target_port: u16,
) -> Result<tokio::net::TcpStream, Box<dyn std::error::Error + Send + Sync>> {
    // Greeting: version 5, 1 method (no auth)
    stream.write_all(&[0x05, 0x01, 0x00]).await?;
    let mut resp = [0u8; 2];
    stream.read_exact(&mut resp).await?;
    if resp[0] != 0x05 || resp[1] != 0x00 {
        return Err("SOCKS5: прокси отверг метод аутентификации".into());
    }

    // CONNECT request
    let mut request = vec![0x05, 0x01, 0x00]; // ver, cmd=CONNECT, rsv

    if let Ok(ipv4) = target_host.parse::<std::net::Ipv4Addr>() {
        request.push(0x01); // IPv4
        request.extend_from_slice(&ipv4.octets());
    } else {
        // Domain name
        let host_bytes = target_host.as_bytes();
        if host_bytes.len() > 255 {
            return Err("SOCKS5: имя хоста слишком длинное".into());
        }
        request.push(0x03);
        request.push(host_bytes.len() as u8);
        request.extend_from_slice(host_bytes);
    }

    request.extend_from_slice(&target_port.to_be_bytes());
    stream.write_all(&request).await?;

    // Response
    let mut resp_header = [0u8; 4];
    stream.read_exact(&mut resp_header).await?;
    if resp_header[1] != 0x00 {
        return Err(
            format!("SOCKS5: подключение не удалось (код {})", resp_header[1]).into(),
        );
    }

    // Skip bound address
    match resp_header[3] {
        0x01 => {
            let mut buf = [0u8; 6];
            stream.read_exact(&mut buf).await?;
        }
        0x03 => {
            let mut len = [0u8; 1];
            stream.read_exact(&mut len).await?;
            let mut buf = vec![0u8; len[0] as usize + 2];
            stream.read_exact(&mut buf).await?;
        }
        0x04 => {
            let mut buf = [0u8; 18];
            stream.read_exact(&mut buf).await?;
        }
        _ => {}
    }

    Ok(stream)
}
