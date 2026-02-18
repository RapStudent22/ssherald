use std::collections::HashMap;

use crate::config::sessions as config;
use crate::ssh::forward::PortForwarder;
use crate::ssh::session::{AuthType, ProxyConfig, SessionConfig, SshConnection};
use crate::ssh::sftp::SftpBrowser;
use crate::terminal::widget::TerminalWidget;

pub struct AppState {
    sessions: Vec<SessionConfig>,
    active_session_id: Option<String>,
    connections: HashMap<String, Connection>,
    show_session_dialog: bool,
    dialog: SessionDialog,
    dialog_focus_needed: bool,
    show_connect_dialog: bool,
    connect_dialog: ConnectDialog,
    last_error: Option<String>,
}

struct Connection {
    config: SessionConfig, // конфиг с паролем — живёт только пока есть соединение
    terminal: TerminalWidget,
    ssh: SshConnection,
    sftp: Option<SftpBrowser>,
    forward: Option<PortForwarder>,
    active_tab: Tab,
    error: Option<String>,
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Shell,
    Sftp,
    Forward,
}

enum DialogAction {
    None,
    Save,
    SaveAndConnect,
}

struct SessionDialog {
    name: String,
    host: String,
    port: String,
    username: String,
    password: String,
    key_path: String,
    auth_choice: usize, // 0=Password, 1=KeyFile, 2=Agent
    editing_id: Option<String>,
    // Прокси
    proxy_enabled: bool,
    proxy_host: String,
    proxy_port: String,
}

impl Default for SessionDialog {
    fn default() -> Self {
        SessionDialog {
            name: String::new(),
            host: String::new(),
            port: "22".to_string(),
            username: String::new(),
            password: String::new(),
            key_path: String::new(),
            auth_choice: 0,
            editing_id: None,
            proxy_enabled: false,
            proxy_host: "127.0.0.1".to_string(),
            proxy_port: String::new(),
        }
    }
}

struct ConnectDialog {
    session_id: String,
    password: String,
}

impl Default for ConnectDialog {
    fn default() -> Self {
        ConnectDialog {
            session_id: String::new(),
            password: String::new(),
        }
    }
}

impl AppState {
    pub fn new(cc: &eframe::CreationContext) -> Self {
        crate::theme::apply(&cc.egui_ctx);
        let sessions = config::load_sessions();

        AppState {
            sessions,
            active_session_id: None,
            connections: HashMap::new(),
            show_session_dialog: false,
            dialog: SessionDialog::default(),
            dialog_focus_needed: false,
            show_connect_dialog: false,
            connect_dialog: ConnectDialog::default(),
            last_error: None,
        }
    }

    /// Подключиться к сессии (конфиг уже содержит пароль / ключ).
    fn connect_session(&mut self, config: &SessionConfig) {
        let ssh = SshConnection::new(config);
        let terminal = TerminalWidget::new(80, 24);

        let connection = Connection {
            config: config.clone(),
            terminal,
            ssh,
            sftp: None,
            forward: None,
            active_tab: Tab::Shell,
            error: None,
        };

        self.connections.insert(config.id.clone(), connection);
        self.active_session_id = Some(config.id.clone());
        self.last_error = None;
    }

    fn disconnect_session(&mut self, session_id: &str) {
        self.connections.remove(session_id);
        if self.active_session_id.as_deref() == Some(session_id) {
            self.active_session_id = None;
        }
    }

    /// Инициировать подключение: для пароля — показать диалог, для ключа/агента — сразу.
    fn try_connect(&mut self, session_id: &str) {
        let session = match self.sessions.iter().find(|s| s.id == session_id).cloned() {
            Some(s) => s,
            None => return,
        };

        self.last_error = None;

        match &session.auth_type {
            AuthType::Password(_) => {
                self.connect_dialog = ConnectDialog {
                    session_id: session.id.clone(),
                    password: String::new(),
                };
                self.show_connect_dialog = true;
                self.dialog_focus_needed = true;
                self.active_session_id = Some(session.id.clone());
            }
            AuthType::KeyFile(_) | AuthType::Agent => {
                self.connect_session(&session);
            }
        }
    }

