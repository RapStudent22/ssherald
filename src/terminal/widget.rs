use crate::ssh::session::SshConnection;
use crate::terminal::emulator::{Cell, TermColor, TerminalEmulator};

// --- Выделение текста ---

#[derive(Clone)]
struct Selection {
    start_row: usize,
    start_col: usize,
    end_row: usize,
    end_col: usize,
}

impl Selection {
    /// Нормализация: начало <= конец
    fn normalized(&self) -> ((usize, usize), (usize, usize)) {
        if (self.start_row, self.start_col) <= (self.end_row, self.end_col) {
            ((self.start_row, self.start_col), (self.end_row, self.end_col))
        } else {
            ((self.end_row, self.end_col), (self.start_row, self.start_col))
        }
    }

    fn is_empty(&self) -> bool {
        self.start_row == self.end_row && self.start_col == self.end_col
    }

    fn contains(&self, row: usize, col: usize) -> bool {
        let ((sr, sc), (er, ec)) = self.normalized();
        if row < sr || row > er {
            return false;
        }
        if sr == er {
            return col >= sc && col <= ec;
        }
        if row == sr {
            return col >= sc;
        }
        if row == er {
            return col <= ec;
        }
        true
    }
}

// --- Виджет терминала ---

pub struct TerminalWidget {
    pub emulator: TerminalEmulator,
    focus: bool,
    font_size: f32,
    last_cols: usize,
    last_rows: usize,
    // Выделение
    selection: Option<Selection>,
    selection_anchor: Option<(usize, usize)>,
    selecting: bool,
}

impl TerminalWidget {
    pub fn new(cols: usize, rows: usize) -> Self {
        TerminalWidget {
            emulator: TerminalEmulator::new(cols, rows),
            focus: true,
            font_size: 14.0,
            last_cols: cols,
            last_rows: rows,
            selection: None,
            selection_anchor: None,
            selecting: false,
        }
    }

