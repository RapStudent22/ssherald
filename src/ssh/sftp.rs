use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;

use crate::ssh::session::{create_russh_session, SessionConfig, SshHandler};

const CHUNK_SIZE: usize = 256 * 1024; // 256 KB per I/O op — sweet spot for SFTP throughput

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
    Download {
        remote: String,
        local: String,
        progress: Arc<TransferState>,
    },
    Upload {
        local: String,
        remote: String,
        progress: Arc<TransferState>,
    },
    Mkdir(String),
    Remove(String),
    Rename {
        from: String,
        to: String,
    },
}

enum SftpResponse {
    DirListing(String, Vec<SftpEntry>),
    Error(String),
    Success(String),
}

pub struct TransferState {
    pub name: String,
    pub total: AtomicU64,
    pub transferred: AtomicU64,
    pub done: AtomicBool,
    pub failed: AtomicBool,
    pub is_upload: bool,
}

impl TransferState {
    fn new(name: &str, total: u64, is_upload: bool) -> Arc<Self> {
        Arc::new(TransferState {
            name: name.to_string(),
            total: AtomicU64::new(total),
            transferred: AtomicU64::new(0),
            done: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            is_upload,
        })
    }

    fn fraction(&self) -> f32 {
        let total = self.total.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        let done = self.transferred.load(Ordering::Relaxed);
        (done as f64 / total as f64) as f32
    }
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
    selected: HashSet<String>,
    show_mkdir_dialog: bool,
    mkdir_name: String,
    active_transfers: Vec<Arc<TransferState>>,
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
            active_transfers: Vec::new(),
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

    pub fn download(&mut self, remote: &str, local: &str, file_size: u64) {
        let name = std::path::Path::new(remote)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let progress = TransferState::new(&name, file_size, false);
        self.active_transfers.push(Arc::clone(&progress));
        let _ = self.request_tx.send(SftpRequest::Download {
            remote: remote.to_string(),
            local: local.to_string(),
            progress,
        });
    }

    pub fn upload(&mut self, local: &str, remote: &str) {
        let file_size = std::fs::metadata(local).map(|m| m.len()).unwrap_or(0);
        let name = std::path::Path::new(local)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let progress = TransferState::new(&name, file_size, true);
        self.active_transfers.push(Arc::clone(&progress));
        let _ = self.request_tx.send(SftpRequest::Upload {
            local: local.to_string(),
            remote: remote.to_string(),
            progress,
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

        self.active_transfers
            .retain(|t| !t.done.load(Ordering::Relaxed) && !t.failed.load(Ordering::Relaxed));
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        self.poll();

        // Drag & Drop
        let dropped = ui.ctx().input(|i| i.raw.dropped_files.clone());
        if !dropped.is_empty() {
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
                }
            }
        }

        // Toolbar
        ui.horizontal(|ui| {
            if ui.button("[..]").clicked() {
                let parent = std::path::Path::new(&self.current_path)
                    .parent()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|| "/".to_string());
                self.navigate_to = Some(parent);
            }
            ui.separator();
            ui.monospace(&self.current_path);
            ui.separator();
            if ui.button("[reload]").clicked() {
                self.navigate_to = Some(self.current_path.clone());
            }
            ui.separator();
            if ui.button("[mkdir]").clicked() {
                self.show_mkdir_dialog = true;
                self.mkdir_name.clear();
            }
        });

        ui.horizontal(|ui| {
            let n = self.selected.len();
            if ui
                .add_enabled(n > 0, egui::Button::new(format!("[get {}]", n)))
                .clicked()
            {
                self.download_selected();
            }
            ui.separator();
            if ui.button("[put...]").clicked() {
                self.upload_via_dialog();
            }
            ui.separator();
            if n > 0 {
                if ui.button("[clear]").clicked() {
                    self.selected.clear();
                }
            } else if !self.entries.is_empty() {
                if ui.button("[sel all]").clicked() {
                    for e in &self.entries {
                        if !e.is_dir {
                            self.selected.insert(e.path.clone());
                        }
                    }
                }
            }
        });

        // Active transfers progress
        if !self.active_transfers.is_empty() {
            ui.add_space(2.0);
            let needs_repaint = !self.active_transfers.is_empty();
            for transfer in &self.active_transfers {
                let frac = transfer.fraction();
                let total = transfer.total.load(Ordering::Relaxed);
                let transferred = transfer.transferred.load(Ordering::Relaxed);
                let direction = if transfer.is_upload { "PUT" } else { "GET" };

                ui.horizontal(|ui| {
                    ui.colored_label(
                        crate::theme::GREEN_DIM,
                        format!(
                            "{} {} {}/{}",
                            direction,
                            transfer.name,
                            format_size(transferred),
                            format_size(total),
                        ),
                    );
                });

                let bar_rect = ui.allocate_space(egui::vec2(ui.available_width(), 4.0)).1;
                ui.painter().rect_filled(
                    bar_rect,
                    0.0,
                    crate::theme::BG_WIDGET,
                );
                let filled = egui::Rect::from_min_size(
                    bar_rect.min,
                    egui::vec2(bar_rect.width() * frac, bar_rect.height()),
                );
                ui.painter().rect_filled(
                    filled,
                    0.0,
                    crate::theme::GREEN,
                );
            }
            ui.add_space(2.0);
            if needs_repaint {
                ui.ctx().request_repaint();
            }
        }

