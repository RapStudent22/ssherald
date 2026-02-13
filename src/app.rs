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
    // Диалог создания/редактирования сессии
    show_session_dialog: bool,
    dialog: SessionDialog,
    // Диалог ввода пароля при подключении
    show_connect_dialog: bool,
    connect_dialog: ConnectDialog,
    // Последняя ошибка (отображается после авто-отключения)
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
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let sessions = config::load_sessions();

        AppState {
            sessions,
            active_session_id: None,
            connections: HashMap::new(),
            show_session_dialog: false,
            dialog: SessionDialog::default(),
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
                    ui.heading("SSHerald");
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

                        // Фон: подсветка активной / наведённой строки
                        let accent = egui::Color32::from_rgb(98, 114, 164);
                        if is_active {
                            // Заметный фон для активной сессии
                            ui.painter().rect_filled(
                                rect,
                                4.0,
                                ui.visuals().selection.bg_fill,
                            );
                            // Акцентная полоска слева
                            let bar = egui::Rect::from_min_max(
                                rect.min,
                                egui::pos2(rect.min.x + 3.0, rect.max.y),
                            );
                            ui.painter().rect_filled(bar, 2.0, accent);
                        } else if button.hovered() {
                            ui.painter().rect_filled(
                                rect,
                                4.0,
                                ui.visuals().widgets.hovered.bg_fill,
                            );
                        }

                        // Название сессии — слева (с отступом от полоски)
                        let text_left = if is_active { rect.min.x + 10.0 } else { rect.min.x + 8.0 };
                        let text_color = if is_active {
                            egui::Color32::WHITE
                        } else {
                            ui.visuals().text_color()
                        };
                        let font = if is_active {
                            egui::FontId::proportional(14.0)
                        } else {
                            egui::FontId::proportional(13.5)
                        };
                        ui.painter().text(
                            egui::pos2(text_left, rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            &session.name,
                            font,
                            text_color,
                        );

                        // Индикатор статуса — справа
                        let dot_radius = 4.0;
                        let dot_center = egui::pos2(
                            rect.max.x - 12.0,
                            rect.center().y,
                        );
                        let dot_color = if is_connected {
                            egui::Color32::from_rgb(80, 250, 123)
                        } else {
                            egui::Color32::from_rgb(255, 85, 85)
                        };
                        ui.painter().circle_filled(dot_center, dot_radius, dot_color);

                        if button.clicked() {
                            if is_connected {
                                self.active_session_id = Some(session.id.clone());
                            } else {
                                connect_id = Some(session.id.clone());
                            }
                        }

                        button.context_menu(|ui| {
                            if !is_connected {
                                if ui.button("🔌 Подключить").clicked() {
                                    connect_id = Some(session.id.clone());
                                    ui.close_menu();
                                }
                            } else {
                                if ui.button("❌ Отключить").clicked() {
                                    disconnect_id = Some(session.id.clone());
                                    ui.close_menu();
                                }
                            }
                            if ui.button("✏ Редактировать").clicked() {
                                edit_session = Some(session.clone());
                                ui.close_menu();
                            }
                            ui.separator();
                            if ui.button("🗑 Удалить").clicked() {
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
                }

                ui.separator();
                if ui.button("➕ Новая сессия").clicked() {
                    self.dialog = SessionDialog::default();
                    self.show_session_dialog = true;
                }
            });
    }

    // ── Центральная панель ──

    fn render_central_panel(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            let active_id = match self.active_session_id.clone() {
                Some(id) => id,
                None => {
                    ui.centered_and_justified(|ui| {
                        ui.label("Выберите или создайте сессию слева");
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
                                ui.colored_label(
                                    egui::Color32::from_rgb(255, 85, 85),
                                    line,
                                );
                            }
                            ui.add_space(8.0);
                        }
                        ui.label("Сессия не подключена. Нажмите на неё для повторного подключения.");
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
                    egui::Color32::from_rgb(255, 85, 85),
                    format!("Ошибка: {}", err),
                );
            }

            // Панель вкладок + статус подключения
            ui.horizontal(|ui| {
                ui.selectable_value(&mut conn.active_tab, Tab::Shell, "🖥 Shell");
                ui.selectable_value(&mut conn.active_tab, Tab::Sftp, "📁 SFTP");
                ui.selectable_value(&mut conn.active_tab, Tab::Forward, "🔀 Port Forward");

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if conn.ssh.is_alive() {
                        ui.colored_label(egui::Color32::from_rgb(80, 250, 123), "● Подключено");
                    } else {
                        ui.colored_label(egui::Color32::from_rgb(255, 85, 85), "● Отключено");
                    }
                });
            });
            ui.separator();

            // Содержимое вкладки
            match conn.active_tab {
                Tab::Shell => {
                    conn.terminal.show(ui, &conn.ssh);
                }
                Tab::Sftp => {
                    if conn.sftp.is_none() {
                        match SftpBrowser::new(&conn.config) {
                            Ok(browser) => conn.sftp = Some(browser),
                            Err(e) => {
                                ui.colored_label(
                                    egui::Color32::from_rgb(255, 85, 85),
                                    format!("Ошибка SFTP: {}", e),
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

        egui::Window::new(format!("🔌 {}", display_name))
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
                        ui.label("Хост:");
                        ui.monospace(&display_host);
                        ui.end_row();

                        ui.label("Пользователь:");
                        ui.monospace(&display_user);
                        ui.end_row();

                        ui.label("Пароль:");
                        let pwd_id = ui.id().with("connect_pwd");
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut self.connect_dialog.password)
                                .id(pwd_id)
                                .password(true)
                                .hint_text("Введите пароль"),
                        );
                        // Автофокус при открытии диалога
                        if !resp.has_focus() && self.connect_dialog.password.is_empty() {
                            ui.memory_mut(|m| m.request_focus(pwd_id));
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
                    if ui.button("🔌 Подключить").clicked() {
                        do_connect = true;
                    }
                    if ui.button("Отмена").clicked() {
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
            "Редактировать сессию"
        } else {
            "Новая сессия"
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
                        ui.label("Имя:");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.dialog.name)
                                .hint_text("My Server"),
                        );
                        ui.end_row();

                        ui.label("Хост:");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.dialog.host)
                                .hint_text("192.168.1.100"),
                        );
                        ui.end_row();

                        ui.label("Порт:");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.dialog.port).desired_width(60.0),
                        );
                        ui.end_row();

                        ui.label("Пользователь:");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.dialog.username)
                                .hint_text("root"),
                        );
                        ui.end_row();

                        ui.label("Авторизация:");
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut self.dialog.auth_choice, 0, "Пароль");
                            ui.radio_value(&mut self.dialog.auth_choice, 1, "Ключ");
                            ui.radio_value(&mut self.dialog.auth_choice, 2, "Agent");
                        });
                        ui.end_row();

                        match self.dialog.auth_choice {
                            0 => {
                                ui.label("Пароль:");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.dialog.password)
                                        .password(true)
                                        .hint_text("опционально"),
                                );
                                ui.end_row();

                                ui.label("");
                                ui.colored_label(
                                    egui::Color32::from_rgb(139, 233, 253),
                                    "Если пусто — запросится при подключении",
                                );
                                ui.end_row();
                            }
                            1 => {
                                ui.label("Путь к ключу:");
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

                        // ── Прокси ──
                        ui.label("Прокси:");
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut self.dialog.proxy_enabled, false, "Нет");
                            ui.radio_value(&mut self.dialog.proxy_enabled, true, "SOCKS5");
                        });
                        ui.end_row();

                        if self.dialog.proxy_enabled {
                            ui.label("Прокси хост:");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.dialog.proxy_host)
                                    .hint_text("127.0.0.1")
                                    .desired_width(180.0),
                            );
                            ui.end_row();

                            ui.label("Прокси порт:");
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
                                        egui::Color32::from_rgb(139, 233, 253),
                                        "Активные SOCKS5-прокси:",
                                    );
                                    for (name, host, port) in &active_proxies {
                                        let is_selected =
                                            self.dialog.proxy_host == *host
                                                && self.dialog.proxy_port == port.to_string();
                                        let label_text = format!("{}:{} — {}", host, port, name);
                                        let selected = is_selected;
                                        let resp = ui.add(
                                            egui::SelectableLabel::new(
                                                selected,
                                                egui::RichText::new(&label_text).size(12.5),
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

                let mut action = DialogAction::None;
                ui.horizontal(|ui| {
                    let can_save = !self.dialog.name.is_empty()
                        && !self.dialog.host.is_empty()
                        && !self.dialog.username.is_empty();

                    if ui
                        .add_enabled(can_save, egui::Button::new("💾 Сохранить"))
                        .clicked()
                    {
                        action = DialogAction::Save;
                    }

                    if ui
                        .add_enabled(can_save, egui::Button::new("🔌 Сохранить и подключить"))
                        .clicked()
                    {
                        action = DialogAction::SaveAndConnect;
                    }

                    if ui.button("Отмена").clicked() {
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
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Автоотключение мёртвых сессий (exit, обрыв связи и т.д.)
        let dead_ids: Vec<String> = self
            .connections
            .iter()
            .filter(|(_, conn)| !conn.ssh.is_alive())
            .map(|(id, _)| id.clone())
            .collect();

        for id in &dead_ids {
            // Сохраняем ошибку до удаления соединения
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

        // Периодическая перерисовка (~60 FPS) при наличии активных соединений
        if !self.connections.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        config::save_sessions(&self.sessions);
    }
}