    /// Вычитываем все доступные данные из SSH и отдаём эмулятору
    pub fn process_ssh_output(&mut self, ssh: &SshConnection) {
        while let Ok(data) = ssh.output_rx.try_recv() {
            self.emulator.process(&data);
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, ssh: &SshConnection) {
        self.process_ssh_output(ssh);

        let cell_size = self.calculate_cell_size(ui);
        let available = ui.available_size();

        let new_cols = ((available.x / cell_size.x) as usize).max(1);
        let new_rows = ((available.y / cell_size.y) as usize).max(1);

        if new_cols != self.last_cols || new_rows != self.last_rows {
            self.emulator.resize(new_cols, new_rows);
            ssh.resize(new_cols as u32, new_rows as u32);
            self.last_cols = new_cols;
            self.last_rows = new_rows;
        }

        let desired_size = egui::vec2(
            new_cols as f32 * cell_size.x,
            new_rows as f32 * cell_size.y,
        );
        let (response, painter) =
            ui.allocate_painter(desired_size, egui::Sense::click_and_drag());

        let origin = response.rect.min;
        let bg_color = egui::Color32::from_rgb(40, 42, 54);
        let selection_bg = egui::Color32::from_rgba_premultiplied(100, 120, 220, 100);

        // Фон
        painter.rect_filled(response.rect, 0.0, bg_color);

        // Рендер сетки терминала
        {
            let visible = self.emulator.visible_rows();

            for (row_idx, row) in visible.iter().enumerate() {
                if row_idx >= new_rows {
                    break;
                }

                let mut job = egui::text::LayoutJob::default();

                for (col_idx, cell) in row.iter().enumerate() {
                    if col_idx >= new_cols {
                        break;
                    }

                    let (fg, cell_bg) = resolve_colors(cell, bg_color);
                    let text = if cell.c < ' ' || cell.c == '\0' {
                        " ".to_string()
                    } else {
                        cell.c.to_string()
                    };

                    let mut format = egui::TextFormat {
                        font_id: egui::FontId::monospace(self.font_size),
                        color: fg,
                        ..Default::default()
                    };

                    // Подсветка выделения
                    if let Some(sel) = &self.selection {
                        if !sel.is_empty() && sel.contains(row_idx, col_idx) {
                            format.background = selection_bg;
                        } else if cell_bg != bg_color {
                            format.background = cell_bg;
                        }
                    } else if cell_bg != bg_color {
                        format.background = cell_bg;
                    }

                    if cell.attr.underline {
                        format.underline = egui::Stroke::new(1.0, fg);
                    }
                    if cell.attr.italic {
                        format.italics = true;
                    }

                    job.append(&text, 0.0, format);
                }

                let galley = ui.fonts(|f| f.layout_job(job));
                painter.galley(
                    egui::pos2(origin.x, origin.y + row_idx as f32 * cell_size.y),
                    galley,
                    egui::Color32::TRANSPARENT,
                );
            }

        }
        // -- grid borrow released --

        // Курсор — вычисляем X-позицию через LayoutJob (тот же подход, что и рендер),
        // чтобы позиция курсора точно совпадала с позицией символов.
        {
            let grid = self.emulator.grid();
            let (cursor_row, cursor_col, cursor_visible) = self.emulator.cursor();

            if cursor_visible && self.focus && !self.emulator.is_scrolled() && cursor_row < new_rows && cursor_col <= new_cols {
                let font_id = egui::FontId::monospace(self.font_size);

                let cursor_x = if cursor_row < grid.len() && cursor_col > 0 {
                    let row = &grid[cursor_row];
                    let mut job = egui::text::LayoutJob::default();
                    for col in 0..cursor_col.min(row.len()).min(new_cols) {
                        let c = row[col].c;
                        let text = if c < ' ' || c == '\0' {
                            " ".to_string()
                        } else {
                            c.to_string()
                        };
                        job.append(
                            &text,
                            0.0,
                            egui::TextFormat {
                                font_id: font_id.clone(),
                                color: egui::Color32::WHITE,
                                ..Default::default()
                            },
                        );
                    }
                    let g = ui.fonts(|f| f.layout_job(job));
                    g.rect.width()
                } else {
                    0.0
                };

                let cursor_rect = egui::Rect::from_min_size(
                    egui::pos2(
                        origin.x + cursor_x,
                        origin.y + cursor_row as f32 * cell_size.y,
                    ),
                    cell_size,
                );

                let time = ui.input(|i| i.time);
                let blink = (time * 2.5) as i64 % 2 == 0;
                if blink {
                    painter.rect_filled(
                        cursor_rect,
                        0.0,
                        egui::Color32::from_rgba_premultiplied(200, 200, 200, 180),
                    );
                }
            }
        }

        // Обработка мыши (выделение)
        self.handle_mouse(ui, &response, origin, cell_size, new_rows, new_cols);

        // Фокус по клику (без drag)
        if response.clicked() && self.selection.is_none() {
            self.focus = true;
        }

        // Клавиатурный ввод
        if self.focus {
            self.handle_input(ui, ssh);
        }

        // Контекстное меню (ПКМ)
        response.context_menu(|ui| {
            let has_sel = self
                .selection
                .as_ref()
                .map_or(false, |s| !s.is_empty());

            if ui
                .add_enabled(has_sel, egui::Button::new("📋 Копировать  Ctrl+Shift+C"))
                .clicked()
            {
                let text = self.get_selected_text();
                if !text.is_empty() {
                    ui.ctx().copy_text(text);
                }
                self.selection = None;
                ui.close_menu();
            }
            if ui.button("📋 Вставить  Ctrl+Shift+V").clicked() {
                if let Ok(mut clipboard) = arboard::Clipboard::new() {
                    if let Ok(text) = clipboard.get_text() {
                        ssh.send(text.as_bytes());
                    }
                }
                ui.close_menu();
            }
        });

        // Скролл колёсиком (пропорционально)
        if response.hovered() {
            let scroll_delta = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll_delta.abs() > 1.0 {
                let lines = (scroll_delta.abs() / cell_size.y).ceil().max(1.0) as usize;
                if scroll_delta > 0.0 {
                    self.emulator.scroll_up_view(lines);
                } else {
                    self.emulator.scroll_down_view(lines);
                }
                self.selection = None;
            }
        }

        // Скроллбар
        let scrollback_len = self.emulator.scrollback_len();
        if scrollback_len > 0 {
            let scrollbar_width = 6.0;
            let scrollbar_x = response.rect.right() - scrollbar_width - 2.0;
            let scrollbar_height = response.rect.height();

            // Фон скроллбара
            painter.rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(scrollbar_x, origin.y),
                    egui::vec2(scrollbar_width, scrollbar_height),
                ),
                3.0,
                egui::Color32::from_rgba_premultiplied(50, 50, 70, 80),
            );

            // Бегунок
            let total_lines = scrollback_len + new_rows;
            let visible_fraction = new_rows as f32 / total_lines as f32;
            let thumb_height = (scrollbar_height * visible_fraction).max(20.0);
            let max_offset = scrollback_len.max(1) as f32;
            let scroll_fraction = self.emulator.scroll_offset() as f32 / max_offset;
            let thumb_y =
                origin.y + (scrollbar_height - thumb_height) * (1.0 - scroll_fraction);

            painter.rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(scrollbar_x, thumb_y),
                    egui::vec2(scrollbar_width, thumb_height),
                ),
                3.0,
                egui::Color32::from_rgba_premultiplied(140, 160, 220, 180),
            );
        }

        // Индикатор прокрутки
        if self.emulator.is_scrolled() {
            let text = format!("↑ {} строк назад", self.emulator.scroll_offset());
            let indicator_rect = egui::Rect::from_min_size(
                egui::pos2(
                    response.rect.center().x - 90.0,
                    response.rect.bottom() - 30.0,
                ),
                egui::vec2(180.0, 26.0),
            );
            painter.rect_filled(
                indicator_rect,
                13.0,
                egui::Color32::from_rgba_premultiplied(60, 70, 110, 220),
            );
            painter.text(
                indicator_rect.center(),
                egui::Align2::CENTER_CENTER,
                &text,
                egui::FontId::proportional(12.0),
                egui::Color32::from_rgb(200, 210, 255),
            );
        }
    }

    // --- Расчёт размера ячейки ---
    // Для определения кол-ва колонок/строк и мышиных координат.
    // Точная X-позиция курсора вычисляется отдельно через LayoutJob.
    fn calculate_cell_size(&self, ui: &egui::Ui) -> egui::Vec2 {
        let font_id = egui::FontId::monospace(self.font_size);
        // Усредняем по 10 символам для стабильного результата
        let g = ui.fonts(|f| {
            f.layout_no_wrap(
                "MMMMMMMMMM".to_string(),
                font_id.clone(),
                egui::Color32::WHITE,
            )
        });
        let char_width = g.rect.width() / 10.0;
        let line_height = g.rect.height();
        egui::vec2(char_width.max(1.0), line_height.max(1.0))
    }

    // --- Мышь: выделение ---
    fn handle_mouse(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        origin: egui::Pos2,
        cell_size: egui::Vec2,
        max_rows: usize,
        max_cols: usize,
    ) {
        let primary_pressed = ui.input(|i| i.pointer.primary_pressed());
        let primary_down = ui.input(|i| i.pointer.primary_down());

        // Начало выделения
        if primary_pressed {
            if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                if response.rect.contains(pos) {
                    let (row, col) = pos_to_cell(pos, origin, cell_size, max_rows, max_cols);
                    self.selection_anchor = Some((row, col));
                    self.selection = None;
                    self.selecting = true;
                    self.focus = true;
                }
            }
        }

        // Обновление выделения при drag
        if self.selecting && primary_down {
            if let Some(pos) = ui.input(|i| i.pointer.latest_pos()) {
                let (row, col) = pos_to_cell(pos, origin, cell_size, max_rows, max_cols);
                if let Some((ar, ac)) = self.selection_anchor {
                    if ar != row || ac != col {
                        self.selection = Some(Selection {
                            start_row: ar,
                            start_col: ac,
                            end_row: row,
                            end_col: col,
                        });
                    }
                }
            }
        }

        // Конец выделения
        if self.selecting && !primary_down {
            self.selecting = false;
        }
    }

    // --- Получение выделенного текста ---
    fn get_selected_text(&self) -> String {
        let sel = match &self.selection {
            Some(s) if !s.is_empty() => s,
            _ => return String::new(),
        };

        let ((sr, sc), (er, ec)) = sel.normalized();
        let visible = self.emulator.visible_rows();
        let mut lines = Vec::new();

        for row in sr..=er {
            if row >= visible.len() {
                break;
            }
            let line = visible[row];
            let col_start = if row == sr { sc } else { 0 };
            let col_end = if row == er {
                ec.min(line.len().saturating_sub(1))
            } else {
                line.len().saturating_sub(1)
            };

            let mut line_text = String::new();
            for col in col_start..=col_end {
                if col < line.len() {
                    let c = line[col].c;
                    line_text.push(if c == '\0' { ' ' } else { c });
                }
            }
            lines.push(line_text.trim_end().to_string());
        }

        lines.join("\n")
    }

    // --- Клавиатурный ввод ---
    fn handle_input(&mut self, ui: &egui::Ui, ssh: &SshConnection) {
        let events = ui.input(|i| i.events.clone());

        // Флаги для предотвращения двойной отправки —
        // egui может генерировать И семантическое событие (Cut/Copy/Paste),
        // И Event::Key для одного нажатия.
        let mut handled_cut = false;
        let mut handled_copy = false;
        let mut handled_paste = false;

        for event in &events {
            match event {
                // --- Семантические события egui (Ctrl+X/C/V) ---

                egui::Event::Cut => {
                    // Ctrl+X → отправляем байт 24 (используется в nano, etc.)
                    self.emulator.reset_scroll();
                    ssh.send(&[24]);
                    handled_cut = true;
                    self.selection = None;
                }
                egui::Event::Copy => {
                    // Ctrl+C → если есть выделение — копируем; иначе SIGINT (байт 3)
                    let text = self.get_selected_text();
                    if !text.is_empty() {
                        ui.ctx().copy_text(text);
                        self.selection = None;
                    } else {
                        ssh.send(&[3]);
                    }
                    handled_copy = true;
                }
                egui::Event::Paste(text) => {
                    self.emulator.reset_scroll();
                    ssh.send(text.as_bytes());
                    self.selection = None;
                    handled_paste = true;
                }

                // --- Обычный текстовый ввод ---
                egui::Event::Text(text) => {
                    self.emulator.reset_scroll();
                    ssh.send(text.as_bytes());
                    self.selection = None;
                }

                // --- Клавиши ---
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    // Shift+PageUp/Down — прокрутка терминала
                    if modifiers.shift && *key == egui::Key::PageUp {
                        self.emulator.scroll_up_view(self.last_rows / 2);
                        continue;
                    }
                    if modifiers.shift && *key == egui::Key::PageDown {
                        self.emulator.scroll_down_view(self.last_rows / 2);
                        continue;
                    }

                    // Ctrl+Shift+C — копирование выделения
                    if modifiers.ctrl && modifiers.shift && *key == egui::Key::C {
                        if !handled_copy {
                            let text = self.get_selected_text();
                            if !text.is_empty() {
                                ui.ctx().copy_text(text);
                                self.selection = None;
                            }
                        }
                        continue;
                    }
                    // Ctrl+Shift+V — вставка через arboard
                    if modifiers.ctrl && modifiers.shift && *key == egui::Key::V {
                        if !handled_paste {
                            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                if let Ok(text) = clipboard.get_text() {
                                    ssh.send(text.as_bytes());
                                }
                            }
                        }
                        self.selection = None;
                        continue;
                    }

                    // Ctrl+C / Ctrl+X / Ctrl+V без Shift —
                    // пропускаем, если уже обработано семантическим событием;
                    // иначе отправляем как fallback.
                    if modifiers.ctrl && !modifiers.shift {
                        match key {
                            egui::Key::C => {
                                if !handled_copy {
                                    let text = self.get_selected_text();
                                    if !text.is_empty() {
                                        ui.ctx().copy_text(text);
                                        self.selection = None;
                                    } else {
                                        ssh.send(&[3]);
                                    }
                                    handled_copy = true;
                                }
                                continue;
                            }
                            egui::Key::X => {
                                if !handled_cut {
                                    ssh.send(&[24]);
                                    handled_cut = true;
                                }
                                continue;
                            }
                            egui::Key::V => {
                                // Обработано через Event::Paste
                                continue;
                            }
                            _ => {}
                        }
                    }

                    if let Some(bytes) = self.key_to_bytes(*key, *modifiers) {
                        self.emulator.reset_scroll();
                        ssh.send(&bytes);
                        self.selection = None;
                    }
                }
                _ => {}
            }
        }
    }

    fn key_to_bytes(&self, key: egui::Key, modifiers: egui::Modifiers) -> Option<Vec<u8>> {
        let app_mode = self.emulator.app_cursor_keys();

        // Ctrl+Key (без Shift) — отправляем control byte
        if modifiers.ctrl && !modifiers.shift {
            let ctrl_byte: Option<u8> = match key {
                egui::Key::A => Some(1),
                egui::Key::B => Some(2),
                egui::Key::C => Some(3),
                egui::Key::D => Some(4),
                egui::Key::E => Some(5),
                egui::Key::F => Some(6),
                egui::Key::G => Some(7),
                egui::Key::H => Some(8),
                egui::Key::I => Some(9),
                egui::Key::J => Some(10),
                egui::Key::K => Some(11),
                egui::Key::L => Some(12),
                egui::Key::M => Some(13),
                egui::Key::N => Some(14),
                egui::Key::O => Some(15),
                egui::Key::P => Some(16),
                egui::Key::Q => Some(17),
                egui::Key::R => Some(18),
                egui::Key::S => Some(19),
                egui::Key::T => Some(20),
                egui::Key::U => Some(21),
                // V пропущена — Ctrl+V = paste
                egui::Key::W => Some(23),
                egui::Key::X => Some(24),
                egui::Key::Y => Some(25),
                egui::Key::Z => Some(26),
                _ => None,
            };
            if let Some(b) = ctrl_byte {
                return Some(vec![b]);
            }
        }

        match key {
            egui::Key::Enter => Some(b"\r".to_vec()),
            egui::Key::Tab => Some(b"\t".to_vec()),
            egui::Key::Backspace => Some(vec![127]),
            egui::Key::Escape => Some(vec![27]),
            egui::Key::ArrowUp => {
                if app_mode {
                    Some(b"\x1bOA".to_vec())
                } else {
                    Some(b"\x1b[A".to_vec())
                }
            }
            egui::Key::ArrowDown => {
                if app_mode {
                    Some(b"\x1bOB".to_vec())
                } else {
                    Some(b"\x1b[B".to_vec())
                }
            }
            egui::Key::ArrowRight => {
                if app_mode {
                    Some(b"\x1bOC".to_vec())
                } else {
                    Some(b"\x1b[C".to_vec())
                }
            }
            egui::Key::ArrowLeft => {
                if app_mode {
                    Some(b"\x1bOD".to_vec())
                } else {
                    Some(b"\x1b[D".to_vec())
                }
            }
            egui::Key::Home => Some(b"\x1b[H".to_vec()),
            egui::Key::End => Some(b"\x1b[F".to_vec()),
            egui::Key::PageUp => Some(b"\x1b[5~".to_vec()),
            egui::Key::PageDown => Some(b"\x1b[6~".to_vec()),
            egui::Key::Insert => Some(b"\x1b[2~".to_vec()),
            egui::Key::Delete => Some(b"\x1b[3~".to_vec()),
            egui::Key::F1 => Some(b"\x1bOP".to_vec()),
            egui::Key::F2 => Some(b"\x1bOQ".to_vec()),
            egui::Key::F3 => Some(b"\x1bOR".to_vec()),
            egui::Key::F4 => Some(b"\x1bOS".to_vec()),
            egui::Key::F5 => Some(b"\x1b[15~".to_vec()),
            egui::Key::F6 => Some(b"\x1b[17~".to_vec()),
            egui::Key::F7 => Some(b"\x1b[18~".to_vec()),
            egui::Key::F8 => Some(b"\x1b[19~".to_vec()),
            egui::Key::F9 => Some(b"\x1b[20~".to_vec()),
            egui::Key::F10 => Some(b"\x1b[21~".to_vec()),
            egui::Key::F11 => Some(b"\x1b[23~".to_vec()),
            egui::Key::F12 => Some(b"\x1b[24~".to_vec()),
            _ => None,
        }
    }
}