        // Errors / status
        if let Some(err) = &self.error {
            ui.colored_label(crate::theme::RED, format!("ERR: {}", err));
        }
        if let Some(msg) = self.status_message.take() {
            ui.colored_label(crate::theme::GREEN, &msg);
        }

        if self.loading {
            ui.spinner();
            return;
        }

        ui.separator();

        // File table
        let mut navigate_path: Option<String> = None;
        let mut delete_path: Option<String> = None;
        let mut toggle_selection: Vec<(String, bool)> = Vec::new();
        let mut download_single: Vec<(String, String, u64)> = Vec::new();

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
                    .column(egui_extras::Column::exact(28.0))
                    .column(egui_extras::Column::remainder().at_least(200.0))
                    .column(egui_extras::Column::auto().at_least(80.0))
                    .column(egui_extras::Column::auto().at_least(140.0))
                    .header(24.0, |mut header| {
                        header.col(|ui| { ui.label(""); });
                        header.col(|ui| { ui.strong("NAME"); });
                        header.col(|ui| { ui.strong("SIZE"); });
                        header.col(|ui| { ui.strong("MODIFIED"); });
                    })
                    .body(|body| {
                        body.rows(22.0, entries.len(), |mut row| {
                            let idx = row.index();
                            let entry = &entries[idx];

                            row.col(|ui| {
                                let mut checked = selected_snapshot.contains(&entry.path);
                                if ui.checkbox(&mut checked, "").changed() {
                                    toggle_selection.push((entry.path.clone(), checked));
                                }
                            });

                            row.col(|ui| {
                                let icon = if entry.is_dir { "d/" } else { " -" };
                                let is_sel = selected_snapshot.contains(&entry.path);
                                let label = format!("{} {}", icon, entry.name);

                                let response = ui.selectable_label(is_sel, &label);

                                if response.clicked() {
                                    if entry.is_dir {
                                        navigate_path = Some(entry.path.clone());
                                    } else {
                                        toggle_selection.push((entry.path.clone(), !is_sel));
                                    }
                                }

                                response.context_menu(|ui| {
                                    if !entry.is_dir {
                                        if ui.button("[get]").clicked() {
                                            if let Some(dir) = dirs::download_dir() {
                                                let local = dir.join(&entry.name);
                                                download_single.push((
                                                    entry.path.clone(),
                                                    local.to_string_lossy().to_string(),
                                                    entry.size,
                                                ));
                                            }
                                            ui.close_menu();
                                        }
                                    }
                                    if entry.is_dir {
                                        if ui.button("[open]").clicked() {
                                            navigate_path = Some(entry.path.clone());
                                            ui.close_menu();
                                        }
                                    }
                                    ui.separator();
                                    if ui.button("[rm]").clicked() {
                                        delete_path = Some(entry.path.clone());
                                        ui.close_menu();
                                    }
                                });
                            });

                            row.col(|ui| {
                                if !entry.is_dir {
                                    ui.label(format_size(entry.size));
                                }
                            });

                            row.col(|ui| {
                                if let Some(ts) = entry.modified {
                                    ui.label(format_timestamp(ts));
                                }
                            });
                        });
                    });
            });

        // Drag & drop overlay
        let hovering = ui.ctx().input(|i| !i.raw.hovered_files.is_empty());
        if hovering {
            let rect = ui.max_rect();
            ui.painter().rect_filled(
                rect,
                0.0,
                egui::Color32::from_rgba_premultiplied(0, 30, 0, 80),
            );
            ui.painter().rect_stroke(
                rect.shrink(4.0),
                0.0,
                egui::Stroke::new(1.0, crate::theme::GREEN),
            );
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "[ DROP FILES TO UPLOAD ]",
                egui::FontId::monospace(16.0),
                crate::theme::GREEN_BRIGHT,
            );
        }

        // Deferred actions
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
        for (remote, local, size) in download_single {
            self.download(&remote, &local, size);
        }

        // Mkdir dialog
        if self.show_mkdir_dialog {
            egui::Window::new("mkdir")
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    ui.horizontal(|ui| {
                        ui.label("name:");
                        ui.text_edit_singleline(&mut self.mkdir_name);
                    });
                    ui.horizontal(|ui| {
                        if ui.button("[create]").clicked() && !self.mkdir_name.is_empty() {
                            let full_path = format!(
                                "{}/{}",
                                current_path.trim_end_matches('/'),
                                self.mkdir_name
                            );
                            self.mkdir(&full_path);
                            self.show_mkdir_dialog = false;
                        }
                        if ui.button("[cancel]").clicked() {
                            self.show_mkdir_dialog = false;
                        }
                    });
                });
        }
    }

    fn download_selected(&mut self) {
        if let Some(dir) = dirs::download_dir() {
            let selected: Vec<_> = self
                .entries
                .iter()
                .filter(|e| self.selected.contains(&e.path))
                .map(|e| (e.path.clone(), e.name.clone(), e.size))
                .collect();
            for (path, name, size) in &selected {
                let local = dir.join(name);
                self.download(path, &local.to_string_lossy(), *size);
            }
            self.selected.clear();
        } else {
            self.error = Some("cannot determine downloads dir".to_string());
        }
    }

    fn upload_via_dialog(&mut self) {
        let dialog = rfd::FileDialog::new().set_title("Select files to upload");

        if let Some(files) = dialog.pick_files() {
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
            }
        }
    }
}

