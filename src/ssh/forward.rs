use crate::ssh::session::{create_russh_session, SessionConfig, SshHandler};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

// ── Типы ──

#[derive(Clone, PartialEq)]
pub enum ForwardType {
    Local,
    Remote,
    Dynamic,
}

#[derive(Clone)]
pub struct ForwardRule {
    pub forward_type: ForwardType,
    pub local_host: String,
    pub local_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
}

// ── Активное перенаправление ──

struct ActiveForward {
    rule: ForwardRule,
    alive: Arc<AtomicBool>,
    error: Arc<parking_lot::Mutex<Option<String>>>,
    conn_count: Arc<AtomicUsize>,
}

// ── Менеджер перенаправлений ──

pub struct PortForwarder {
    config: SessionConfig,
    forwards: Vec<ActiveForward>,
    // UI: диалог добавления
    show_add_dialog: bool,
    new_forward_type: usize, // 0=Local, 1=Remote
    new_local_host: String,
    new_local_port: String,
    new_remote_host: String,
    new_remote_port: String,
    // Сообщения
    status_message: Option<String>,
    error_messages: Vec<String>,
}

impl PortForwarder {
    pub fn new(config: &SessionConfig) -> Self {
        PortForwarder {
            config: config.clone(),
            forwards: Vec::new(),
            show_add_dialog: false,
            new_forward_type: 0,
            new_local_host: "127.0.0.1".to_string(),
            new_local_port: String::new(),
            new_remote_host: "localhost".to_string(),
            new_remote_port: String::new(),
            status_message: None,
            error_messages: Vec::new(),
        }
    }

    fn start_forward(&mut self, rule: ForwardRule) {
        let alive = Arc::new(AtomicBool::new(true));
        let error: Arc<parking_lot::Mutex<Option<String>>> =
            Arc::new(parking_lot::Mutex::new(None));
        let conn_count = Arc::new(AtomicUsize::new(0));

        let config = self.config.clone();
        let rule_clone = rule.clone();
        let alive_clone = alive.clone();
        let error_clone = error.clone();
        let conn_count_clone = conn_count.clone();

        std::thread::spawn(move || {
            let rt = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(e) => {
                    *error_clone.lock() = Some(format!("Tokio runtime: {}", e));
                    alive_clone.store(false, Ordering::Relaxed);
                    return;
                }
            };
            let result = match rule_clone.forward_type {
                ForwardType::Local => rt.block_on(run_local_forward_async(
                    &config,
                    &rule_clone,
                    &alive_clone,
                    &conn_count_clone,
                )),
                ForwardType::Remote => rt.block_on(run_remote_forward_async(
                    &config,
                    &rule_clone,
                    &alive_clone,
                    &conn_count_clone,
                )),
                ForwardType::Dynamic => rt.block_on(run_dynamic_forward_async(
                    &config,
                    &rule_clone,
                    &alive_clone,
                    &conn_count_clone,
                )),
            };
            if let Err(e) = result {
                *error_clone.lock() = Some(e.to_string());
            }
            alive_clone.store(false, Ordering::Relaxed);
        });

