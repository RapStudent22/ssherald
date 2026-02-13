use std::collections::HashSet;
use std::sync::mpsc;

use crate::ssh::session::{create_russh_session, SessionConfig, SshHandler};

#[derive(Clone)]
pub struct SftpEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<u64>,
}

enum SftpRequest {
    ListDir(String),
    Download { remote: String, local: String },
    Upload { local: String, remote: String },
    Mkdir(String),
    Remove(String),
    Rename { from: String, to: String },
}

enum SftpResponse {
    DirListing(String, Vec<SftpEntry>),
    Error(String),
    Success(String),
}

pub struct SftpBrowser {
    pub current_path: String,
    pub entries: Vec<SftpEntry>,
    pub error: Option<String>,
    pub loading: bool,
    pub status_message: Option<String>,
    request_tx: tokio::sync::mpsc::UnboundedSender<SftpRequest>,
    response_rx: mpsc::Receiver<SftpResponse>,
    navigate_to: Option<String>,
    // Выделение файлов
    selected: HashSet<String>,
    // Диалоги
    show_mkdir_dialog: bool,
    mkdir_name: String,
}

impl SftpBrowser {
    pub fn new(config: &SessionConfig) -> Result<Self, String> {
        let (req_tx, req_rx) = tokio::sync::mpsc::unbounded_channel();
        let (resp_tx, resp_rx) = mpsc::channel();

        let config = config.clone();

        std::thread::spawn(move || {
            let rt = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = resp_tx.send(SftpResponse::Error(format!(
                        "Tokio runtime: {}",
                        e
                    )));
                    return;
                }
            };
            if let Err(e) = rt.block_on(sftp_thread_async(&config, req_rx, &resp_tx)) {
                let _ = resp_tx.send(SftpResponse::Error(e.to_string()));
            }
        });

        let browser = SftpBrowser {
            current_path: "/".to_string(),
            entries: Vec::new(),
            error: None,
            loading: true,
            status_message: None,
            request_tx: req_tx,
            response_rx: resp_rx,
            navigate_to: None,
            selected: HashSet::new(),
            show_mkdir_dialog: false,
            mkdir_name: String::new(),
        };

        browser
            .request_tx
            .send(SftpRequest::ListDir("/home".to_string()))
            .map_err(|e| e.to_string())?;

        Ok(browser)
    }

    pub fn navigate(&mut self, path: &str) {
        self.loading = true;
        self.error = None;
        self.selected.clear();
        let _ = self
            .request_tx
            .send(SftpRequest::ListDir(path.to_string()));
    }

    pub fn download(&self, remote: &str, local: &str) {
        let _ = self.request_tx.send(SftpRequest::Download {
            remote: remote.to_string(),
            local: local.to_string(),
        });
    }

    pub fn upload(&self, local: &str, remote: &str) {
        let _ = self.request_tx.send(SftpRequest::Upload {
            local: local.to_string(),
            remote: remote.to_string(),
        });
    }

    pub fn mkdir(&self, path: &str) {
        let _ = self
            .request_tx
            .send(SftpRequest::Mkdir(path.to_string()));
    }

    pub fn remove(&self, path: &str) {
        let _ = self
            .request_tx
            .send(SftpRequest::Remove(path.to_string()));
    }

    #[allow(dead_code)]
    pub fn rename(&self, from: &str, to: &str) {
        let _ = self.request_tx.send(SftpRequest::Rename {
            from: from.to_string(),
            to: to.to_string(),
        });
    }

    fn poll(&mut self) {
        while let Ok(response) = self.response_rx.try_recv() {
            match response {
                SftpResponse::DirListing(path, entries) => {
                    self.current_path = path;
                    self.entries = entries;
                    self.loading = false;
                }
                SftpResponse::Error(e) => {
                    self.error = Some(e);
                    self.loading = false;
                }
                SftpResponse::Success(msg) => {
                    self.status_message = Some(msg);
                    let _ = self
                        .request_tx
                        .send(SftpRequest::ListDir(self.current_path.clone()));
                }
            }
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        self.poll();

        // ── Drag & Drop: загрузка файлов на сервер ──
        let dropped = ui.ctx().input(|i| i.raw.dropped_files.clone());
        if !dropped.is_empty() {
            let mut count = 0usize;
            for file in &dropped {
                if let Some(path) = &file.path {
                    let filename = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    let remote = format!(
                        "{}/{}",
                        self.current_path.trim_end_matches('/'),
                        filename
                    );
                    self.upload(&path.to_string_lossy(), &remote);
                    count += 1;
                }
            }
            if count > 0 {
                self.status_message = Some(format!("Загрузка {} файлов на сервер...", count));
            }
        }

        // ── Панель инструментов ──
        ui.horizontal(|ui| {
            // Навигация
            if ui.button("⬆ Вверх").clicked() {
                let parent = std::path::Path::new(&self.current_path)
                    .parent()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|| "/".to_string());
                self.navigate_to = Some(parent);
            }
            ui.separator();
            ui.monospace(&self.current_path);
            ui.separator();
            if ui.button("🔄").clicked() {
                self.navigate_to = Some(self.current_path.clone());
            }
            ui.separator();
            if ui.button("📁 Новая папка").clicked() {
                self.show_mkdir_dialog = true;
                self.mkdir_name.clear();
            }
        });

        // Вторая строка: действия с файлами
        ui.horizontal(|ui| {
            let n = self.selected.len();

            // Скачать выбранные
            if ui
                .add_enabled(
                    n > 0,
                    egui::Button::new(format!("⬇ Скачать выбранные ({})", n)),
                )
                .clicked()
            {
                self.download_selected();
            }

            ui.separator();

            // Загрузить через диалог
            if ui.button("⬆ Загрузить файлы...").clicked() {
                self.upload_via_dialog();
            }

            ui.separator();

            // Выделить все / снять выделение
            if n > 0 {
                if ui.button("✖ Снять выделение").clicked() {
                    self.selected.clear();
                }
            } else if !self.entries.is_empty() {
                if ui.button("☑ Выделить все файлы").clicked() {
                    for e in &self.entries {
                        if !e.is_dir {
                            self.selected.insert(e.path.clone());
                        }
                    }
                }
            }
        });

        // ── Ошибки / статус ──
        if let Some(err) = &self.error {
            ui.colored_label(
                egui::Color32::from_rgb(255, 85, 85),
                format!("Ошибка: {}", err),
            );
        }
        if let Some(msg) = self.status_message.take() {
            ui.colored_label(egui::Color32::from_rgb(80, 250, 123), &msg);
        }

        if self.loading {
            ui.spinner();
            return;
        }

        ui.separator();

        // ── Таблица файлов ──
        let mut navigate_path: Option<String> = None;
        let mut delete_path: Option<String> = None;
        let mut toggle_selection: Vec<(String, bool)> = Vec::new();
        let mut download_single: Vec<(String, String)> = Vec::new();

        // Снимок для использования в замыканиях без borrow conflict
        let entries = self.entries.clone();
        let selected_snapshot = self.selected.clone();
        let current_path = self.current_path.clone();

        let available_height = ui.available_height();

        egui::ScrollArea::vertical()
            .max_height(available_height)
            .show(ui, |ui| {
                egui_extras::TableBuilder::new(ui)
                    .striped(true)
                    .resizable(true)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .column(egui_extras::Column::exact(28.0)) // чекбокс
                    .column(egui_extras::Column::remainder().at_least(200.0)) // имя
                    .column(egui_extras::Column::auto().at_least(80.0)) // размер
                    .column(egui_extras::Column::auto().at_least(140.0)) // дата
                    .header(24.0, |mut header| {
                        header.col(|ui| {
                            ui.label("");
                        });
                        header.col(|ui| {
                            ui.strong("Имя");
                        });
                        header.col(|ui| {
                            ui.strong("Размер");
                        });
                        header.col(|ui| {
                            ui.strong("Изменён");
                        });
                    })
                    .body(|body| {
                        body.rows(22.0, entries.len(), |mut row| {
                            let idx = row.index();
                            let entry = &entries[idx];

                            // Чекбокс
                            row.col(|ui| {
                                let is_sel = selected_snapshot.contains(&entry.path);
                                let mut checked = is_sel;
                                if ui.checkbox(&mut checked, "").changed() {
                                    toggle_selection.push((entry.path.clone(), checked));
                                }
                            });

                            // Имя + навигация + контекстное меню
                            row.col(|ui| {
                                let icon = if entry.is_dir { "📁" } else { "📄" };
                                let is_sel = selected_snapshot.contains(&entry.path);
                                let label = format!("{} {}", icon, entry.name);

                                let response = ui.selectable_label(is_sel, &label);

                                if response.clicked() {
                                    if entry.is_dir {
                                        navigate_path = Some(entry.path.clone());
                                    } else {
                                        // Toggle selection по клику на файл
                                        toggle_selection.push((entry.path.clone(), !is_sel));
                                    }
                                }

                                // Контекстное меню
                                response.context_menu(|ui| {
                                    if !entry.is_dir {
                                        if ui.button("⬇ Скачать").clicked() {
                                            if let Some(dir) = dirs::download_dir() {
                                                let local = dir.join(&entry.name);
                                                download_single.push((
                                                    entry.path.clone(),
                                                    local.to_string_lossy().to_string(),
                                                ));
                                            }
                                            ui.close_menu();
                                        }
                                    }
                                    if entry.is_dir {
                                        if ui.button("📂 Открыть").clicked() {
                                            navigate_path = Some(entry.path.clone());
                                            ui.close_menu();
                                        }
                                    }
                                    ui.separator();
                                    if ui.button("🗑 Удалить").clicked() {
                                        delete_path = Some(entry.path.clone());
                                        ui.close_menu();
                                    }
                                });
                            });

                            // Размер
                            row.col(|ui| {
                                if !entry.is_dir {
                                    ui.label(format_size(entry.size));
                                }
                            });

                            // Дата
                            row.col(|ui| {
                                if let Some(ts) = entry.modified {
                                    ui.label(format_timestamp(ts));
                                }
                            });
                        });
                    });
            });

        // ── Drag & drop overlay ──
        let hovering = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
        if hovering {
            let rect = ui.max_rect();
            ui.painter().rect_filled(
                rect,
                8.0,
                egui::Color32::from_rgba_premultiplied(80, 140, 220, 50),
            );
            ui.painter().rect_stroke(
                rect.shrink(4.0),
                8.0,
                egui::Stroke::new(2.0, egui::Color32::from_rgb(139, 233, 253)),
            );
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "📤 Перетащите файлы сюда для загрузки на сервер",
                egui::FontId::proportional(20.0),
                egui::Color32::WHITE,
            );
        }

        // ── Применение отложенных действий ──
        if let Some(path) = navigate_path.or(self.navigate_to.take()) {
            self.navigate(&path);
        }
        if let Some(path) = delete_path {
            self.remove(&path);
        }
        for (path, selected) in toggle_selection {
            if selected {
                self.selected.insert(path);
            } else {
                self.selected.remove(&path);
            }
        }
        for (remote, local) in download_single {
            self.download(&remote, &local);
        }

        // ── Диалог создания папки ──
        if self.show_mkdir_dialog {
            egui::Window::new("Создать папку")
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Имя:");
                        ui.text_edit_singleline(&mut self.mkdir_name);
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Создать").clicked() && !self.mkdir_name.is_empty() {
                            let full_path = format!(
                                "{}/{}",
                                current_path.trim_end_matches('/'),
                                self.mkdir_name
                            );
                            self.mkdir(&full_path);
                            self.show_mkdir_dialog = false;
                        }
                        if ui.button("Отмена").clicked() {
                            self.show_mkdir_dialog = false;
                        }
                    });
                });
        }
    }

    // ── Скачать все выбранные файлы в ~/Downloads ──
    fn download_selected(&mut self) {
        if let Some(dir) = dirs::download_dir() {
            let selected: Vec<String> = self.selected.iter().cloned().collect();
            for path in &selected {
                let filename = std::path::Path::new(path)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let local = dir.join(&filename);
                self.download(path, &local.to_string_lossy());
            }
            self.status_message = Some(format!(
                "Скачивание {} файлов в {}...",
                selected.len(),
                dir.display()
            ));
            self.selected.clear();
        } else {
            self.error = Some("Не удалось определить папку загрузок".to_string());
        }
    }

    // ── Загрузить файлы через нативный диалог ──
    fn upload_via_dialog(&mut self) {
        let dialog = rfd::FileDialog::new().set_title("Выберите файлы для загрузки на сервер");

        if let Some(files) = dialog.pick_files() {
            let mut count = 0usize;
            for file in &files {
                let filename = file
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let remote = format!(
                    "{}/{}",
                    self.current_path.trim_end_matches('/'),
                    filename
                );
                self.upload(&file.to_string_lossy(), &remote);
                count += 1;
            }
            if count > 0 {
                self.status_message = Some(format!("Загрузка {} файлов на сервер...", count));
            }
        }
    }
}