// ── Background async SFTP thread ──

async fn sftp_thread_async(
    config: &SessionConfig,
    mut req_rx: tokio::sync::mpsc::UnboundedReceiver<SftpRequest>,
    resp_tx: &mpsc::Sender<SftpResponse>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let session = create_russh_session(config, SshHandler::new()).await?;

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
            SftpRequest::Download {
                remote,
                local,
                progress,
            } => {
                match download_chunked(&sftp, &remote, &local, &progress).await {
                    Ok(()) => {
                        progress.done.store(true, Ordering::Relaxed);
                        let _ =
                            resp_tx.send(SftpResponse::Success(format!("OK: get {}", remote)));
                    }
                    Err(e) => {
                        progress.failed.store(true, Ordering::Relaxed);
                        let _ = resp_tx.send(SftpResponse::Error(e.to_string()));
                    }
                }
            }
            SftpRequest::Upload {
                local,
                remote,
                progress,
            } => {
                match upload_chunked(&sftp, &local, &remote, &progress).await {
                    Ok(()) => {
                        progress.done.store(true, Ordering::Relaxed);
                        let _ =
                            resp_tx.send(SftpResponse::Success(format!("OK: put {}", remote)));
                    }
                    Err(e) => {
                        progress.failed.store(true, Ordering::Relaxed);
                        let _ = resp_tx.send(SftpResponse::Error(e.to_string()));
                    }
                }
            }
            SftpRequest::Mkdir(path) => match sftp.create_dir(&path).await {
                Ok(()) => {
                    let _ = resp_tx.send(SftpResponse::Success(format!("OK: mkdir {}", path)));
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
                        let _ = resp_tx.send(SftpResponse::Success(format!("OK: rm {}", path)));
                    }
                    Err(e) => {
                        let _ = resp_tx.send(SftpResponse::Error(e.to_string()));
                    }
                }
            }
            SftpRequest::Rename { from, to } => match sftp.rename(&from, &to).await {
                Ok(()) => {
                    let _ = resp_tx.send(SftpResponse::Success(format!(
                        "OK: mv {} -> {}",
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
                t.duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|d| d.as_secs())
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

async fn download_chunked(
    sftp: &russh_sftp::client::SftpSession,
    remote: &str,
    local: &str,
    progress: &TransferState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut remote_file = sftp
        .open(remote)
        .await
        .map_err(|e| format!("open remote '{}': {}", remote, e))?;

    if progress.total.load(Ordering::Relaxed) == 0 {
        if let Ok(meta) = remote_file.metadata().await {
            progress.total.store(meta.len(), Ordering::Relaxed);
        }
    }

    // Ensure the local parent directory exists
    if let Some(parent) = std::path::Path::new(local).parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("create local dir '{}': {}", parent.display(), e))?;
    }

    let mut local_file = tokio::fs::File::create(local)
        .await
        .map_err(|e| format!("create local file '{}': {}", local, e))?;

    let mut buf = vec![0u8; CHUNK_SIZE];
    let mut total_read: u64 = 0;

    loop {
        let n = remote_file
            .read(&mut buf)
            .await
            .map_err(|e| format!("read remote '{}' at offset {}: {}", remote, total_read, e))?;
        if n == 0 {
            break;
        }
        local_file.write_all(&buf[..n]).await?;
        total_read += n as u64;
        progress.transferred.store(total_read, Ordering::Relaxed);
    }

    local_file.flush().await?;
    // Explicitly close the remote SFTP file handle
    remote_file.shutdown().await.ok();
    Ok(())
}

async fn upload_chunked(
    sftp: &russh_sftp::client::SftpSession,
    local: &str,
    remote: &str,
    progress: &TransferState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut local_file = tokio::fs::File::open(local)
        .await
        .map_err(|e| format!("open local '{}': {}", local, e))?;
    let meta = local_file.metadata().await?;
    let file_size = meta.len();
    progress.total.store(file_size, Ordering::Relaxed);

    let mut remote_file = sftp
        .create(remote)
        .await
        .map_err(|e| format!("create remote '{}': {}", remote, e))?;

    let mut buf = vec![0u8; CHUNK_SIZE];
    let mut total_written: u64 = 0;

    loop {
        let n = local_file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        remote_file
            .write_all(&buf[..n])
            .await
            .map_err(|e| format!("write remote '{}' at offset {}: {}", remote, total_written, e))?;
        total_written += n as u64;
        progress.transferred.store(total_written, Ordering::Relaxed);
    }

    remote_file.shutdown().await?;
    Ok(())
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}K", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1}M", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2}G", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
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