        self.forwards.push(ActiveForward {
            rule,
            alive,
            error,
            conn_count,
        });
    }

    /// Возвращает список активных SOCKS5-прокси (host, port).
    pub fn active_socks5_proxies(&self) -> Vec<(String, u16)> {
        self.forwards
            .iter()
            .filter(|f| {
                f.rule.forward_type == ForwardType::Dynamic && f.alive.load(Ordering::Relaxed)
            })
            .map(|f| (f.rule.local_host.clone(), f.rule.local_port))
            .collect()
    }

    fn stop_forward(&mut self, index: usize) {
        if let Some(fwd) = self.forwards.get(index) {
            fwd.alive.store(false, Ordering::Relaxed);
        }
    }

    // ── UI ──

    pub fn show(&mut self, ui: &mut egui::Ui) {
        // Собираем ошибки от завершившихся форвардов
        self.error_messages.clear();
        for fwd in &self.forwards {
            if !fwd.alive.load(Ordering::Relaxed) {
                if let Some(err) = fwd.error.lock().take() {
                    self.error_messages.push(err);
                }
            }
        }
        self.forwards
            .retain(|fwd| fwd.alive.load(Ordering::Relaxed));

        // Панель инструментов
        ui.horizontal(|ui| {
            if ui.button("[+ add rule]").clicked() {
                self.show_add_dialog = true;
                self.new_forward_type = 0;
                self.new_local_host = "127.0.0.1".to_string();
                self.new_local_port.clear();
                self.new_remote_host = "localhost".to_string();
                self.new_remote_port.clear();
            }
        });

        // Статус / ошибки
        if let Some(msg) = self.status_message.take() {
            ui.colored_label(crate::theme::GREEN, &msg);
        }
        for err in &self.error_messages {
            ui.colored_label(
                crate::theme::RED,
                format!("ERR: {}", err),
            );
        }

        ui.separator();

        if self.forwards.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() / 3.0);
                ui.colored_label(crate::theme::GREEN_DIM, "// no active port forwards");
                ui.add_space(8.0);
                ui.colored_label(crate::theme::GREY, "// click [+ add rule] to create one");
            });
        } else {
            // Таблица активных форвардов
            let mut stop_idx: Option<usize> = None;

            egui_extras::TableBuilder::new(ui)
                .striped(true)
                .resizable(true)
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(egui_extras::Column::auto().at_least(70.0))
                .column(egui_extras::Column::remainder().at_least(140.0))
                .column(egui_extras::Column::auto().at_least(20.0))
                .column(egui_extras::Column::remainder().at_least(140.0))
                .column(egui_extras::Column::auto().at_least(50.0))
                .column(egui_extras::Column::auto().at_least(40.0))
                .header(24.0, |mut header| {
                    header.col(|ui| { ui.strong("TYPE"); });
                    header.col(|ui| { ui.strong("LOCAL"); });
                    header.col(|ui| { ui.strong(""); });
                    header.col(|ui| { ui.strong("REMOTE"); });
                    header.col(|ui| { ui.strong("#"); });
                    header.col(|ui| { ui.strong(""); });
                })
                .body(|body| {
                    let count = self.forwards.len();
                    body.rows(24.0, count, |mut row| {
                        let idx = row.index();
                        let fwd = &self.forwards[idx];

                        row.col(|ui| {
                            let (label, color) = match fwd.rule.forward_type {
                                ForwardType::Local => {
                                    ("-L", crate::theme::GREEN)
                                }
                                ForwardType::Remote => {
                                    ("-R", crate::theme::AMBER)
                                }
                                ForwardType::Dynamic => {
                                    ("-D", crate::theme::CYAN)
                                }
                            };
                            ui.colored_label(color, label);
                        });
                        row.col(|ui| {
                            ui.monospace(format!(
                                "{}:{}",
                                fwd.rule.local_host, fwd.rule.local_port
                            ));
                        });
                        row.col(|ui| {
                            let arrow = match fwd.rule.forward_type {
                                ForwardType::Local => "->",
                                ForwardType::Remote => "<-",
                                ForwardType::Dynamic => "<>",
                            };
                            ui.label(arrow);
                        });
                        row.col(|ui| {
                            if fwd.rule.forward_type == ForwardType::Dynamic {
                                ui.colored_label(
                                    crate::theme::GREY,
                                    "*",
                                );
                            } else {
                                ui.monospace(format!(
                                    "{}:{}",
                                    fwd.rule.remote_host, fwd.rule.remote_port
                                ));
                            }
                        });
                        row.col(|ui| {
                            let n = fwd.conn_count.load(Ordering::Relaxed);
                            ui.label(format!("{}", n));
                        });
                        row.col(|ui| {
                            if ui
                                .button("[x]")
                                .on_hover_text("stop")
                                .clicked()
                            {
                                stop_idx = Some(idx);
                            }
                        });
                    });
                });

            if let Some(idx) = stop_idx {
                self.stop_forward(idx);
                self.status_message = Some("forward stopped".to_string());
            }
        }

        // Диалог добавления
        if self.show_add_dialog {
            self.render_add_dialog(ui);
        }
    }

    fn render_add_dialog(&mut self, ui: &mut egui::Ui) {
        let mut do_add = false;

        egui::Window::new("add port forward")
            .collapsible(false)
            .resizable(false)
            .default_width(400.0)
            .show(ui.ctx(), |ui| {
                egui::Grid::new("forward_dialog_grid")
                    .num_columns(2)
                    .spacing([10.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("type:");
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut self.new_forward_type, 0, "-L local");
                            ui.radio_value(&mut self.new_forward_type, 1, "-R remote");
                            ui.radio_value(&mut self.new_forward_type, 2, "-D socks5");
                        });
                        ui.end_row();

                        ui.label("bind host:");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.new_local_host)
                                .hint_text("127.0.0.1"),
                        );
                        ui.end_row();

                        ui.label("bind port:");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.new_local_port)
                                .hint_text(if self.new_forward_type == 2 {
                                    "1080"
                                } else {
                                    "8080"
                                }),
                        );
                        ui.end_row();

                        if self.new_forward_type != 2 {
                            ui.label("dest host:");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.new_remote_host)
                                    .hint_text("localhost"),
                            );
                            ui.end_row();

                            ui.label("dest port:");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.new_remote_port)
                                    .hint_text("5432"),
                            );
                            ui.end_row();
                        }
                    });

                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);

                let local_str = format!(
                    "{}:{}",
                    if self.new_local_host.is_empty() { "..." } else { &self.new_local_host },
                    if self.new_local_port.is_empty() { "..." } else { &self.new_local_port },
                );
                let remote_str = format!(
                    "{}:{}",
                    if self.new_remote_host.is_empty() { "..." } else { &self.new_remote_host },
                    if self.new_remote_port.is_empty() { "..." } else { &self.new_remote_port },
                );

                let description = match self.new_forward_type {
                    0 => format!("{} -> ssh -> {}", local_str, remote_str),
                    1 => format!("{} <- ssh <- {}", local_str, remote_str),
                    _ => format!("socks5 proxy on {}", local_str),
                };
                ui.colored_label(crate::theme::GREEN_DIM, &description);

                ui.add_space(4.0);

                ui.horizontal(|ui| {
                    let local_port_ok = self.new_local_port.parse::<u16>().is_ok();
                    let can_add = if self.new_forward_type == 2 {
                        local_port_ok && !self.new_local_host.is_empty()
                    } else {
                        let remote_port_ok = self.new_remote_port.parse::<u16>().is_ok();
                        local_port_ok
                            && remote_port_ok
                            && !self.new_local_host.is_empty()
                            && !self.new_remote_host.is_empty()
                    };

                    if ui
                        .add_enabled(can_add, egui::Button::new("[start]"))
                        .clicked()
                    {
                        do_add = true;
                    }
                    if ui.button("[cancel]").clicked() {
                        self.show_add_dialog = false;
                    }
                });
            });

        if do_add {
            let forward_type = match self.new_forward_type {
                0 => ForwardType::Local,
                1 => ForwardType::Remote,
                _ => ForwardType::Dynamic,
            };
            let rule = ForwardRule {
                forward_type,
                local_host: self.new_local_host.clone(),
                local_port: self.new_local_port.parse().unwrap_or(0),
                remote_host: self.new_remote_host.clone(),
                remote_port: self.new_remote_port.parse().unwrap_or(0),
            };
            self.start_forward(rule);
            self.show_add_dialog = false;
            self.status_message = Some("forward started".to_string());
        }
    }
}