// --- Вспомогательные функции (standalone, без &self, чтобы не конфликтовать с borrow) ---

fn resolve_colors(cell: &Cell, bg_default: egui::Color32) -> (egui::Color32, egui::Color32) {
    let mut fg = term_color_to_egui(cell.attr.fg, true, cell.attr.bold);
    let mut bg = term_color_to_egui(cell.attr.bg, false, false);

    if cell.attr.inverse {
        std::mem::swap(&mut fg, &mut bg);
    }

    if cell.attr.bg == TermColor::Default && !cell.attr.inverse {
        bg = bg_default;
    }

    (fg, bg)
}

fn term_color_to_egui(color: TermColor, is_fg: bool, is_bold: bool) -> egui::Color32 {
    match color {
        TermColor::Default => {
            if is_fg {
                egui::Color32::from_rgb(248, 248, 242)
            } else {
                egui::Color32::from_rgb(40, 42, 54)
            }
        }
        TermColor::Indexed(idx) => {
            let effective_idx = if is_bold && idx < 8 { idx + 8 } else { idx };
            indexed_color(effective_idx)
        }
        TermColor::Rgb(r, g, b) => egui::Color32::from_rgb(r, g, b),
    }
}

fn pos_to_cell(
    pos: egui::Pos2,
    origin: egui::Pos2,
    cell_size: egui::Vec2,
    max_rows: usize,
    max_cols: usize,
) -> (usize, usize) {
    let col = ((pos.x - origin.x) / cell_size.x).max(0.0) as usize;
    let row = ((pos.y - origin.y) / cell_size.y).max(0.0) as usize;
    (
        row.min(max_rows.saturating_sub(1)),
        col.min(max_cols.saturating_sub(1)),
    )
}

