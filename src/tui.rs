//! Bare `event`: a minimal event-creation form (ratatui + crossterm).
//!
//! Five fields: `title`, `date`, `time`, `duration`,
//! `context`. `date` must be today or later and well-formed: otherwise
//! a random confused ASCII face pops up above the field and its frame
//! turns red, live while typing and sticky on submit. No emoji.
//! `time` accepts only `HH:MM` (prefilled with now).
//! `Enter` submits from anywhere (so `context` is single-line input with
//! visual wrapping), `Esc` cancels without writing anything.

use std::io;

use chrono::{NaiveDate, NaiveTime};
use crossterm::{
    event::{self, Event as CrosstermEvent, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders, Paragraph},
};

use crate::calendar::PALETTE;
use crate::cli::today;
use crate::config::{Config, OldEventPolicy, field_comment};
use crate::event::{parse_duration, snap_quarter};

/// Data of a submitted form.
#[derive(Debug, Clone, PartialEq)]
pub struct TuiData {
    /// Becomes `## <title>`.
    pub title: String,
    /// Target day file.
    pub date: NaiveDate,
    /// Event time (`Time:`).
    pub time: NaiveTime,
    /// Raw duration text (`45m`, `2h`, `1h30m`).
    pub duration_raw: String,
    /// Free-form context.
    pub context: String,
}

/// How the TUI session ended.
#[derive(Debug)]
pub enum TuiOutcome {
    /// The user pressed `Enter` on valid input.
    Submitted(TuiData),
    /// The user pressed `Esc`/`Ctrl-C`.
    Cancelled,
}

/// Form fields in focus order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Title,
    Date,
    Time,
    Duration,
    Context,
}

impl Field {
    fn next(self) -> Self {
        match self {
            Self::Title => Self::Date,
            Self::Date => Self::Time,
            Self::Time => Self::Duration,
            Self::Duration => Self::Context,
            Self::Context => Self::Title,
        }
    }

    fn prev(self) -> Self {
        match self {
            Self::Title => Self::Context,
            Self::Date => Self::Title,
            Self::Time => Self::Date,
            Self::Duration => Self::Time,
            Self::Context => Self::Duration,
        }
    }
}

/// Form state. The pure part (keys/validation) is unit-tested below.
struct App {
    focus: Field,
    title: String,
    date_buf: String,
    time_buf: String,
    duration: String,
    context: String,
    /// Cursor as a char index into the focused buffer.
    cursor: usize,
    error: Option<String>,
    done: bool,
    cancelled: bool,
}

/// Current local time, the default event time.
fn now_time() -> NaiveTime {
    chrono::Local::now().time()
}

/// Strict `HH:MM` for the TUI time field. No smart formats (`930`, `now`, `+30`).
fn strict_time(raw: &str) -> Result<NaiveTime, chrono::ParseError> {
    NaiveTime::parse_from_str(raw.trim(), "%H:%M")
}

/// Confused ASCII faces (asciiart.eu, one-line faces): one pops up above
/// the date field when the date is past or malformed.
const DATE_FACES: [&str; 3] = ["<'(o.o)'>", "`o._.o'", "@(o),(o)"];

/// Deterministic face per date buffer so it stays steady across redraws.
fn face_for(buf: &str) -> &'static str {
    let idx = buf
        .bytes()
        .fold(0usize, |acc, b| acc.wrapping_add(b as usize));
    DATE_FACES[idx % DATE_FACES.len()]
}

/// Live date complaint: past dates and malformed buffers alike.
/// `None` means the date field is fine (for now).
fn live_date_error(buf: &str) -> Option<String> {
    match NaiveDate::parse_from_str(buf.trim(), "%Y-%m-%d") {
        Err(_) => Some(String::from("bad date: expected YYYY-MM-DD")),
        Ok(date) if date < today() => Some(format!(
            "date is in the past: {} is before today",
            date.format("%Y-%m-%d")
        )),
        Ok(_) => None,
    }
}

impl App {
    fn new() -> Self {
        Self {
            focus: Field::Title,
            title: String::new(),
            date_buf: today().format("%Y-%m-%d").to_string(),
            time_buf: now_time().format("%H:%M").to_string(),
            duration: String::new(),
            context: String::new(),
            cursor: 0,
            error: None,
            done: false,
            cancelled: false,
        }
    }

    fn buf(&self) -> &str {
        match self.focus {
            Field::Title => &self.title,
            Field::Date => &self.date_buf,
            Field::Time => &self.time_buf,
            Field::Duration => &self.duration,
            Field::Context => &self.context,
        }
    }