// ── Local Port Forwarding (-L) ──

async fn run_local_forward_async(
    config: &SessionConfig,
    rule: &ForwardRule,
    alive: &AtomicBool,
    conn_count: &AtomicUsize,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let session = Arc::new(create_russh_session(config, SshHandler::new()).await?);
    let listener =
        tokio::net::TcpListener::bind(format!("{}:{}", rule.local_host, rule.local_port)).await?;

    while alive.load(Ordering::Relaxed) {
        let accept = tokio::time::timeout(std::time::Duration::from_millis(500), listener.accept())
            .await;

        match accept {
            Ok(Ok((stream, _))) => {
                let session = session.clone();
                let host = rule.remote_host.clone();
                let port = rule.remote_port;
                conn_count.fetch_add(1, Ordering::Relaxed);

                tokio::spawn(async move {
                    let _ = relay_direct_tcpip(session, stream, &host, port).await;
                });
            }
            Ok(Err(_)) => break,
            Err(_) => continue, // timeout — проверяем alive
        }
    }

    Ok(())
}

async fn relay_direct_tcpip(
    session: Arc<russh::client::Handle<SshHandler>>,
    local_stream: tokio::net::TcpStream,
    remote_host: &str,
    remote_port: u16,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let channel = session
        .channel_open_direct_tcpip(remote_host, remote_port as u32, "127.0.0.1", 0)
        .await?;

    let channel_stream = channel.into_stream();
    let (mut ch_read, mut ch_write) = tokio::io::split(channel_stream);
    let (mut tcp_read, mut tcp_write) = local_stream.into_split();

    tokio::select! {
        r = tokio::io::copy(&mut ch_read, &mut tcp_write) => { let _ = r; }
        r = tokio::io::copy(&mut tcp_read, &mut ch_write) => { let _ = r; }
    }

    Ok(())
}