    fn save_session_from_dialog(&mut self) {
        let port: u16 = self.dialog.port.parse().unwrap_or(22);
        let auth_type = match self.dialog.auth_choice {
            0 => AuthType::Password(String::new()), // Пароль не сохраняется
            1 => AuthType::KeyFile(self.dialog.key_path.clone()),
            2 => AuthType::Agent,
            _ => AuthType::Password(String::new()),
        };
        let proxy = if self.dialog.proxy_enabled {
            Some(ProxyConfig {
                host: self.dialog.proxy_host.clone(),
                port: self.dialog.proxy_port.parse().unwrap_or(1080),
            })
        } else {
            None
        };

        if let Some(id) = &self.dialog.editing_id.clone() {
            if let Some(session) = self.sessions.iter_mut().find(|s| &s.id == id) {
                session.name = self.dialog.name.clone();
                session.host = self.dialog.host.clone();
                session.port = port;
                session.username = self.dialog.username.clone();
                session.auth_type = auth_type;
                session.proxy = proxy;
            }
        } else {
            let session = SessionConfig {
                id: uuid::Uuid::new_v4().to_string(),
                name: self.dialog.name.clone(),
                host: self.dialog.host.clone(),
                port,
                username: self.dialog.username.clone(),
                auth_type,
                proxy,
            };
            self.sessions.push(session);
        }

        config::save_sessions(&self.sessions);
        self.show_session_dialog = false;
        self.dialog = SessionDialog::default();
    }

    // ── Левая панель: список сессий ──