// ── Фоновый async SFTP-поток ──

async fn sftp_thread_async(
    config: &SessionConfig,
    mut req_rx: tokio::sync::mpsc::UnboundedReceiver<SftpRequest>,
    resp_tx: &mpsc::Sender<SftpResponse>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let session = create_russh_session(config, SshHandler::new()).await?;

    // Открываем SFTP-подсистему
    let channel = session.channel_open_session().await?;
    channel.request_subsystem(true, "sftp").await?;
    let sftp = russh_sftp::client::SftpSession::new(channel.into_stream()).await?;

    while let Some(req) = req_rx.recv().await {
        match req {
            SftpRequest::ListDir(path) => match list_dir_async(&sftp, &path).await {
                Ok(entries) => {
                    let _ = resp_tx.send(SftpResponse::DirListing(path, entries));
                }
                Err(e) => {
                    let _ = resp_tx.send(SftpResponse::Error(e.to_string()));
                }
            },
            SftpRequest::Download { remote, local } => {
                match download_file_async(&sftp, &remote, &local).await {
                    Ok(()) => {
                        let _ =
                            resp_tx.send(SftpResponse::Success(format!("✅ Скачано: {}", remote)));
                    }
                    Err(e) => {
                        let _ = resp_tx.send(SftpResponse::Error(e.to_string()));
                    }
                }
            }
            SftpRequest::Upload { local, remote } => {
                match upload_file_async(&sftp, &local, &remote).await {
                    Ok(()) => {
                        let _ = resp_tx
                            .send(SftpResponse::Success(format!("✅ Загружено: {}", remote)));
                    }
                    Err(e) => {
                        let _ = resp_tx.send(SftpResponse::Error(e.to_string()));
                    }
                }
            }
            SftpRequest::Mkdir(path) => match sftp.create_dir(&path).await {
                Ok(()) => {
                    let _ = resp_tx
                        .send(SftpResponse::Success(format!("✅ Создана папка: {}", path)));
                }
                Err(e) => {
                    let _ = resp_tx.send(SftpResponse::Error(e.to_string()));
                }
            },
            SftpRequest::Remove(path) => {
                let result = match sftp.remove_file(&path).await {
                    Ok(()) => Ok(()),
                    Err(_) => sftp.remove_dir(&path).await,
                };
                match result {
                    Ok(()) => {
                        let _ = resp_tx
                            .send(SftpResponse::Success(format!("✅ Удалено: {}", path)));
                    }
                    Err(e) => {
                        let _ = resp_tx.send(SftpResponse::Error(e.to_string()));
                    }
                }
            }
            SftpRequest::Rename { from, to } => match sftp.rename(&from, &to).await {
                Ok(()) => {
                    let _ = resp_tx.send(SftpResponse::Success(format!(
                        "✅ Переименовано: {} → {}",
                        from, to
                    )));
                }
                Err(e) => {
                    let _ = resp_tx.send(SftpResponse::Error(e.to_string()));
                }
            },
        }
    }

    Ok(())
}