/// Цветовая палитра Dracula (16 базовых + 256 расширенных)
fn indexed_color(idx: u8) -> egui::Color32 {
    match idx {
        0 => egui::Color32::from_rgb(40, 42, 54),
        1 => egui::Color32::from_rgb(255, 85, 85),
        2 => egui::Color32::from_rgb(80, 250, 123),
        3 => egui::Color32::from_rgb(241, 250, 140),
        4 => egui::Color32::from_rgb(189, 147, 249),
        5 => egui::Color32::from_rgb(255, 121, 198),
        6 => egui::Color32::from_rgb(139, 233, 253),
        7 => egui::Color32::from_rgb(248, 248, 242),
        8 => egui::Color32::from_rgb(98, 114, 164),
        9 => egui::Color32::from_rgb(255, 110, 110),
        10 => egui::Color32::from_rgb(105, 255, 148),
        11 => egui::Color32::from_rgb(255, 255, 165),
        12 => egui::Color32::from_rgb(214, 172, 255),
        13 => egui::Color32::from_rgb(255, 146, 223),
        14 => egui::Color32::from_rgb(164, 255, 255),
        15 => egui::Color32::from_rgb(255, 255, 255),
        16..=231 => {
            let n = idx - 16;
            let r_comp = n / 36;
            let g_comp = (n % 36) / 6;
            let b_comp = n % 6;
            let r = if r_comp > 0 { 55 + r_comp * 40 } else { 0 };
            let g = if g_comp > 0 { 55 + g_comp * 40 } else { 0 };
            let b = if b_comp > 0 { 55 + b_comp * 40 } else { 0 };
            egui::Color32::from_rgb(r, g, b)
        }
        232..=255 => {
            let v = 8 + (idx - 232) * 10;
            egui::Color32::from_rgb(v, v, v)
        }
    }
}