    fn buf_mut(&mut self) -> &mut String {
        match self.focus {
            Field::Title => &mut self.title,
            Field::Date => &mut self.date_buf,
            Field::Time => &mut self.time_buf,
            Field::Duration => &mut self.duration,
            Field::Context => &mut self.context,
        }
    }

    fn move_focus(&mut self, delta: i32) {
        self.focus = if delta >= 0 {
            self.focus.next()
        } else {
            self.focus.prev()
        };
        self.cursor = self.buf().chars().count();
    }

    fn shift_date(&mut self, days: i64) {
        match NaiveDate::parse_from_str(self.date_buf.trim(), "%Y-%m-%d")
            .ok()
            .and_then(|date| date.checked_add_signed(chrono::Duration::days(days)))
        {
            Some(next) => {
                self.date_buf = next.format("%Y-%m-%d").to_string();
                self.error = live_date_error(&self.date_buf);
            }
            None => self.error = Some(String::from("bad date: expected YYYY-MM-DD")),
        }
        self.cursor = self.buf().chars().count();
    }

    fn shift_time(&mut self, forward: bool) {
        let now = now_time();
        let base = strict_time(&self.time_buf).unwrap_or(now);
        self.time_buf = snap_quarter(base, forward).format("%H:%M").to_string();
        self.error = None;
        self.cursor = self.buf().chars().count();
    }

    /// Re-show (or clear) the date complaint while typing in the date field.
    fn refresh_date_error(&mut self) {
        if self.focus == Field::Date {
            self.error = live_date_error(self.buf());
        }
    }

    fn insert(&mut self, ch: char) {
        let byte: usize = self
            .buf()
            .chars()
            .take(self.cursor)
            .map(|c| c.len_utf8())
            .sum();
        self.buf_mut().insert(byte, ch);
        self.cursor += 1;
        self.error = None;
        self.refresh_date_error();
    }

    fn delete_before(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.cursor -= 1;
        let byte: usize = self
            .buf()
            .chars()
            .take(self.cursor)
            .map(|c| c.len_utf8())
            .sum();
        self.buf_mut().remove(byte);
        self.error = None;
        self.refresh_date_error();
    }

    fn move_cursor(&mut self, delta: isize) {
        let len = self.buf().chars().count() as isize;
        self.cursor = (self.cursor as isize + delta).clamp(0, len) as usize;
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
            self.cancelled = true;
            return;
        }
        match key.code {
            KeyCode::Esc => {
                self.cancelled = true;
            }
            KeyCode::Enter => self.submit(),
            KeyCode::Tab => self.move_focus(1),
            KeyCode::BackTab => self.move_focus(-1),
            KeyCode::Up => self.move_focus(-1),
            KeyCode::Down => self.move_focus(1),
            _ => self.edit_key(key),
        }
    }

    /// Keys that type or adjust the focused field.
    fn edit_key(&mut self, key: KeyEvent) {
        if self.focus == Field::Date {
            match key.code {
                KeyCode::Left | KeyCode::Char('h') => self.shift_date(-1),
                KeyCode::Right | KeyCode::Char('l') => self.shift_date(1),
                KeyCode::Char('j') => self.move_focus(1),
                KeyCode::Char('k') => self.move_focus(-1),
                KeyCode::Char(ch) if ch.is_ascii_digit() || ch == '-' => self.insert(ch),
                KeyCode::Backspace => self.delete_before(),
                _ => {}
            }
            return;
        }
        if self.focus == Field::Time {
            match key.code {
                KeyCode::Left => self.shift_time(false),
                KeyCode::Right => self.shift_time(true),
                KeyCode::Char('j') => self.move_focus(1),
                KeyCode::Char('k') => self.move_focus(-1),
                KeyCode::Backspace => self.delete_before(),
                KeyCode::Char(ch) => self.insert(ch),
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Left => self.move_cursor(-1),
            KeyCode::Right => self.move_cursor(1),
            KeyCode::Backspace => self.delete_before(),
            KeyCode::Char(ch) => self.insert(ch),
            _ => {}
        }
    }

    fn submit(&mut self) {
        self.error = None;
        if self.title.trim().is_empty() {
            self.error = Some(String::from("title is empty"));
            return;
        }
        if let Some(complaint) = live_date_error(&self.date_buf) {
            self.error = Some(complaint);
            return;
        }
        if strict_time(&self.time_buf).is_err() {
            self.error = Some(String::from("bad time: expected HH:MM"));
            return;
        }
        if parse_duration(&self.duration).is_err() {
            self.error = Some(String::from("bad duration: expected 45m, 2h, 1h30m"));
            return;
        }
        crate::debug_log!(
            "tui submit: {} {} {}",
            self.date_buf.trim(),
            self.time_buf.trim(),
            self.title.trim()
        );
        self.done = true;
    }

    fn data(&self) -> Option<TuiData> {
        if !self.done {
            return None;
        }
        Some(TuiData {
            title: self.title.trim().to_owned(),
            date: NaiveDate::parse_from_str(self.date_buf.trim(), "%Y-%m-%d").ok()?,
            time: strict_time(&self.time_buf).ok()?,
            duration_raw: self.duration.trim().to_owned(),
            context: self.context.trim().to_owned(),
        })
    }
}