    fn render_sessions_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("sessions_panel")
            .default_width(220.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("[ SSHerald ]")
                            .color(crate::theme::GREEN_BRIGHT)
                            .strong(),
                    );
                });
                ui.separator();

                let mut connect_id: Option<String> = None;
                let mut disconnect_id: Option<String> = None;
                let mut delete_id: Option<String> = None;
                let mut edit_session: Option<SessionConfig> = None;

                egui::ScrollArea::vertical().show(ui, |ui| {
                    for session in &self.sessions {
                        let is_connected = self.connections.contains_key(&session.id);
                        let is_active = self.active_session_id.as_ref() == Some(&session.id);

                        let row_width = ui.available_width();
                        let row_height = if is_active { 30.0 } else { 26.0 };
                        let (rect, button) = ui.allocate_exact_size(
                            egui::vec2(row_width, row_height),
                            egui::Sense::click(),
                        );

                        if is_active {
                            ui.painter().rect_filled(
                                rect,
                                0.0,
                                crate::theme::BG_ACTIVE,
                            );
                            let bar = egui::Rect::from_min_max(
                                rect.min,
                                egui::pos2(rect.min.x + 2.0, rect.max.y),
                            );
                            ui.painter().rect_filled(bar, 0.0, crate::theme::GREEN);
                        } else if button.hovered() {
                            ui.painter().rect_filled(
                                rect,
                                0.0,
                                crate::theme::BG_HOVER,
                            );
                        }

                        let text_left = rect.min.x + 8.0;
                        let text_color = if is_active {
                            crate::theme::GREEN_BRIGHT
                        } else {
                            crate::theme::GREEN_DIM
                        };
                        let font = egui::FontId::monospace(13.0);

                        // Status prefix
                        let prefix = if is_connected { "> " } else { "  " };
                        ui.painter().text(
                            egui::pos2(text_left, rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            &format!("{}{}", prefix, session.name),
                            font,
                            text_color,
                        );

                        // Status indicator text
                        let (status_text, status_color) = if is_connected {
                            ("ON", crate::theme::GREEN)
                        } else {
                            ("--", crate::theme::GREY)
                        };
                        ui.painter().text(
                            egui::pos2(rect.max.x - 8.0, rect.center().y),
                            egui::Align2::RIGHT_CENTER,
                            status_text,
                            egui::FontId::monospace(10.0),
                            status_color,
                        );

                        if button.clicked() {
                            if is_connected {
                                self.active_session_id = Some(session.id.clone());
                            } else {
                                connect_id = Some(session.id.clone());
                            }
                        }

                        button.context_menu(|ui| {
                            if !is_connected {
                                if ui.button("[connect]").clicked() {
                                    connect_id = Some(session.id.clone());
                                    ui.close_menu();
                                }
                            } else {
                                if ui.button("[disconnect]").clicked() {
                                    disconnect_id = Some(session.id.clone());
                                    ui.close_menu();
                                }
                            }
                            if ui.button("[edit]").clicked() {
                                edit_session = Some(session.clone());
                                ui.close_menu();
                            }
                            ui.separator();
                            if ui.button("[delete]").clicked() {
                                delete_id = Some(session.id.clone());
                                if is_connected {
                                    disconnect_id = Some(session.id.clone());
                                }
                                ui.close_menu();
                            }
                        });
                    }
                });

                // Отложенные действия
                if let Some(id) = connect_id {
                    self.try_connect(&id);
                }
                if let Some(id) = disconnect_id {
                    self.disconnect_session(&id);
                }
                if let Some(id) = delete_id {
                    self.sessions.retain(|s| s.id != id);
                    config::save_sessions(&self.sessions);
                }
                if let Some(session) = edit_session {
                    self.dialog = SessionDialog {
                        name: session.name.clone(),
                        host: session.host.clone(),
                        port: session.port.to_string(),
                        username: session.username.clone(),
                        password: String::new(),
                        key_path: match &session.auth_type {
                            AuthType::KeyFile(p) => p.clone(),
                            _ => String::new(),
                        },
                        auth_choice: match &session.auth_type {
                            AuthType::Password(_) => 0,
                            AuthType::KeyFile(_) => 1,
                            AuthType::Agent => 2,
                        },
                        editing_id: Some(session.id.clone()),
                        proxy_enabled: session.proxy.is_some(),
                        proxy_host: session
                            .proxy
                            .as_ref()
                            .map(|p| p.host.clone())
                            .unwrap_or_else(|| "127.0.0.1".to_string()),
                        proxy_port: session
                            .proxy
                            .as_ref()
                            .map(|p| p.port.to_string())
                            .unwrap_or_default(),
                    };
                    self.show_session_dialog = true;
                    self.dialog_focus_needed = true;
                }

                ui.separator();
                if ui.button("[+ new session]").clicked() {
                    self.dialog = SessionDialog::default();
                    self.show_session_dialog = true;
                    self.dialog_focus_needed = true;
                }
            });
    }

    // ── Центральная панель ──

    fn render_central_panel(&mut self, ctx: &egui::Context) {
        let any_dialog = self.show_session_dialog || self.show_connect_dialog;
        egui::CentralPanel::default().show(ctx, |ui| {
            let active_id = match self.active_session_id.clone() {
                Some(id) => id,
                None => {
                    ui.centered_and_justified(|ui| {
                        ui.colored_label(
                            crate::theme::GREEN_DIM,
                            "// select or create a session",
                        );
                    });
                    return;
                }
            };

            let conn = match self.connections.get_mut(&active_id) {
                Some(c) => c,
                None => {
                    // Нет активного соединения — показываем статус
                    ui.vertical_centered(|ui| {
                        ui.add_space(ui.available_height() / 3.0);
                        if let Some(err) = &self.last_error {
                            for line in err.lines() {
                                ui.colored_label(crate::theme::RED, line);
                            }
                            ui.add_space(8.0);
                        }
                        ui.colored_label(
                            crate::theme::GREEN_DIM,
                            "Session disconnected. Click to reconnect.",
                        );
                    });
                    return;
                }
            };

            // Проверяем ошибки SSH
            if let Some(err) = conn.ssh.take_error() {
                conn.error = Some(err);
            }

            if let Some(err) = &conn.error {
                ui.colored_label(
                    crate::theme::RED,
                    format!("ERR: {}", err),
                );
            }

            ui.horizontal(|ui| {
                ui.selectable_value(&mut conn.active_tab, Tab::Shell, "[SHELL]");
                ui.selectable_value(&mut conn.active_tab, Tab::Sftp, "[SFTP]");
                ui.selectable_value(&mut conn.active_tab, Tab::Forward, "[FWD]");

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if conn.ssh.is_alive() {
                        ui.colored_label(crate::theme::GREEN, "[ONLINE]");
                    } else {
                        ui.colored_label(crate::theme::RED, "[OFFLINE]");
                    }
                });
            });
            ui.separator();

            match conn.active_tab {
                Tab::Shell => {
                    conn.terminal.show(ui, &conn.ssh, !any_dialog);
                }
                Tab::Sftp => {
                    if conn.sftp.is_none() {
                        match SftpBrowser::new(&conn.config) {
                            Ok(browser) => conn.sftp = Some(browser),
                            Err(e) => {
                                ui.colored_label(
                                    crate::theme::RED,
                                    format!("SFTP ERR: {}", e),
                                );
                            }
                        }
                    }

                    if let Some(sftp) = &mut conn.sftp {
                        sftp.show(ui);
                    }
                }
                Tab::Forward => {
                    if conn.forward.is_none() {
                        conn.forward = Some(PortForwarder::new(&conn.config));
                    }

                    if let Some(fwd) = &mut conn.forward {
                        fwd.show(ui);
                    }
                }
            }
        });
    }

    // ── Диалог ввода пароля при подключении ──

    fn render_connect_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_connect_dialog {
            return;
        }

        let session = self
            .sessions
            .iter()
            .find(|s| s.id == self.connect_dialog.session_id)
            .cloned();
        let session = match session {
            Some(s) => s,
            None => {
                self.show_connect_dialog = false;
                return;
            }
        };

        let display_name = session.name.clone();
        let display_host = format!("{}:{}", session.host, session.port);
        let display_user = session.username.clone();

        let mut open = true;
        let mut do_connect = false;

        egui::Window::new(format!("connect: {}", display_name))
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_width(350.0)
            .default_pos(egui::pos2(
                ctx.screen_rect().center().x - 175.0,
                ctx.screen_rect().center().y - 80.0,
            ))
            .show(ctx, |ui| {
                egui::Grid::new("connect_dialog_grid")
                    .num_columns(2)
                    .spacing([10.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("host:");
                        ui.monospace(&display_host);
                        ui.end_row();

                        ui.label("user:");
                        ui.monospace(&display_user);
                        ui.end_row();

                        ui.label("pass:");
                        let pwd_id = ui.id().with("connect_pwd");
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut self.connect_dialog.password)
                                .id(pwd_id)
                                .password(true)
                                .hint_text("enter password"),
                        );
                        if self.dialog_focus_needed {
                            ui.memory_mut(|m| m.request_focus(pwd_id));
                            self.dialog_focus_needed = false;
                        }
                        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            do_connect = true;
                        }
                        ui.end_row();
                    });

                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);

                ui.horizontal(|ui| {
                    if ui.button("[connect]").clicked() {
                        do_connect = true;
                    }
                    if ui.button("[cancel]").clicked() {
                        self.connect_dialog.password.clear();
                        self.show_connect_dialog = false;
                        self.active_session_id = None;
                    }
                });
            });

        if do_connect && !self.connect_dialog.password.is_empty() {
            let mut config = session;
            config.auth_type = AuthType::Password(self.connect_dialog.password.clone());
            self.connect_session(&config);
            self.connect_dialog.password.clear();
            self.show_connect_dialog = false;
        }

        if !open {
            self.connect_dialog.password.clear();
            self.show_connect_dialog = false;
            self.active_session_id = None;
        }
    }

    // ── Диалог создания/редактирования сессии ──

    fn render_session_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_session_dialog {
            return;
        }

        // Собираем активные SOCKS5-прокси до мутабельного заимствования в closure
        let active_proxies: Vec<(String, String, u16)> = self
            .connections
            .iter()
            .flat_map(|(id, conn)| {
                let session_name = self
                    .sessions
                    .iter()
                    .find(|s| s.id == *id)
                    .map(|s| s.name.clone())
                    .unwrap_or_default();
                conn.forward
                    .as_ref()
                    .map(|fwd| {
                        fwd.active_socks5_proxies()
                            .into_iter()
                            .map(move |(host, port)| (session_name.clone(), host, port))
                    })
                    .into_iter()
                    .flatten()
            })
            .collect();

        let title = if self.dialog.editing_id.is_some() {
            "edit session"
        } else {
            "new session"
        };

        let mut open = true;
        egui::Window::new(title)
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_width(400.0)
            .default_pos(egui::pos2(
                ctx.screen_rect().center().x - 200.0,
                ctx.screen_rect().center().y - 150.0,
            ))
            .show(ctx, |ui| {
                egui::Grid::new("session_dialog_grid")
                    .num_columns(2)
                    .spacing([10.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("name:");
                        let name_id = ui.id().with("session_name");
                        let name_resp = ui.add(
                            egui::TextEdit::singleline(&mut self.dialog.name)
                                .id(name_id)
                                .hint_text("My Server"),
                        );
                        if self.dialog_focus_needed {
                            ui.memory_mut(|m| m.request_focus(name_id));
                            self.dialog_focus_needed = false;
                        }
                        if name_resp.lost_focus()
                            && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        {
                            ui.memory_mut(|m| {
                                m.request_focus(ui.id().with("session_host"));
                            });
                        }
                        ui.end_row();

                        ui.label("host:");
                        let host_id = ui.id().with("session_host");
                        let host_resp = ui.add(
                            egui::TextEdit::singleline(&mut self.dialog.host)
                                .id(host_id)
                                .hint_text("192.168.1.100"),
                        );
                        if host_resp.lost_focus()
                            && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        {
                            ui.memory_mut(|m| {
                                m.request_focus(ui.id().with("session_port"));
                            });
                        }
                        ui.end_row();

                        ui.label("port:");
                        let port_id = ui.id().with("session_port");
                        let port_resp = ui.add(
                            egui::TextEdit::singleline(&mut self.dialog.port)
                                .id(port_id)
                                .desired_width(60.0),
                        );
                        if port_resp.lost_focus()
                            && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        {
                            ui.memory_mut(|m| {
                                m.request_focus(ui.id().with("session_user"));
                            });
                        }
                        ui.end_row();

                        ui.label("user:");
                        let user_id = ui.id().with("session_user");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.dialog.username)
                                .id(user_id)
                                .hint_text("root"),
                        );
                        ui.end_row();

                        ui.label("auth:");
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut self.dialog.auth_choice, 0, "password");
                            ui.radio_value(&mut self.dialog.auth_choice, 1, "key");
                            ui.radio_value(&mut self.dialog.auth_choice, 2, "agent");
                        });
                        ui.end_row();

                        match self.dialog.auth_choice {
                            0 => {
                                ui.label("pass:");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.dialog.password)
                                        .password(true)
                                        .hint_text("optional"),
                                );
                                ui.end_row();

                                ui.label("");
                                ui.colored_label(
                                    crate::theme::GREEN_DIM,
                                    "// if empty, prompted on connect",
                                );
                                ui.end_row();
                            }
                            1 => {
                                ui.label("key:");
                                ui.horizontal(|ui| {
                                    ui.add(
                                        egui::TextEdit::singleline(&mut self.dialog.key_path)
                                            .hint_text("~/.ssh/id_ed25519"),
                                    );
                                });
                                ui.end_row();
                            }
                            _ => {}
                        }

                        ui.label("proxy:");
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut self.dialog.proxy_enabled, false, "none");
                            ui.radio_value(&mut self.dialog.proxy_enabled, true, "socks5");
                        });
                        ui.end_row();

                        if self.dialog.proxy_enabled {
                            ui.label("proxy host:");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.dialog.proxy_host)
                                    .hint_text("127.0.0.1")
                                    .desired_width(180.0),
                            );
                            ui.end_row();

                            ui.label("proxy port:");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.dialog.proxy_port)
                                    .hint_text("1080")
                                    .desired_width(80.0),
                            );
                            ui.end_row();

                            if !active_proxies.is_empty() {
                                ui.label("");
                                ui.vertical(|ui| {
                                    ui.colored_label(
                                        crate::theme::GREEN_DIM,
                                        "// active socks5 proxies:",
                                    );
                                    for (name, host, port) in &active_proxies {
                                        let is_selected =
                                            self.dialog.proxy_host == *host
                                                && self.dialog.proxy_port == port.to_string();
                                        let label_text = format!("{}:{} -- {}", host, port, name);
                                        let resp = ui.add(
                                            egui::SelectableLabel::new(
                                                is_selected,
                                                egui::RichText::new(&label_text),
                                            ),
                                        );
                                        if resp.clicked() {
                                            self.dialog.proxy_host = host.clone();
                                            self.dialog.proxy_port = port.to_string();
                                        }
                                    }
                                });
                                ui.end_row();
                            }
                        }
                    });

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);

                let can_save = !self.dialog.name.is_empty()
                    && !self.dialog.host.is_empty()
                    && !self.dialog.username.is_empty();

                let mut action = DialogAction::None;

                if can_save && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    action = DialogAction::SaveAndConnect;
                }

                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(can_save, egui::Button::new("[save]"))
                        .clicked()
                    {
                        action = DialogAction::Save;
                    }

                    if ui
                        .add_enabled(can_save, egui::Button::new("[save+connect]"))
                        .clicked()
                    {
                        action = DialogAction::SaveAndConnect;
                    }

                    if ui.button("[cancel]").clicked() {
                        self.show_session_dialog = false;
                    }
                });

                if matches!(action, DialogAction::Save | DialogAction::SaveAndConnect) {
                    let password = self.dialog.password.clone();
                    let is_password_auth = self.dialog.auth_choice == 0;
                    let editing_id = self.dialog.editing_id.clone();

                    self.save_session_from_dialog();

                    if matches!(action, DialogAction::SaveAndConnect) {
                        // Определяем ID только что сохранённой сессии
                        let session_id = editing_id.or_else(|| {
                            self.sessions.last().map(|s| s.id.clone())
                        });
                        if let Some(id) = session_id {
                            if is_password_auth && !password.is_empty() {
                                // Подключаемся сразу с введённым паролем
                                if let Some(session) =
                                    self.sessions.iter().find(|s| s.id == id).cloned()
                                {
                                    let mut config = session;
                                    config.auth_type = AuthType::Password(password);
                                    self.connect_session(&config);
                                }
                            } else {
                                self.try_connect(&id);
                            }
                        }
                    }
                }
            });

        if !open {
            self.show_session_dialog = false;
        }
    }
}

