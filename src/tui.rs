//! `event --tui`: a minimal event-creation form (ratatui + crossterm).
//!
//! Five fields: `title`, `date`, `time`, `duration`,
//! `context`. `time` is prefilled with now and accepts `HH:MM`, `HHMM`,
//! `H`, `now`, `+30` (minutes from now).
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
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders, Paragraph},
};

use crate::cli::today;
use crate::event::{parse_duration, parse_time_entry, snap_quarter};

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
                self.error = None;
            }
            None => self.error = Some(String::from("bad date: expected YYYY-MM-DD")),
        }
        self.cursor = self.buf().chars().count();
    }

    fn shift_time(&mut self, forward: bool) {
        let now = now_time();
        let base = parse_time_entry(&self.time_buf, now).unwrap_or(now);
        self.time_buf = snap_quarter(base, forward).format("%H:%M").to_string();
        self.error = None;
        self.cursor = self.buf().chars().count();
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
        if NaiveDate::parse_from_str(self.date_buf.trim(), "%Y-%m-%d").is_err() {
            self.error = Some(String::from("bad date: expected YYYY-MM-DD"));
            return;
        }
        if parse_time_entry(&self.time_buf, now_time()).is_err() {
            self.error = Some(String::from("bad time: expected HH:MM, 1430, 9, now, +30"));
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
            time: parse_time_entry(self.time_buf.trim(), now_time()).ok()?,
            duration_raw: self.duration.trim().to_owned(),
            context: self.context.trim().to_owned(),
        })
    }
}

/// Run the form. The terminal is restored on every exit path.
pub fn run() -> anyhow::Result<TuiOutcome> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    if execute!(stdout, EnterAlternateScreen).is_err() {
        let _ = disable_raw_mode();
        anyhow::bail!("failed to enter the alternate screen");
    }
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = match Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(err) => {
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
            return Err(err.into());
        }
    };

    let mut app = App::new();
    let flow = event_loop(&mut terminal, &mut app);

    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();
    flow?;

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
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(7),
            Constraint::Min(4),
        ])
        .split(popup);
    render_input(
        frame,
        rows[0],
        "title",
        app.focus == Field::Title,
        &app.title,
        field_cursor(app, Field::Title),
    );
    render_input(
        frame,
        rows[1],
        "date (YYYY-MM-DD)",
        app.focus == Field::Date,
        &app.date_buf,
        field_cursor(app, Field::Date),
    );
    render_input(
        frame,
        rows[2],
        "time (HH:MM)",
        app.focus == Field::Time,
        &app.time_buf,
        field_cursor(app, Field::Time),
    );
    render_input(
        frame,
        rows[3],
        "duration (45m, 2h, 1h30m)",
        app.focus == Field::Duration,
        &app.duration,
        field_cursor(app, Field::Duration),
    );
    render_input(
        frame,
        rows[4],
        "context",
        app.focus == Field::Context,
        &app.context,
        field_cursor(app, Field::Context),
    );
    render_footer(frame, rows[5], app.error.as_deref());
}

fn field_cursor(app: &App, field: Field) -> Option<usize> {
    (app.focus == field).then_some(app.cursor)
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
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
) {
    let border = if focused {
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
    let offset = crow.saturating_sub(inner_h.saturating_sub(1));
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
            area.x + 1 + ccol as u16,
            area.y + 1 + (crow - offset) as u16,
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
        let parsed = parse_time_entry(&app.time_buf, now_time());
        assert!(parsed.is_ok());
    }

    #[test]
    fn time_arrows_snap_to_quarters() {
        use crate::event::shift_time_of_day;
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
        assert_eq!(second, shift_time_of_day(first, 15));
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
    fn submit_accepts_smart_time() {
        let mut app = App::new();
        type_text(&mut app, "Standup");
        app.handle_key(key(KeyCode::Tab));
        app.handle_key(key(KeyCode::Tab));
        app.time_buf = String::from("930");
        app.handle_key(key(KeyCode::Tab));
        type_text(&mut app, "45m");
        app.handle_key(key(KeyCode::Tab));
        type_text(&mut app, "Agenda.");
        app.handle_key(key(KeyCode::Enter));
        assert!(app.done);
        let data = app.data().unwrap();
        assert_eq!(data.time, NaiveTime::from_hms_opt(9, 30, 0).unwrap());
    }
}