/// Run a closure inside the alternate screen with raw mode enabled,
/// restoring the terminal on every exit path.
pub(crate) fn with_terminal<T>(
    run: impl FnOnce(&mut Terminal<CrosstermBackend<io::Stdout>>) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    if execute!(stdout, EnterAlternateScreen).is_err() {
        let _ = disable_raw_mode();
        anyhow::bail!("failed to enter the alternate screen");
    }
    let mut terminal = match Terminal::new(CrosstermBackend::new(stdout)) {
        Ok(terminal) => terminal,
        Err(err) => {
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
            return Err(err.into());
        }
    };
    let result = run(&mut terminal);
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();
    result
}

/// Run the form. The terminal is restored on every exit path.
pub fn run() -> anyhow::Result<TuiOutcome> {
    let mut app = App::new();
    with_terminal(|terminal| event_loop(terminal, &mut app))?;
    if app.cancelled {
        return Ok(TuiOutcome::Cancelled);
    }
    match app.data() {
        Some(data) => Ok(TuiOutcome::Submitted(data)),
        None => Ok(TuiOutcome::Cancelled),
    }
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> anyhow::Result<()> {
    loop {
        terminal.draw(|frame| render(frame, app))?;
        if let CrosstermEvent::Key(key) = event::read()? {
            app.handle_key(key);
            if app.done || app.cancelled {
                break;
            }
        }
    }
    Ok(())
}

fn render(frame: &mut ratatui::Frame<'_>, app: &App) {
    let popup = centered(frame.area(), 64, 24);
    // Live state, not only submit errors: the complaint shows before Enter,
    // while any other field is being fixed.
    let live = live_date_error(&app.date_buf);
    let mut constraints = vec![Constraint::Length(3)];
    if live.is_some() {
        constraints.push(Constraint::Length(1));
    }
    constraints.extend([
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(7),
        Constraint::Min(4),
    ]);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(popup);
    let mut idx = 0;
    render_input(
        frame,
        rows[idx],
        "title",
        app.focus == Field::Title,
        &app.title,
        field_cursor(app, Field::Title),
        false,
    );
    idx += 1;
    if live.is_some() {
        frame.render_widget(
            Paragraph::new(face_for(&app.date_buf))
                .style(Style::default().fg(Color::Red))
                .alignment(Alignment::Center),
            rows[idx],
        );
        idx += 1;
    }
    render_input(
        frame,
        rows[idx],
        "date (YYYY-MM-DD)",
        app.focus == Field::Date,
        &app.date_buf,
        field_cursor(app, Field::Date),
        live.is_some(),
    );
    idx += 1;
    render_input(
        frame,
        rows[idx],
        "time (HH:MM)",
        app.focus == Field::Time,
        &app.time_buf,
        field_cursor(app, Field::Time),
        false,
    );
    idx += 1;
    render_input(
        frame,
        rows[idx],
        "duration (45m, 2h, 1h30m)",
        app.focus == Field::Duration,
        &app.duration,
        field_cursor(app, Field::Duration),
        false,
    );
    idx += 1;
    render_input(
        frame,
        rows[idx],
        "context",
        app.focus == Field::Context,
        &app.context,
        field_cursor(app, Field::Context),
        false,
    );
    idx += 1;
    render_footer(frame, rows[idx], app.error.as_deref().or(live.as_deref()));
}

fn field_cursor(app: &App, field: Field) -> Option<usize> {
    (app.focus == field).then_some(app.cursor)
}

/// Center a fixed `width`x`height` block inside `area`, clamped when small.
pub(crate) fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

// Justified: six narrow params read better here than a bag struct for one call site.
#[allow(clippy::too_many_arguments)]
fn render_input(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    title: &str,
    focused: bool,
    text: &str,
    cursor: Option<usize>,
    invalid: bool,
) {
    let border = if invalid {
        Color::Red
    } else if focused {
        Color::Yellow
    } else {
        Color::DarkGray
    };
    let inner_w = (area.width as usize).saturating_sub(2).max(1);
    let inner_h = (area.height as usize).saturating_sub(2).max(1);
    let rows = visual_rows(text, inner_w);
    let (crow, ccol) = cursor
        .map(|idx| (idx / inner_w, idx % inner_w))
        .unwrap_or((0, 0));
    let offset = crow
        .saturating_sub(inner_h.saturating_sub(1))
        .min(rows.len().saturating_sub(inner_h));
    let visible: Vec<Line> = rows
        .iter()
        .skip(offset)
        .take(inner_h)
        .map(|row| Line::from(row.clone()))
        .collect();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(border));
    frame.render_widget(Paragraph::new(visible).block(block), area);
    if focused {
        frame.set_cursor_position((
            area.x + 1 + ccol.min(inner_w.saturating_sub(1)) as u16,
            area.y + 1 + crow.saturating_sub(offset).min(inner_h.saturating_sub(1)) as u16,
        ));
    }
}