impl eframe::App for AppState {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.031, 0.031, 0.031, 1.0] // theme::BG as opaque
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Custom title bar
        egui::TopBottomPanel::top("titlebar")
            .exact_height(28.0)
            .frame(
                egui::Frame::none()
                    .fill(crate::theme::BG)
                    .inner_margin(egui::Margin::symmetric(6.0, 0.0)),
            )
            .show(ctx, |ui| {
                let full_rect = ui.max_rect();
                let btn_w: f32 = 32.0;
                let btn_h: f32 = 24.0;
                let btn_gap: f32 = 2.0;
                let btn_margin: f32 = 6.0;
                let buttons_total = btn_w * 3.0 + btn_gap * 2.0 + btn_margin;

                // ── Buttons FIRST so they get interaction priority ──
                let button_area = egui::Rect::from_min_max(
                    egui::pos2(full_rect.max.x - buttons_total, full_rect.min.y),
                    full_rect.max,
                );

                let mut close_clicked = false;
                let mut max_clicked = false;
                let mut min_clicked = false;
                let mut any_btn_hovered = false;

                ui.allocate_new_ui(egui::UiBuilder::new().max_rect(button_area), |ui| {
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            ui.spacing_mut().item_spacing.x = btn_gap;

                            let close = ui.add(
                                egui::Button::new(
                                    egui::RichText::new(" x ")
                                        .monospace()
                                        .size(14.0),
                                )
                                .min_size(egui::vec2(btn_w, btn_h)),
                            );
                            close_clicked = close.clicked();
                            any_btn_hovered |= close.hovered();

                            let maximize = ui.add(
                                egui::Button::new(
                                    egui::RichText::new(" o ")
                                        .monospace()
                                        .size(14.0),
                                )
                                .min_size(egui::vec2(btn_w, btn_h)),
                            );
                            max_clicked = maximize.clicked();
                            any_btn_hovered |= maximize.hovered();

                            let minimize = ui.add(
                                egui::Button::new(
                                    egui::RichText::new(" _ ")
                                        .monospace()
                                        .size(14.0),
                                )
                                .min_size(egui::vec2(btn_w, btn_h)),
                            );
                            min_clicked = minimize.clicked();
                            any_btn_hovered |= minimize.hovered();
                        },
                    );
                });

                if close_clicked {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                if max_clicked {
                    let is_max = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!is_max));
                }
                if min_clicked {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                }

                // ── Title text ──
                ui.painter().text(
                    egui::pos2(full_rect.min.x + 8.0, full_rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    "SSHerald",
                    egui::FontId::monospace(13.0),
                    crate::theme::GREEN_DIM,
                );

                // ── Drag zone — only when no button is hovered ──
                let drag_rect = egui::Rect::from_min_max(
                    full_rect.min,
                    egui::pos2(full_rect.max.x - buttons_total - 4.0, full_rect.max.y),
                );
                let drag_resp = ui.interact(
                    drag_rect,
                    ui.id().with("titlebar_drag"),
                    egui::Sense::click_and_drag(),
                );
                if drag_resp.drag_started() && !any_btn_hovered {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                if drag_resp.double_clicked() {
                    let is_max = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!is_max));
                }

                // ── Bottom separator ──
                ui.painter().line_segment(
                    [
                        egui::pos2(full_rect.min.x, full_rect.max.y),
                        egui::pos2(full_rect.max.x, full_rect.max.y),
                    ],
                    egui::Stroke::new(1.0, crate::theme::GREEN_DARK),
                );
            });

        // Bottom border line
        egui::TopBottomPanel::bottom("bottom_border")
            .exact_height(1.0)
            .frame(egui::Frame::none().fill(crate::theme::GREEN_DARK))
            .show(ctx, |_| {});

        // Paint side borders on foreground layer
        let screen = ctx.screen_rect();
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("window_border"),
        ));
        painter.rect_stroke(
            screen,
            0.0,
            egui::Stroke::new(1.0, crate::theme::GREEN_DARK),
        );

        // Dead session cleanup
        let dead_ids: Vec<String> = self
            .connections
            .iter()
            .filter(|(_, conn)| !conn.ssh.is_alive())
            .map(|(id, _)| id.clone())
            .collect();

        for id in &dead_ids {
            let error = self.connections.get(id).and_then(|conn| {
                conn.ssh.take_error().or_else(|| conn.error.clone())
            });
            if let Some(err) = error {
                self.last_error = Some(err);
            }
            self.connections.remove(id);
        }

        self.render_sessions_panel(ctx);
        self.render_central_panel(ctx);
        self.render_session_dialog(ctx);
        self.render_connect_dialog(ctx);

        if !self.connections.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        config::save_sessions(&self.sessions);
    }
}