async fn list_dir_async(
    sftp: &russh_sftp::client::SftpSession,
    path: &str,
) -> Result<Vec<SftpEntry>, Box<dyn std::error::Error + Send + Sync>> {
    let entries = sftp.read_dir(path).await?;
    let mut result: Vec<SftpEntry> = entries
        .into_iter()
        .filter_map(|entry| {
            let name = entry.file_name();
            if name == "." || name == ".." {
                return None;
            }
            let file_path = if path == "/" {
                format!("/{}", name)
            } else {
                format!("{}/{}", path.trim_end_matches('/'), name)
            };
            let metadata = entry.metadata();
            let is_dir = metadata.is_dir();
            let size = metadata.len();
            let modified = metadata.modified().ok().and_then(|t| {
                t.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs())
            });
            Some(SftpEntry {
                name,
                path: file_path,
                is_dir,
                size,
                modified,
            })
        })
        .collect();
    result.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.cmp(&b.name)));
    Ok(result)
}

async fn download_file_async(
    sftp: &russh_sftp::client::SftpSession,
    remote: &str,
    local: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use tokio::io::AsyncReadExt;
    let mut remote_file = sftp.open(remote).await?;
    let mut data = Vec::new();
    remote_file.read_to_end(&mut data).await?;
    tokio::fs::write(local, &data).await?;
    Ok(())
}

async fn upload_file_async(
    sftp: &russh_sftp::client::SftpSession,
    local: &str,
    remote: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use tokio::io::AsyncWriteExt;
    let data = tokio::fs::read(local).await?;
    let mut remote_file = sftp.create(remote).await?;
    remote_file.write_all(&data).await?;
    remote_file.shutdown().await?;
    Ok(())
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} Б", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} КБ", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} МБ", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} ГБ", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

fn format_timestamp(ts: u64) -> String {
    let secs = ts;
    let days = secs / 86400;
    let years = 1970 + days / 365;
    let remaining_days = days % 365;
    let months = remaining_days / 30 + 1;
    let day = remaining_days % 30 + 1;
    format!("{:04}-{:02}-{:02}", years, months, day)
}
