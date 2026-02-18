use vte::{Params, Parser, Perform};

#[derive(Clone, Copy, PartialEq)]
pub enum TermColor {
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

#[derive(Clone, Copy)]
pub struct CellAttr {
    pub fg: TermColor,
    pub bg: TermColor,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

impl Default for CellAttr {
    fn default() -> Self {
        CellAttr {
            fg: TermColor::Default,
            bg: TermColor::Default,
            bold: false,
            italic: false,
            underline: false,
            inverse: false,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Cell {
    pub c: char,
    pub attr: CellAttr,
}

impl Default for Cell {
    fn default() -> Self {
        Cell {
            c: ' ',
            attr: CellAttr::default(),
        }
    }
}

pub struct TerminalEmulator {
    grid: Vec<Vec<Cell>>,
    cols: usize,
    rows: usize,
    cursor_row: usize,
    cursor_col: usize,
    cursor_visible: bool,
    saved_cursor: Option<(usize, usize, CellAttr)>,
    current_attr: CellAttr,
    scroll_top: usize,
    scroll_bottom: usize,
    parser: Parser,
    scrollback: Vec<Vec<Cell>>,
    scroll_offset: usize,
    alt_grid: Option<Vec<Vec<Cell>>>,
    alt_cursor: Option<(usize, usize)>,
    app_cursor_keys: bool,
    auto_wrap: bool,
    wrap_next: bool,
    tab_stops: Vec<bool>,
    #[allow(dead_code)]
    pending_data: Vec<u8>,
}

impl TerminalEmulator {
    pub fn new(cols: usize, rows: usize) -> Self {
        let grid = vec![vec![Cell::default(); cols]; rows];
        let mut tab_stops = vec![false; cols];
        for i in (0..cols).step_by(8) {
            tab_stops[i] = true;
        }

        TerminalEmulator {
            grid,
            cols,
            rows,
            cursor_row: 0,
            cursor_col: 0,
            cursor_visible: true,
            saved_cursor: None,
            current_attr: CellAttr::default(),
            scroll_top: 0,
            scroll_bottom: rows.saturating_sub(1),
            parser: Parser::new(),
            scrollback: Vec::new(),
            scroll_offset: 0,
            alt_grid: None,
            alt_cursor: None,
            app_cursor_keys: false,
            auto_wrap: true,
            wrap_next: false,
            tab_stops,
            pending_data: Vec::new(),
        }
    }

    #[allow(dead_code)]
    pub fn feed(&mut self, data: &[u8]) {
        self.pending_data.extend_from_slice(data);
    }

    #[allow(dead_code)]
    pub fn flush(&mut self) {
        let data = std::mem::take(&mut self.pending_data);
        // Извлекаем парсер, чтобы избежать двойного &mut self
        let mut parser = std::mem::replace(&mut self.parser, Parser::new());
        for &byte in &data {
            parser.advance(self, byte);
        }
        self.parser = parser;
    }

    /// Обработать данные напрямую (без буферизации)
    pub fn process(&mut self, data: &[u8]) {
        self.scroll_offset = 0;
        let mut parser = std::mem::replace(&mut self.parser, Parser::new());
        for &byte in data {
            parser.advance(self, byte);
        }
        self.parser = parser;
    }

    pub fn grid(&self) -> &[Vec<Cell>] {
        &self.grid
    }

    pub fn cursor(&self) -> (usize, usize, bool) {
        (self.cursor_row, self.cursor_col, self.cursor_visible)
    }

    #[allow(dead_code)]
    pub fn cols(&self) -> usize {
        self.cols
    }

    #[allow(dead_code)]
    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn app_cursor_keys(&self) -> bool {
        self.app_cursor_keys
    }

    pub fn scrollback_len(&self) -> usize {
        self.scrollback.len()
    }

    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    pub fn scroll_up_view(&mut self, lines: usize) {
        self.scroll_offset = (self.scroll_offset + lines).min(self.scrollback.len());
    }

    pub fn scroll_down_view(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
    }

    pub fn reset_scroll(&mut self) {
        self.scroll_offset = 0;
    }

    pub fn is_scrolled(&self) -> bool {
        self.scroll_offset > 0
    }

    /// Возвращает строки для отображения с учётом scroll_offset.
    /// При scroll_offset == 0 возвращает текущую сетку.
    /// При scroll_offset > 0 показывает строки из scrollback + часть сетки.
    pub fn visible_rows(&self) -> Vec<&Vec<Cell>> {
        if self.scroll_offset == 0 {
            return self.grid.iter().collect();
        }

        let total_lines = self.scrollback.len() + self.rows;
        let view_start = total_lines.saturating_sub(self.rows + self.scroll_offset);

        let mut result = Vec::with_capacity(self.rows);
        for i in 0..self.rows {
            let idx = view_start + i;
            if idx < self.scrollback.len() {
                result.push(&self.scrollback[idx]);
            } else {
                let grid_idx = idx - self.scrollback.len();
                if grid_idx < self.grid.len() {
                    result.push(&self.grid[grid_idx]);
                }
            }
        }
        result
    }

    pub fn resize(&mut self, new_cols: usize, new_rows: usize) {
        if new_cols == 0 || new_rows == 0 || (new_cols == self.cols && new_rows == self.rows) {
            return;
        }

        let mut new_grid = vec![vec![Cell::default(); new_cols]; new_rows];
        let copy_rows = new_rows.min(self.rows);
        let copy_cols = new_cols.min(self.cols);

        // Если новый экран меньше и курсор ниже видимой области — прокручиваем
        if self.cursor_row >= new_rows {
            let shift = self.cursor_row - new_rows + 1;
            for i in 0..shift {
                if i < self.rows {
                    self.scrollback.push(self.grid[i].clone());
                }
            }
            for r in 0..copy_rows {
                let src_row = r + shift;
                if src_row < self.rows {
                    for c in 0..copy_cols {
                        new_grid[r][c] = self.grid[src_row][c];
                    }
                }
            }
            self.cursor_row = new_rows - 1;
        } else {
            for r in 0..copy_rows {
                for c in 0..copy_cols {
                    if r < self.grid.len() && c < self.grid[r].len() {
                        new_grid[r][c] = self.grid[r][c];
                    }
                }
            }
        }

        self.grid = new_grid;
        self.cols = new_cols;
        self.rows = new_rows;
        self.scroll_top = 0;
        self.scroll_bottom = new_rows.saturating_sub(1);
        self.cursor_col = self.cursor_col.min(new_cols.saturating_sub(1));

        self.tab_stops = vec![false; new_cols];
        for i in (0..new_cols).step_by(8) {
            self.tab_stops[i] = true;
        }

    
    }

    // --- Внутренние методы ---

    fn scroll_up(&mut self) {
        let top = self.scroll_top;
        let bottom = self.scroll_bottom;

        if top == 0 && self.alt_grid.is_none() {
            self.scrollback.push(self.grid[0].clone());
        }

        for r in top..bottom {
            self.grid[r] = self.grid[r + 1].clone();
        }
        self.grid[bottom] = vec![Cell::default(); self.cols];
    }

    fn scroll_down(&mut self) {
        let top = self.scroll_top;
        let bottom = self.scroll_bottom;

        for r in (top + 1..=bottom).rev() {
            self.grid[r] = self.grid[r - 1].clone();
        }
        self.grid[top] = vec![Cell::default(); self.cols];
    }

    fn newline(&mut self) {
        if self.cursor_row == self.scroll_bottom {
            self.scroll_up();
        } else if self.cursor_row < self.rows.saturating_sub(1) {
            self.cursor_row += 1;
        }
    }

    fn put_char(&mut self, c: char) {
        if self.wrap_next {
            self.cursor_col = 0;
            self.newline();
            self.wrap_next = false;
        }

        if self.cursor_row < self.rows && self.cursor_col < self.cols {
            self.grid[self.cursor_row][self.cursor_col] = Cell {
                c,
                attr: self.current_attr,
            };
        }

        if self.cursor_col < self.cols.saturating_sub(1) {
            self.cursor_col += 1;
        } else if self.auto_wrap {
            self.wrap_next = true;
        }
    }

    fn erase_in_display(&mut self, mode: u16) {
        match mode {
            0 => {
                // Erase from cursor to end
                for c in self.cursor_col..self.cols {
                    self.grid[self.cursor_row][c] = Cell::default();
                }
                for r in (self.cursor_row + 1)..self.rows {
                    self.grid[r] = vec![Cell::default(); self.cols];
                }
            }
            1 => {
                // Erase from start to cursor
                for r in 0..self.cursor_row {
                    self.grid[r] = vec![Cell::default(); self.cols];
                }
                for c in 0..=self.cursor_col.min(self.cols.saturating_sub(1)) {
                    self.grid[self.cursor_row][c] = Cell::default();
                }
            }
            2 => {
                // Erase entire display
                for r in 0..self.rows {
                    self.grid[r] = vec![Cell::default(); self.cols];
                }
            }
            3 => {
                // Erase display + scrollback
                for r in 0..self.rows {
                    self.grid[r] = vec![Cell::default(); self.cols];
                }
                self.scrollback.clear();
                self.scroll_offset = 0;
            }
            _ => {}
        }
    }

    fn erase_in_line(&mut self, mode: u16) {
        if self.cursor_row >= self.rows {
            return;
        }
        match mode {
            0 => {
                for c in self.cursor_col..self.cols {
                    self.grid[self.cursor_row][c] = Cell::default();
                }
            }
            1 => {
                for c in 0..=self.cursor_col.min(self.cols.saturating_sub(1)) {
                    self.grid[self.cursor_row][c] = Cell::default();
                }
            }
            2 => {
                self.grid[self.cursor_row] = vec![Cell::default(); self.cols];
            }
            _ => {}
        }
    }

    fn handle_sgr(&mut self, params: &[u16]) {
        if params.is_empty() {
            self.current_attr = CellAttr::default();
            return;
        }

        let mut i = 0;
        while i < params.len() {
            match params[i] {
                0 => self.current_attr = CellAttr::default(),
                1 => self.current_attr.bold = true,
                2 => {} // dim — игнорируем
                3 => self.current_attr.italic = true,
                4 => self.current_attr.underline = true,
                7 => self.current_attr.inverse = true,
                21 | 22 => self.current_attr.bold = false,
                23 => self.current_attr.italic = false,
                24 => self.current_attr.underline = false,
                27 => self.current_attr.inverse = false,
                30..=37 => self.current_attr.fg = TermColor::Indexed((params[i] - 30) as u8),
                38 => {
                    i += 1;
                    if i < params.len() {
                        match params[i] {
                            5 => {
                                i += 1;
                                if i < params.len() {
                                    self.current_attr.fg =
                                        TermColor::Indexed(params[i] as u8);
                                }
                            }
                            2 => {
                                if i + 3 < params.len() {
                                    self.current_attr.fg = TermColor::Rgb(
                                        params[i + 1] as u8,
                                        params[i + 2] as u8,
                                        params[i + 3] as u8,
                                    );
                                    i += 3;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                39 => self.current_attr.fg = TermColor::Default,
                40..=47 => self.current_attr.bg = TermColor::Indexed((params[i] - 40) as u8),
                48 => {
                    i += 1;
                    if i < params.len() {
                        match params[i] {
                            5 => {
                                i += 1;
                                if i < params.len() {
                                    self.current_attr.bg =
                                        TermColor::Indexed(params[i] as u8);
                                }
                            }
                            2 => {
                                if i + 3 < params.len() {
                                    self.current_attr.bg = TermColor::Rgb(
                                        params[i + 1] as u8,
                                        params[i + 2] as u8,
                                        params[i + 3] as u8,
                                    );
                                    i += 3;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                49 => self.current_attr.bg = TermColor::Default,
                90..=97 => {
                    self.current_attr.fg = TermColor::Indexed((params[i] - 90 + 8) as u8)
                }
                100..=107 => {
                    self.current_attr.bg = TermColor::Indexed((params[i] - 100 + 8) as u8)
                }
                _ => {}
            }
            i += 1;
        }
    }

    fn enter_alt_screen(&mut self) {
        if self.alt_grid.is_none() {
            self.alt_grid = Some(std::mem::replace(
                &mut self.grid,
                vec![vec![Cell::default(); self.cols]; self.rows],
            ));
            self.alt_cursor = Some((self.cursor_row, self.cursor_col));
            self.cursor_row = 0;
            self.cursor_col = 0;
        }
    }

    fn exit_alt_screen(&mut self) {
        if let Some(grid) = self.alt_grid.take() {
            self.grid = grid;
            if let Some((row, col)) = self.alt_cursor.take() {
                self.cursor_row = row.min(self.rows.saturating_sub(1));
                self.cursor_col = col.min(self.cols.saturating_sub(1));
            }
        }
    }
}

impl Perform for TerminalEmulator {
    fn print(&mut self, c: char) {
        self.put_char(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            0x07 => {} // BEL
            0x08 => {
                // BS — backspace
                if self.cursor_col > 0 {
                    self.cursor_col -= 1;
                    self.wrap_next = false;
                }
            }
            0x09 => {
                // HT — tab
                let next_tab = (self.cursor_col + 1..self.cols)
                    .find(|&c| self.tab_stops.get(c).copied().unwrap_or(false))
                    .unwrap_or(self.cols.saturating_sub(1));
                self.cursor_col = next_tab;
                self.wrap_next = false;
            }
            0x0A | 0x0B | 0x0C => {
                // LF, VT, FF
                self.newline();
                self.wrap_next = false;
            }
            0x0D => {
                // CR
                self.cursor_col = 0;
                self.wrap_next = false;
            }
            _ => {}
        }
    }

    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _action: char) {}
    fn put(&mut self, _byte: u8) {}
    fn unhook(&mut self) {}
    fn osc_dispatch(&mut self, _params: &[&[u8]], _bell_terminated: bool) {}

    fn csi_dispatch(
        &mut self,
        params: &Params,
        intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        let flat_params: Vec<u16> = params
            .iter()
            .map(|p| p.first().copied().unwrap_or(0))
            .collect();

        let p1 = flat_params.first().copied().unwrap_or(0);
        let p2 = flat_params.get(1).copied().unwrap_or(0);

        let has_question = intermediates.contains(&b'?');

        match action {
            'A' => {
                // CUU — cursor up
                let n = if p1 == 0 { 1 } else { p1 as usize };
                self.cursor_row = self.cursor_row.saturating_sub(n);
                self.wrap_next = false;
            }
            'B' => {
                // CUD — cursor down
                let n = if p1 == 0 { 1 } else { p1 as usize };
                self.cursor_row = (self.cursor_row + n).min(self.rows.saturating_sub(1));
                self.wrap_next = false;
            }
            'C' => {
                // CUF — cursor forward
                let n = if p1 == 0 { 1 } else { p1 as usize };
                self.cursor_col = (self.cursor_col + n).min(self.cols.saturating_sub(1));
                self.wrap_next = false;
            }
            'D' => {
                // CUB — cursor backward
                let n = if p1 == 0 { 1 } else { p1 as usize };
                self.cursor_col = self.cursor_col.saturating_sub(n);
                self.wrap_next = false;
            }
            'E' => {
                // CNL — cursor next line
                let n = if p1 == 0 { 1 } else { p1 as usize };
                self.cursor_row = (self.cursor_row + n).min(self.rows.saturating_sub(1));
                self.cursor_col = 0;
            }
            'F' => {
                // CPL — cursor previous line
                let n = if p1 == 0 { 1 } else { p1 as usize };
                self.cursor_row = self.cursor_row.saturating_sub(n);
                self.cursor_col = 0;
            }
            'G' => {
                // CHA — cursor horizontal absolute
                let col = if p1 == 0 { 1 } else { p1 as usize };
                self.cursor_col = (col - 1).min(self.cols.saturating_sub(1));
                self.wrap_next = false;
            }
            'H' | 'f' => {
                // CUP — cursor position
                let row = if p1 == 0 { 1 } else { p1 as usize };
                let col = if p2 == 0 { 1 } else { p2 as usize };
                self.cursor_row = (row - 1).min(self.rows.saturating_sub(1));
                self.cursor_col = (col - 1).min(self.cols.saturating_sub(1));
                self.wrap_next = false;
            }
            'J' => {
                self.erase_in_display(p1);
            }
            'K' => {
                self.erase_in_line(p1);
            }
            'L' => {
                // IL — insert lines
                let n = if p1 == 0 { 1 } else { p1 as usize };
                for _ in 0..n {
                    if self.cursor_row <= self.scroll_bottom {
                        if self.scroll_bottom < self.grid.len() {
                            self.grid.remove(self.scroll_bottom);
                        }
                        self.grid
                            .insert(self.cursor_row, vec![Cell::default(); self.cols]);
                    }
                }
            }
            'M' => {
                // DL — delete lines
                let n = if p1 == 0 { 1 } else { p1 as usize };
                for _ in 0..n {
                    if self.cursor_row <= self.scroll_bottom {
                        self.grid.remove(self.cursor_row);
                        self.grid
                            .insert(self.scroll_bottom, vec![Cell::default(); self.cols]);
                    }
                }
            }
            'P' => {
                // DCH — delete characters
                let n = if p1 == 0 { 1 } else { p1 as usize };
                if self.cursor_row < self.rows {
                    let row = &mut self.grid[self.cursor_row];
                    for _ in 0..n.min(self.cols - self.cursor_col) {
                        if self.cursor_col < row.len() {
                            row.remove(self.cursor_col);
                            row.push(Cell::default());
                        }
                    }
                }
            }
            '@' => {
                // ICH — insert characters
                let n = if p1 == 0 { 1 } else { p1 as usize };
                if self.cursor_row < self.rows {
                    let row = &mut self.grid[self.cursor_row];
                    for _ in 0..n {
                        if row.len() >= self.cols {
                            row.pop();
                        }
                        row.insert(self.cursor_col, Cell::default());
                    }
                }
            }
            'S' => {
                // SU — scroll up
                let n = if p1 == 0 { 1 } else { p1 as usize };
                for _ in 0..n {
                    self.scroll_up();
                }
            }
            'T' if !has_question => {
                // SD — scroll down
                let n = if p1 == 0 { 1 } else { p1 as usize };
                for _ in 0..n {
                    self.scroll_down();
                }
            }
            'X' => {
                // ECH — erase characters
                let n = if p1 == 0 { 1 } else { p1 as usize };
                if self.cursor_row < self.rows {
                    for c in self.cursor_col..(self.cursor_col + n).min(self.cols) {
                        self.grid[self.cursor_row][c] = Cell::default();
                    }
                }
            }
            'd' => {
                // VPA — vertical position absolute
                let row = if p1 == 0 { 1 } else { p1 as usize };
                self.cursor_row = (row - 1).min(self.rows.saturating_sub(1));
                self.wrap_next = false;
            }
            'h' => {
                // SM — set mode
                if has_question {
                    for &p in &flat_params {
                        match p {
                            1 => self.app_cursor_keys = true,
                            7 => self.auto_wrap = true,
                            25 => self.cursor_visible = true,
                            47 | 1047 => self.enter_alt_screen(),
                            1049 => {
                                self.saved_cursor = Some((
                                    self.cursor_row,
                                    self.cursor_col,
                                    self.current_attr,
                                ));
                                self.enter_alt_screen();
                            }
                            _ => {}
                        }
                    }
                }
            }
            'l' => {
                // RM — reset mode
                if has_question {
                    for &p in &flat_params {
                        match p {
                            1 => self.app_cursor_keys = false,
                            7 => self.auto_wrap = false,
                            25 => self.cursor_visible = false,
                            47 | 1047 => self.exit_alt_screen(),
                            1049 => {
                                self.exit_alt_screen();
                                if let Some((row, col, attr)) = self.saved_cursor.take() {
                                    self.cursor_row = row.min(self.rows.saturating_sub(1));
                                    self.cursor_col = col.min(self.cols.saturating_sub(1));
                                    self.current_attr = attr;
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            'm' => {
                // SGR — select graphic rendition
                if flat_params.is_empty() {
                    self.current_attr = CellAttr::default();
                } else {
                    self.handle_sgr(&flat_params);
                }
            }
            'n' => {
                // DSR — device status report (игнорируем)
            }
            'r' => {
                // DECSTBM — set scroll region
                if !has_question {
                    let top = if p1 == 0 { 1 } else { p1 as usize };
                    let bottom = if p2 == 0 { self.rows } else { p2 as usize };
                    self.scroll_top = (top - 1).min(self.rows.saturating_sub(1));
                    self.scroll_bottom = (bottom - 1).min(self.rows.saturating_sub(1));
                    if self.scroll_top >= self.scroll_bottom {
                        self.scroll_top = 0;
                        self.scroll_bottom = self.rows.saturating_sub(1);
                    }
                    self.cursor_row = self.scroll_top;
                    self.cursor_col = 0;
                }
            }
            's' => {
                // SCOSC — save cursor
                if !has_question {
                    self.saved_cursor =
                        Some((self.cursor_row, self.cursor_col, self.current_attr));
                }
            }
            'u' => {
                // SCORC — restore cursor
                if let Some((row, col, attr)) = self.saved_cursor {
                    self.cursor_row = row.min(self.rows.saturating_sub(1));
                    self.cursor_col = col.min(self.cols.saturating_sub(1));
                    self.current_attr = attr;
                }
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, byte: u8) {
        match byte {
            b'7' => {
                // DECSC — save cursor
                self.saved_cursor =
                    Some((self.cursor_row, self.cursor_col, self.current_attr));
            }
            b'8' => {
                // DECRC — restore cursor
                if let Some((row, col, attr)) = self.saved_cursor {
                    self.cursor_row = row.min(self.rows.saturating_sub(1));
                    self.cursor_col = col.min(self.cols.saturating_sub(1));
                    self.current_attr = attr;
                }
            }
            b'D' => {
                // IND — index (move down, scroll if needed)
                self.newline();
            }
            b'E' => {
                // NEL — next line
                self.cursor_col = 0;
                self.newline();
            }
            b'M' => {
                // RI — reverse index
                if self.cursor_row == self.scroll_top {
                    self.scroll_down();
                } else if self.cursor_row > 0 {
                    self.cursor_row -= 1;
                }
            }
            b'c' => {
                // RIS — full reset
                let cols = self.cols;
                let rows = self.rows;
                *self = Self::new(cols, rows);
            }
            _ => {}
        }
    }
}