fn render_footer(frame: &mut ratatui::Frame<'_>, area: Rect, error: Option<&str>) {
    let mut lines = Vec::new();
    if let Some(error) = error {
        lines.push(Line::from(error.to_owned()));
    }
    lines.push(Line::from(
        "Tab, up/down: fields; Enter: create; Esc: cancel",
    ));
    lines.push(Line::from(
        "date: left/right/h/l: day; time: left/right: quarters; j/k: fields",
    ));
    let style = if error.is_some() {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    frame.render_widget(Paragraph::new(lines).style(style), area);
}

/// Split text into visual rows of at most `width` chars.
/// The cursor math in `render_input` relies on exactly this wrapping.
fn visual_rows(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = Vec::new();
    for line in text.split('\n') {
        let chars: Vec<char> = line.chars().collect();
        if chars.is_empty() {
            rows.push(String::new());
            continue;
        }
        for chunk in chars.chunks(width) {
            rows.push(chunk.iter().collect());
        }
    }
    rows
}

const SETTING_KEYS: [&str; 12] = [
    "chora_dir",
    "editor",
    "open_dirs",
    "auto_commit",
    "manage_old_event",
    "remote_url",
    "day_color",
    "repeat_color",
    "show_repeat",
    "fixed_cells",
    "header",
    "show_today",
];

struct SettingsApp {
    cfg: Config,
    index: usize,
    error: Option<String>,
    done: bool,
    cancelled: bool,
}

fn cycle_color(current: &str, step: isize) -> String {
    let pos = PALETTE
        .iter()
        .position(|(name, _)| name.eq_ignore_ascii_case(current.trim()))
        .unwrap_or(0) as isize;
    let next = (pos + step).rem_euclid(PALETTE.len() as isize) as usize;
    PALETTE[next].0.to_owned()
}

fn cycle_policy(current: OldEventPolicy, step: isize) -> OldEventPolicy {
    const ALL: [OldEventPolicy; 3] = [
        OldEventPolicy::Nothing,
        OldEventPolicy::Backlog,
        OldEventPolicy::Delete,
    ];
    let pos = ALL.iter().position(|&p| p == current).unwrap_or(0) as isize;
    ALL[(pos + step).rem_euclid(ALL.len() as isize) as usize]
}

impl SettingsApp {
    fn new(cfg: Config) -> Self {
        Self {
            cfg,
            index: 0,
            error: None,
            done: false,
            cancelled: false,
        }
    }

    fn text_mut(&mut self) -> Option<&mut String> {
        match self.index {
            0 => Some(&mut self.cfg.main.chora_dir),
            1 => Some(&mut self.cfg.main.editor),
            2 => Some(&mut self.cfg.main.open_dirs),
            5 => Some(&mut self.cfg.main.remote_url),
            10 => Some(&mut self.cfg.calendar.header),
            _ => None,
        }
    }

    fn value_str(&self, idx: usize) -> String {
        match idx {
            0 => self.cfg.main.chora_dir.clone(),
            1 => self.cfg.main.editor.clone(),
            2 => self.cfg.main.open_dirs.clone(),
            3 => format!("{} min", self.cfg.main.auto_commit),
            4 => match self.cfg.main.manage_old_event {
                OldEventPolicy::Nothing => String::from("nothing"),
                OldEventPolicy::Backlog => String::from("backlog"),
                OldEventPolicy::Delete => String::from("delete"),
            },
            5 => self.cfg.main.remote_url.clone(),
            6 => self.cfg.calendar.day_color.clone(),
            7 => self.cfg.calendar.repeat_color.clone(),
            8 => self.cfg.calendar.show_repeat.to_string(),
            9 => self.cfg.calendar.fixed_cells.to_string(),
            10 => self.cfg.calendar.header.clone(),
            11 => self.cfg.calendar.show_today.to_string(),
            _ => String::new(),
        }
    }

    fn move_focus(&mut self, delta: isize) {
        let len = SETTING_KEYS.len() as isize;
        self.index = (self.index as isize + delta).rem_euclid(len) as usize;
    }

    fn step(&mut self, delta: isize) {
        self.error = None;
        match self.index {
            3 => {
                self.cfg.main.auto_commit = if delta >= 0 {
                    self.cfg.main.auto_commit.saturating_add(1)
                } else {
                    self.cfg.main.auto_commit.saturating_sub(1)
                };
            }
            4 => {
                self.cfg.main.manage_old_event = cycle_policy(self.cfg.main.manage_old_event, delta)
            }
            6 => self.cfg.calendar.day_color = cycle_color(&self.cfg.calendar.day_color, delta),
            7 => {
                self.cfg.calendar.repeat_color = cycle_color(&self.cfg.calendar.repeat_color, delta)
            }
            8 => self.cfg.calendar.show_repeat = !self.cfg.calendar.show_repeat,
            9 => self.cfg.calendar.fixed_cells = !self.cfg.calendar.fixed_cells,
            11 => self.cfg.calendar.show_today = !self.cfg.calendar.show_today,
            _ => {}
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
            self.cancelled = true;
            return;
        }
        match key.code {
            KeyCode::Esc => self.cancelled = true,
            KeyCode::Enter => {
                if self.cfg.main.chora_dir.trim().is_empty() {
                    self.error = Some(String::from("chora_dir cannot be empty"));
                } else {
                    self.done = true;
                }
            }
            KeyCode::Tab | KeyCode::Down => self.move_focus(1),
            KeyCode::BackTab | KeyCode::Up => self.move_focus(-1),
            KeyCode::Left => self.step(-1),
            KeyCode::Right => self.step(1),
            KeyCode::Backspace => {
                self.error = None;
                if let Some(buf) = self.text_mut() {
                    buf.pop();
                } else if self.index == 3 {
                    self.cfg.main.auto_commit /= 10;
                }
            }
            KeyCode::Char(ch) => {
                self.error = None;
                if let Some(buf) = self.text_mut() {
                    buf.push(ch);
                } else if self.index == 3 && ch.is_ascii_digit() {
                    let digit = u64::from(ch as u8 - b'0');
                    self.cfg.main.auto_commit = self
                        .cfg
                        .main
                        .auto_commit
                        .saturating_mul(10)
                        .saturating_add(digit);
                } else {
                    match ch {
                        'j' => self.move_focus(1),
                        'k' => self.move_focus(-1),
                        'h' => self.step(-1),
                        'l' | ' ' => self.step(1),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}

/// Interactive TUI editor for all `config.toml` fields.
/// Returns `Some(Config)` on `Enter`, `None` on `Esc`/`Ctrl-C`.
pub fn run_settings(cfg: &Config) -> anyhow::Result<Option<Config>> {
    let mut app = SettingsApp::new(cfg.clone());
    with_terminal(|terminal| {
        loop {
            terminal.draw(|frame| render_settings(frame, &app))?;
            if let CrosstermEvent::Key(key) = event::read()? {
                app.handle_key(key);
                if app.done || app.cancelled {
                    break;
                }
            }
        }
        Ok(())
    })?;
    Ok((!app.cancelled && app.done).then_some(app.cfg))
}

fn render_settings(frame: &mut ratatui::Frame<'_>, app: &SettingsApp) {
    let area = centered(frame.area(), 76, 20);
    let block = Block::default()
        .borders(Borders::ALL)
        .title("settings")
        .border_style(Style::default().fg(Color::Yellow));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = Vec::with_capacity(18);
    for (idx, &key) in SETTING_KEYS.iter().enumerate() {
        if idx == 0 {
            lines.push(Line::styled("[main]", Style::default().fg(Color::DarkGray)));
        } else if idx == 6 {
            lines.push(Line::styled(
                "[calendar]",
                Style::default().fg(Color::DarkGray),
            ));
        }
        let focused = idx == app.index;
        let marker = if focused { "> " } else { "  " };
        let is_text = matches!(idx, 0 | 1 | 2 | 5 | 10);
        let val = app.value_str(idx);
        let shown = if is_text {
            format!("\"{val}\"")
        } else {
            format!("< {val} >")
        };
        let style = if focused {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        };
        lines.push(Line::styled(format!("{marker}{key:<18} = {shown}"), style));
    }
    lines.push(Line::from(""));
    let hint = field_comment(SETTING_KEYS[app.index])
        .unwrap_or_default()
        .trim_start_matches("# ");
    lines.push(Line::styled(hint, Style::default().fg(Color::Cyan)));
    let footer = app.error.as_deref().unwrap_or(
        "Tab/up/down: move · left/right/space: toggle · type: edit text · Enter: save · Esc: cancel",
    );
    let footer_color = if app.error.is_some() {
        Color::Red
    } else {
        Color::DarkGray
    };
    lines.push(Line::styled(footer, Style::default().fg(footer_color)));
    frame.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_text(app: &mut App, text: &str) {
        for ch in text.chars() {
            app.handle_key(key(KeyCode::Char(ch)));
        }
    }

    fn fill_valid(app: &mut App) {
        type_text(app, "Standup");
        app.handle_key(key(KeyCode::Tab)); // Date (leave today as is)
        app.handle_key(key(KeyCode::Tab)); // Time (prefilled now)
        app.handle_key(key(KeyCode::Tab)); // Duration
        type_text(app, "45m");
        app.handle_key(key(KeyCode::Tab)); // Context
        type_text(app, "Agenda.");
    }

    #[test]
    fn new_starts_on_title_with_today() {
        let app = App::new();
        assert_eq!(app.focus, Field::Title);
        assert_eq!(app.date_buf, today().format("%Y-%m-%d").to_string());
    }

    #[test]
    fn typing_and_backspace_in_title() {
        let mut app = App::new();
        type_text(&mut app, "ab");
        app.handle_key(key(KeyCode::Backspace));
        type_text(&mut app, "c");
        assert_eq!(app.title, "ac");
    }

    #[test]
    fn tab_cycles_all_fields() {
        let mut app = App::new();
        for expected in [
            Field::Date,
            Field::Time,
            Field::Duration,
            Field::Context,
            Field::Title,
        ] {
            app.handle_key(key(KeyCode::Tab));
            assert_eq!(app.focus, expected);
        }
    }

    #[test]
    fn up_down_move_focus() {
        let mut app = App::new();
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.focus, Field::Date);
        app.handle_key(key(KeyCode::Up));
        assert_eq!(app.focus, Field::Title);
    }

    #[test]
    fn arrows_shift_date_by_one_day() {
        let mut app = App::new();
        app.handle_key(key(KeyCode::Tab));
        let start = NaiveDate::parse_from_str(&app.date_buf, "%Y-%m-%d").unwrap();
        app.handle_key(key(KeyCode::Right));
        let next = NaiveDate::parse_from_str(&app.date_buf, "%Y-%m-%d").unwrap();
        assert_eq!(
            next,
            start.checked_add_signed(chrono::Duration::days(1)).unwrap()
        );
        app.handle_key(key(KeyCode::Char('h')));
        let back = NaiveDate::parse_from_str(&app.date_buf, "%Y-%m-%d").unwrap();
        assert_eq!(back, start);
    }

    #[test]
    fn jk_move_focus_from_date() {
        let mut app = App::new();
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.focus, Field::Date);
        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(app.focus, Field::Time);
        app.handle_key(key(KeyCode::Up));
        assert_eq!(app.focus, Field::Date);
        app.handle_key(key(KeyCode::Char('k')));
        assert_eq!(app.focus, Field::Title);
    }

    #[test]
    fn submit_rejects_empty_title() {
        let mut app = App::new();
        app.handle_key(key(KeyCode::Enter));
        assert!(!app.done);
        assert_eq!(app.error.as_deref(), Some("title is empty"));
    }

    #[test]
    fn submit_rejects_bad_duration() {
        let mut app = App::new();
        type_text(&mut app, "Standup");
        app.handle_key(key(KeyCode::Tab));
        app.handle_key(key(KeyCode::Tab));
        app.handle_key(key(KeyCode::Tab));
        type_text(&mut app, "ages");
        app.handle_key(key(KeyCode::Enter));
        assert!(!app.done);
        assert!(app.error.unwrap().contains("bad duration"));
    }

    #[test]
    fn submit_accepts_valid_form() {
        let mut app = App::new();
        fill_valid(&mut app);
        app.handle_key(key(KeyCode::Enter));
        assert!(app.done);
        let data = app.data().unwrap();
        assert_eq!(data.title, "Standup");
        assert_eq!(data.duration_raw, "45m");
        assert_eq!(data.context, "Agenda.");
    }

    #[test]
    fn esc_cancels() {
        let mut app = App::new();
        app.handle_key(key(KeyCode::Esc));
        assert!(app.cancelled);
        assert!(app.data().is_none());
    }

    #[test]
    fn render_does_not_panic() {
        use ratatui::backend::TestBackend;
        for (width, height) in [(80, 30), (64, 24), (40, 20)] {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).unwrap();
            let app = App::new();
            terminal.draw(|frame| render(frame, &app)).unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(text.contains("time (HH:MM"), "{width}x{height}");
        }
    }

    #[test]
    fn ctrl_c_cancels() {
        let mut app = App::new();
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.cancelled);
    }

    #[test]
    fn time_prefilled_with_now() {
        let app = App::new();
        assert!(NaiveTime::parse_from_str(&app.time_buf, "%H:%M").is_ok());
    }

    #[test]
    fn time_arrows_snap_to_quarters() {
        use chrono::Timelike;
        let mut app = App::new();
        app.handle_key(key(KeyCode::Tab));
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.focus, Field::Time);
        // Ahead lands strictly on the next mark (whatever the prefill was).
        app.handle_key(key(KeyCode::Right));
        let first = NaiveTime::parse_from_str(&app.time_buf, "%H:%M").unwrap();
        assert_eq!(first.minute() % 15, 0);
        // On a mark it keeps stepping.
        app.handle_key(key(KeyCode::Right));
        let second = NaiveTime::parse_from_str(&app.time_buf, "%H:%M").unwrap();
        assert_eq!(second, snap_quarter(first, true));
        // Behind mirrors it.
        app.handle_key(key(KeyCode::Left));
        assert_eq!(
            NaiveTime::parse_from_str(&app.time_buf, "%H:%M").unwrap(),
            first
        );
    }

    #[test]
    fn jk_move_focus_from_time() {
        let mut app = App::new();
        app.handle_key(key(KeyCode::Tab));
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.focus, Field::Time);
        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(app.focus, Field::Duration);
        app.handle_key(key(KeyCode::Up));
        assert_eq!(app.focus, Field::Time);
        app.handle_key(key(KeyCode::Char('k')));
        assert_eq!(app.focus, Field::Date);
    }

    #[test]
    fn submit_rejects_bad_time() {
        let mut app = App::new();
        type_text(&mut app, "Standup");
        app.handle_key(key(KeyCode::Tab));
        app.handle_key(key(KeyCode::Tab));
        app.time_buf = String::from("ages");
        app.handle_key(key(KeyCode::Enter));
        assert!(!app.done);
        assert!(app.error.unwrap().contains("bad time"));
    }

    #[test]
    fn submit_rejects_smart_time() {
        for raw in ["930", "9", "now", "+30", "ages", ""] {
            let mut app = App::new();
            type_text(&mut app, "Standup");
            app.handle_key(key(KeyCode::Tab));
            app.handle_key(key(KeyCode::Tab));
            app.time_buf = String::from(raw);
            app.handle_key(key(KeyCode::Tab));
            type_text(&mut app, "45m");
            app.handle_key(key(KeyCode::Tab));
            type_text(&mut app, "Agenda.");
            app.handle_key(key(KeyCode::Enter));
            assert!(!app.done, "{raw}");
            assert_eq!(
                app.error.as_deref(),
                Some("bad time: expected HH:MM"),
                "{raw}"
            );
        }
    }

    #[test]
    fn submit_rejects_past_date() {
        let mut app = App::new();
        type_text(&mut app, "Standup");
        app.handle_key(key(KeyCode::Tab));
        let yesterday = today()
            .checked_add_signed(chrono::Duration::days(-1))
            .unwrap();
        app.date_buf = yesterday.format("%Y-%m-%d").to_string();
        app.handle_key(key(KeyCode::Tab));
        app.handle_key(key(KeyCode::Tab));
        type_text(&mut app, "45m");
        app.handle_key(key(KeyCode::Tab));
        type_text(&mut app, "Agenda.");
        app.handle_key(key(KeyCode::Enter));
        assert!(!app.done);
        let error = app.error.unwrap();
        assert!(error.contains("past"), "{error}");
    }

    #[test]
    fn submit_accepts_future_date() {
        let mut app = App::new();
        type_text(&mut app, "Standup");
        app.handle_key(key(KeyCode::Tab));
        let tomorrow = today()
            .checked_add_signed(chrono::Duration::days(1))
            .unwrap();
        app.date_buf = tomorrow.format("%Y-%m-%d").to_string();
        app.handle_key(key(KeyCode::Tab));
        app.handle_key(key(KeyCode::Tab));
        type_text(&mut app, "45m");
        app.handle_key(key(KeyCode::Tab));
        type_text(&mut app, "Agenda.");
        app.handle_key(key(KeyCode::Enter));
        assert!(app.done);
        assert_eq!(app.data().unwrap().date, tomorrow);
    }

    #[test]
    fn date_complains_live_while_typing() {
        let mut app = App::new();
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.focus, Field::Date);
        app.date_buf.clear();
        app.cursor = 0;
        type_text(&mut app, "2020-01-01");
        assert!(app.error.as_deref().unwrap().contains("past"));
        app.date_buf.clear();
        app.cursor = 0;
        type_text(&mut app, "20");
        assert!(app.error.as_deref().unwrap().contains("bad date"));
        app.date_buf.clear();
        app.cursor = 0;
        type_text(&mut app, &today().format("%Y-%m-%d").to_string());
        assert!(app.error.is_none());
    }

    #[test]
    fn shift_date_into_past_complains() {
        let mut app = App::new();
        app.handle_key(key(KeyCode::Tab));
        app.handle_key(key(KeyCode::Char('h')));
        assert!(app.error.as_deref().unwrap().contains("past"));
        app.handle_key(key(KeyCode::Char('l')));
        assert!(app.error.is_none());
    }

    #[test]
    fn face_shows_above_invalid_date() {
        use ratatui::backend::TestBackend;
        let mut bad = App::new();
        bad.date_buf = String::from("2020-01-01");
        for app in [App::new(), bad] {
            let backend = TestBackend::new(80, 30);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal.draw(|frame| render(frame, &app)).unwrap();
            let text: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            let has_face = DATE_FACES.iter().any(|face| text.contains(face));
            assert_eq!(has_face, app.date_buf == "2020-01-01");
        }
    }

    #[test]
    fn single_line_input_keeps_text_visible_at_exact_width() {
        use ratatui::backend::TestBackend;
        let mut app = App::new();
        type_text(&mut app, &"x".repeat(62));
        let backend = TestBackend::new(80, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains(&"x".repeat(62)));
    }

    #[test]
    fn settings_edits_text_toggles_and_submits() {
        let mut app = SettingsApp::new(Config::default());
        // Index 0: chora_dir (text). Reject empty on Enter.
        app.cfg.main.chora_dir.clear();
        app.handle_key(key(KeyCode::Enter));
        assert!(!app.done);
        assert!(app.error.is_some());
        for ch in "~/notes".chars() {
            app.handle_key(key(KeyCode::Char(ch)));
        }
        assert_eq!(app.cfg.main.chora_dir, "~/notes");

        // Index 1: editor (text)
        app.handle_key(key(KeyCode::Tab));
        app.handle_key(key(KeyCode::Char('h')));
        app.handle_key(key(KeyCode::Char('x')));
        assert_eq!(app.cfg.main.editor, "hx");

        // Index 3: auto_commit (number)
        app.handle_key(key(KeyCode::Tab));
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.index, 3);
        app.handle_key(key(KeyCode::Right));
        assert_eq!(app.cfg.main.auto_commit, 1);
        app.handle_key(key(KeyCode::Char('5')));
        assert_eq!(app.cfg.main.auto_commit, 15);
        app.handle_key(key(KeyCode::Backspace));
        assert_eq!(app.cfg.main.auto_commit, 1);

        // Index 4: manage_old_event (policy cycle)
        app.handle_key(key(KeyCode::Tab));
        app.handle_key(key(KeyCode::Right));
        assert_eq!(app.cfg.main.manage_old_event, OldEventPolicy::Backlog);

        // Index 6: day_color (palette cycle)
        app.handle_key(key(KeyCode::Tab));
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.index, 6);
        app.handle_key(key(KeyCode::Right));
        assert_eq!(app.cfg.calendar.day_color, "yellow");

        // Index 8: show_repeat (bool toggle)
        app.handle_key(key(KeyCode::Tab));
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.index, 8);
        app.handle_key(key(KeyCode::Char(' ')));
        assert!(!app.cfg.calendar.show_repeat);

        app.handle_key(key(KeyCode::Enter));
        assert!(app.done);
    }

    #[test]
    fn render_settings_shows_all_keys() {
        use ratatui::backend::TestBackend;
        let app = SettingsApp::new(Config::default());
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render_settings(frame, &app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        for key in SETTING_KEYS {
            assert!(text.contains(key), "settings screen misses `{key}`");
        }
    }
}