// ── Remote Port Forwarding (-R) ──

async fn run_remote_forward_async(
    config: &SessionConfig,
    rule: &ForwardRule,
    alive: &AtomicBool,
    conn_count: &AtomicUsize,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut session = create_russh_session(config, SshHandler::with_forwarded_tx(tx)).await?;

    // Запрашиваем remote forwarding у SSH-сервера
    session
        .tcpip_forward(&rule.remote_host, rule.remote_port as u32)
        .await?;

    let local_host = rule.local_host.clone();
    let local_port = rule.local_port;

    while alive.load(Ordering::Relaxed) {
        let channel_opt = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv())
            .await;

        match channel_opt {
            Ok(Some(channel)) => {
                let host = local_host.clone();
                conn_count.fetch_add(1, Ordering::Relaxed);

                tokio::spawn(async move {
                    let _ = relay_forwarded_channel(channel, &host, local_port).await;
                });
            }
            Ok(None) => break,
            Err(_) => continue,
        }
    }

    // Отменяем remote forwarding
    let _ = session
        .cancel_tcpip_forward(&rule.remote_host, rule.remote_port as u32)
        .await;

    Ok(())
}

async fn relay_forwarded_channel(
    channel: russh::Channel<russh::client::Msg>,
    local_host: &str,
    local_port: u16,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let local_stream =
        tokio::net::TcpStream::connect(format!("{}:{}", local_host, local_port)).await?;

    let channel_stream = channel.into_stream();
    let (mut ch_read, mut ch_write) = tokio::io::split(channel_stream);
    let (mut tcp_read, mut tcp_write) = local_stream.into_split();

    tokio::select! {
        r = tokio::io::copy(&mut ch_read, &mut tcp_write) => { let _ = r; }
        r = tokio::io::copy(&mut tcp_read, &mut ch_write) => { let _ = r; }
    }

    Ok(())
}

// ── Dynamic Port Forwarding / SOCKS5 (-D) ──

async fn run_dynamic_forward_async(
    config: &SessionConfig,
    rule: &ForwardRule,
    alive: &AtomicBool,
    conn_count: &AtomicUsize,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let session = Arc::new(create_russh_session(config, SshHandler::new()).await?);
    let listener =
        tokio::net::TcpListener::bind(format!("{}:{}", rule.local_host, rule.local_port)).await?;

    while alive.load(Ordering::Relaxed) {
        let accept = tokio::time::timeout(std::time::Duration::from_millis(500), listener.accept())
            .await;

        match accept {
            Ok(Ok((stream, _))) => {
                let session = session.clone();
                conn_count.fetch_add(1, Ordering::Relaxed);

                tokio::spawn(async move {
                    let _ = handle_socks5_client(session, stream).await;
                });
            }
            Ok(Err(_)) => break,
            Err(_) => continue,
        }
    }

    Ok(())
}

/// SOCKS5 рукопожатие + relay через SSH direct-tcpip.
async fn handle_socks5_client(
    session: Arc<russh::client::Handle<SshHandler>>,
    mut stream: tokio::net::TcpStream,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // 1. Greeting
    let mut header = [0u8; 2];
    stream.read_exact(&mut header).await?;
    if header[0] != 0x05 {
        return Err("Не SOCKS5".into());
    }

    let nmethods = header[1] as usize;
    let mut methods = vec![0u8; nmethods];
    stream.read_exact(&mut methods).await?;

    if !methods.contains(&0x00) {
        stream.write_all(&[0x05, 0xFF]).await?;
        return Err("Нет подходящего метода аутентификации".into());
    }
    stream.write_all(&[0x05, 0x00]).await?;

    // 2. Request
    let mut req_header = [0u8; 4];
    stream.read_exact(&mut req_header).await?;
    if req_header[0] != 0x05 || req_header[1] != 0x01 {
        let reply = [0x05, 0x07, 0x00, 0x01, 0, 0, 0, 0, 0, 0];
        stream.write_all(&reply).await?;
        return Err("Команда не поддерживается (только CONNECT)".into());
    }

    let (dest_host, dest_port) = match req_header[3] {
        0x01 => {
            let mut addr = [0u8; 4];
            stream.read_exact(&mut addr).await?;
            let mut port_buf = [0u8; 2];
            stream.read_exact(&mut port_buf).await?;
            (
                format!("{}.{}.{}.{}", addr[0], addr[1], addr[2], addr[3]),
                u16::from_be_bytes(port_buf),
            )
        }
        0x03 => {
            let mut len_buf = [0u8; 1];
            stream.read_exact(&mut len_buf).await?;
            let mut domain = vec![0u8; len_buf[0] as usize];
            stream.read_exact(&mut domain).await?;
            let mut port_buf = [0u8; 2];
            stream.read_exact(&mut port_buf).await?;
            (
                String::from_utf8_lossy(&domain).to_string(),
                u16::from_be_bytes(port_buf),
            )
        }
        0x04 => {
            let mut addr = [0u8; 16];
            stream.read_exact(&mut addr).await?;
            let mut port_buf = [0u8; 2];
            stream.read_exact(&mut port_buf).await?;
            let segments: Vec<String> = (0..8)
                .map(|i| format!("{:x}", u16::from_be_bytes([addr[i * 2], addr[i * 2 + 1]])))
                .collect();
            (segments.join(":"), u16::from_be_bytes(port_buf))
        }
        _ => {
            stream
                .write_all(&[0x05, 0x08, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                .await?;
            return Err("Неподдерживаемый тип адреса".into());
        }
    };

    // 3. Открываем SSH-канал до целевого хоста
    let channel = match session
        .channel_open_direct_tcpip(&dest_host, dest_port as u32, "127.0.0.1", 0)
        .await
    {
        Ok(ch) => ch,
        Err(e) => {
            stream
                .write_all(&[0x05, 0x05, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                .await?;
            return Err(format!(
                "SSH direct-tcpip к {}:{} не удался: {}",
                dest_host, dest_port, e
            )
            .into());
        }
    };

    // 4. Ответ: успех
    stream
        .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
        .await?;

    // 5. Relay данных
    let channel_stream = channel.into_stream();
    let (mut ch_read, mut ch_write) = tokio::io::split(channel_stream);
    let (mut tcp_read, mut tcp_write) = stream.into_split();

    tokio::select! {
        _ = tokio::io::copy(&mut ch_read, &mut tcp_write) => {}
        _ = tokio::io::copy(&mut tcp_read, &mut ch_write) => {}
    }

    Ok(())
}
