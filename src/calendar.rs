//! `calendar`: a read-only month-grid TUI (ratatui + crossterm).
//!
//! One cell per day: the first event title above the day number.
//! `fixed_cells` pins 6x3 cells (square on typical fonts) centered in
//! the terminal; by default cells stretch over the full width.
//! Events from the day file use `day_color`, weekday repeatables use
//! `repeat_color` (a day with both shows the day event). Arrows/hjkl move
//! the selection across month borders, `t` jumps to today, `q`/`Esc` quit.

use std::path::{Path, PathBuf};

use chrono::{Datelike, NaiveDate};
use crossterm::event::{self, Event as CrosstermEvent, KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::cli::today;
use crate::event::parse_day;
use crate::store::Store;
use crate::tui::{centered, with_terminal};

/// One of the 42 visible cells. `current` marks the shown month;
/// border days from adjacent months render dimmed.
#[derive(Debug, Clone, Copy)]
pub struct Cell {
    /// Cell date (may belong to an adjacent month).
    pub date: NaiveDate,
    /// True when the date is inside the shown month.
    pub current: bool,
}

/// Event markers of a cell: first titles of each kind.
#[derive(Debug, Clone)]
pub struct DayMarks {
    /// First event title recorded on that date, if any.
    pub day_title: Option<String>,
    /// First weekday-repeatable title, if any.
    pub repeat_title: Option<String>,
}

impl DayMarks {
    /// The shown title: the day event wins over the repeatable.
    #[must_use]
    pub fn title(&self) -> Option<&str> {
        self.day_title.as_deref().or(self.repeat_title.as_deref())
    }

    /// True for color1 (recorded on that date), false for color2.
    #[must_use]
    pub fn is_day_event(&self) -> bool {
        self.day_title.is_some()
    }
}

/// 6x7 grid starting on Sunday and covering `year`/`month`.
/// `None` only for out-of-range years.
#[must_use]
pub fn month_grid(year: i32, month: u32) -> Option<Vec<Cell>> {
    let first = NaiveDate::from_ymd_opt(year, month, 1)?;
    let offset = first.weekday().num_days_from_sunday() as i64;
    let mut start = first.checked_sub_signed(chrono::Duration::days(offset))?;
    let mut cells = Vec::with_capacity(42);
    for _ in 0..42 {
        cells.push(Cell {
            date: start,
            current: start.month() == month,
        });
        start = start.succ_opt()?;
    }
    Some(cells)
}

/// Markers for the 42 grid cells, in grid order. Each weekday file is read
/// once, not once per cell. `show_repeat == false` skips repeatables
/// entirely (fewer reads, day events only).
pub fn load_marks(
    root: &Path,
    year: i32,
    month: u32,
    show_repeat: bool,
) -> anyhow::Result<Vec<DayMarks>> {
    let cells = month_grid(year, month).ok_or_else(|| anyhow::anyhow!("date out of range"))?;
    let store = Store::new(root.to_path_buf());
    let mut weekday_titles: Vec<Option<String>> = Vec::with_capacity(7);
    for cell in &cells[..7] {
        let title = if show_repeat {
            store
                .read_repeatables(cell.date)?
                .into_iter()
                .next()
                .map(|event| event.title)
        } else {
            None
        };
        weekday_titles.push(title);
    }
    let mut marks = Vec::with_capacity(42);
    for cell in &cells {
        let day_title = parse_day(cell.date, &store.read_day(cell.date)?)
            .into_iter()
            .next()
            .map(|event| event.title);
        let repeat_title =
            weekday_titles[cell.date.weekday().num_days_from_sunday() as usize].clone();
        marks.push(DayMarks {
            day_title,
            repeat_title,
        });
    }
    Ok(marks)
}

/// Terminal-palette color names in cycle order.
pub const PALETTE: [(&str, Color); 16] = [
    ("black", Color::Black),
    ("red", Color::Red),
    ("green", Color::Green),
    ("yellow", Color::Yellow),
    ("blue", Color::Blue),
    ("magenta", Color::Magenta),
    ("cyan", Color::Cyan),
    ("white", Color::White),
    ("gray", Color::Gray),
    ("darkgray", Color::DarkGray),
    ("lightred", Color::LightRed),
    ("lightgreen", Color::LightGreen),
    ("lightyellow", Color::LightYellow),
    ("lightblue", Color::LightBlue),
    ("lightmagenta", Color::LightMagenta),
    ("lightcyan", Color::LightCyan),
];

/// Terminal-palette color names. No RGB: strictly terminal colors.
pub fn parse_color(raw: &str) -> Result<Color, String> {
    let lower = raw.trim().to_lowercase();
    PALETTE
        .iter()
        .find(|(name, _)| *name == lower)
        .map(|(_, color)| *color)
        .ok_or_else(|| {
            let names: Vec<&str> = PALETTE.iter().map(|(n, _)| *n).collect();
            format!("unknown color `{raw}`: expected {}", names.join(", "))
        })
}

/// Truncate to `width` chars (multibyte-safe).
fn fit(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let mut end = 0;
    for (count, (index, ch)) in text.char_indices().enumerate() {
        if count == width {
            break;
        }
        end = index + ch.len_utf8();
    }
    text[..end].to_owned()
}

/// Display options, built once from `[calendar]` in `main`.
#[derive(Debug, Clone)]
pub struct Options {
    /// Color for events recorded on that date.
    pub day_color: Color,
    /// Color for weekday repeatables.
    pub repeat_color: Color,
    /// Whether repeatables are marked on the calendar at all.
    pub show_repeat: bool,
    /// Fixed 6x3 cells (square on typical fonts) instead of full-width stretch.
    pub fixed_cells: bool,
    /// `strftime` format of the selected date, top-left.
    pub header: String,
    /// Today hint, top-right, only when looking at another date.
    pub show_today: bool,
}

struct App {
    root: PathBuf,
    year: i32,
    month: u32,
    cells: Vec<Cell>,
    marks: Vec<DayMarks>,
    selected: NaiveDate,
    options: Options,
    /// Set by `Enter`: open this date after the TUI exits.
    open_requested: Option<NaiveDate>,
    done: bool,
}

impl App {
    fn new(root: &Path, options: Options) -> anyhow::Result<Self> {
        let selected = today();
        let mut app = Self {
            root: root.to_path_buf(),
            year: selected.year(),
            month: selected.month(),
            cells: Vec::new(),
            marks: Vec::new(),
            selected,
            options,
            open_requested: None,
            done: false,
        };
        app.reload()?;
        Ok(app)
    }

    fn reload(&mut self) -> anyhow::Result<()> {
        self.cells = month_grid(self.year, self.month)
            .ok_or_else(|| anyhow::anyhow!("date out of range"))?;
        self.marks = load_marks(&self.root, self.year, self.month, self.options.show_repeat)?;
        Ok(())
    }

    fn shift(&mut self, days: i64) -> anyhow::Result<()> {
        if let Some(next) = self
            .selected
            .checked_add_signed(chrono::Duration::days(days))
        {
            self.selected = next;
            if next.year() != self.year || next.month() != self.month {
                self.year = next.year();
                self.month = next.month();
                self.reload()?;
            }
        }
        Ok(())
    }

    fn jump_to_today(&mut self) -> anyhow::Result<()> {
        let now = today();
        if now != self.selected {
            self.selected = now;
            self.year = now.year();
            self.month = now.month();
            self.reload()?;
        }
        Ok(())
    }

    fn handle_key_event(&mut self, key: KeyEvent) -> anyhow::Result<()> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
            self.done = true;
            return Ok(());
        }
        self.handle_key(key.code)
    }

    fn handle_key(&mut self, code: KeyCode) -> anyhow::Result<()> {
        match code {
            KeyCode::Esc | KeyCode::Char('q') => self.done = true,
            KeyCode::Enter => {
                self.open_requested = Some(self.selected);
                self.done = true;
            }
            KeyCode::Left | KeyCode::Char('h') => self.shift(-1)?,
            KeyCode::Right | KeyCode::Char('l') => self.shift(1)?,
            KeyCode::Up | KeyCode::Char('k') => self.shift(-7)?,
            KeyCode::Down | KeyCode::Char('j') => self.shift(7)?,
            KeyCode::Char('t') => self.jump_to_today()?,
            _ => {}
        }
        Ok(())
    }
}

/// Run the calendar. Read-only: no commits, no network. The terminal is
/// restored on every exit path. Returns the date to open when `Enter`
/// was pressed (`None` on quit).
pub fn run(root: &Path, options: Options) -> anyhow::Result<Option<NaiveDate>> {
    let mut app = App::new(root, options)?;
    with_terminal(|terminal| {
        loop {
            terminal.draw(|frame| render(frame, &app))?;
            if let CrosstermEvent::Key(key) = event::read()? {
                app.handle_key_event(key)?;
                if app.done {
                    break;
                }
            }
        }
        Ok(app.open_requested)
    })
}

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// Fixed cells are 6 wide and 3 tall: terminal glyphs are about twice
/// as tall as wide, so 6x3 reads as a square.
const FIXED_CELL_WIDTH: u16 = 6;
const FIXED_CELL_HEIGHT: u16 = 3;
/// Seven cells plus six 1-wide separators; six rows plus five 1-tall ones.
const FIXED_GRID_WIDTH: u16 = 7 * FIXED_CELL_WIDTH + 6;
const FIXED_GRID_HEIGHT: u16 = 6 * FIXED_CELL_HEIGHT + 5;

/// Today hint for the header row: `None` on the `NONE` header, on today
/// itself, or when `show_today` is off.
fn today_hint(app: &App) -> Option<String> {
    if app.options.header.trim().eq_ignore_ascii_case("none") {
        return None;
    }
    (app.options.show_today && app.selected != today())
        .then(|| format!("today {}", today().format(&app.options.header)))
}

/// Top row: selected date left, today hint right (only on other dates).
fn render_header(frame: &mut ratatui::Frame<'_>, app: &App, area: ratatui::layout::Rect) {
    let header_left = Paragraph::new(Line::from(Span::raw(
        app.selected.format(&app.options.header).to_string(),
    )));
    match today_hint(app) {
        Some(hint) => {
            let halves = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(area);
            frame.render_widget(header_left, halves[0]);
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    hint,
                    Style::default().add_modifier(ratatui::style::Modifier::DIM),
                )))
                .alignment(Alignment::Right),
                halves[1],
            );
        }
        None => frame.render_widget(header_left, area),
    }
}

fn render(frame: &mut ratatui::Frame<'_>, app: &App) {
    let area = frame.area();
    let fixed = app.options.fixed_cells;
    let (min_width, weeks_height) = if fixed {
        (FIXED_GRID_WIDTH, FIXED_GRID_HEIGHT)
    } else {
        (56, 17)
    };
    if area.width < min_width || area.height < weeks_height + 5 {
        frame.render_widget(
            Paragraph::new("terminal too small for the calendar").alignment(Alignment::Center),
            area,
        );
        return;
    }
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(weeks_height),
            Constraint::Length(1),
        ])
        .split(area);

    // `NONE` hides the whole header row, hint included.
    if app.options.header.trim().eq_ignore_ascii_case("none") {
        frame.render_widget(Paragraph::new(Line::from("")), rows[0]);
    } else {
        render_header(frame, app, rows[0]);
    }

    let title = Line::from(vec![
        Span::styled(
            "< ",
            Style::default().add_modifier(ratatui::style::Modifier::DIM),
        ),
        Span::styled(
            app.selected.format("%B %Y").to_string(),
            Style::default().add_modifier(ratatui::style::Modifier::BOLD),
        ),
        Span::styled(
            " >",
            Style::default().add_modifier(ratatui::style::Modifier::DIM),
        ),
    ]);
    frame.render_widget(Paragraph::new(title).alignment(Alignment::Center), rows[1]);

    let names: Vec<Paragraph<'_>> = WEEKDAYS
        .iter()
        .map(|day| {
            Paragraph::new(Line::from(Span::styled(
                *day,
                Style::default().add_modifier(ratatui::style::Modifier::DIM),
            )))
            .alignment(Alignment::Center)
        })
        .collect();
    if fixed {
        render_row(frame, centered(rows[2], FIXED_GRID_WIDTH, 1), &names, true);
    } else {
        render_row(frame, rows[2], &names, false);
    }

    let weeks = if fixed {
        centered(rows[3], FIXED_GRID_WIDTH, FIXED_GRID_HEIGHT)
    } else {
        rows[3]
    };
    let cell_height = if fixed { FIXED_CELL_HEIGHT } else { 2 };
    let mut strips = Vec::with_capacity(11);
    for strip in 0..11 {
        strips.push(if strip % 2 == 0 {
            Constraint::Length(cell_height)
        } else {
            Constraint::Length(1)
        });
    }
    let week_strips = Layout::default()
        .direction(Direction::Vertical)
        .constraints(strips)
        .split(weeks);
    // Separator columns from the first week row; identical for all rows.
    let mut sep_xs: Vec<u16> = Vec::new();
    let mut hseps: Vec<ratatui::layout::Rect> = Vec::new();
    for (strip, strip_area) in week_strips.iter().enumerate() {
        if strip % 2 == 1 {
            hseps.push(*strip_area);
            continue;
        }
        let cols = week_columns(*strip_area, fixed);
        let week = strip / 2;
        for day in 0..7 {
            let index = week * 7 + day;
            frame.render_widget(cell_widget(app, index, cols[day * 2].width), cols[day * 2]);
            if sep_xs.len() < 6 {
                sep_xs.push(cols[day * 2 + 1].x);
            }
        }
    }
    draw_grid(frame, weeks, &sep_xs, &hseps);

    let footer = Paragraph::new(Line::from(Span::styled(
        "arrows/hjkl move · enter open · t today · q quit",
        Style::default().add_modifier(ratatui::style::Modifier::DIM),
    )))
    .alignment(Alignment::Center);
    frame.render_widget(footer, rows[4]);
}

fn render_row(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    cells: &[Paragraph<'_>],
    fixed: bool,
) {
    let cols = if fixed {
        week_columns(area, true)
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Ratio(1, 7); 7])
            .split(area)
    };
    // Fixed columns interleave 1-wide separators: cells sit on even slots.
    let step = if fixed { 2 } else { 1 };
    for (cell, col) in cells.iter().zip(cols.iter().step_by(step)) {
        frame.render_widget(cell, *col);
    }
}

/// Seven cells with 1-wide separators, no trailing separator.
fn week_columns(area: ratatui::layout::Rect, fixed: bool) -> std::rc::Rc<[ratatui::layout::Rect]> {
    let mut constraints = Vec::with_capacity(13);
    for _ in 0..7 {
        constraints.push(if fixed {
            Constraint::Length(FIXED_CELL_WIDTH)
        } else {
            Constraint::Ratio(1, 7)
        });
        constraints.push(Constraint::Length(1));
    }
    constraints.pop();
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(area)
}

/// Explicit grid lines, drawn after the cells: full-height `│` separators
/// first, then full-width `─` separators with `┼` crossings on top.
/// No outer frame — only the lines between cells.
fn draw_grid(
    frame: &mut ratatui::Frame<'_>,
    weeks: ratatui::layout::Rect,
    sep_xs: &[u16],
    hseps: &[ratatui::layout::Rect],
) {
    let style = Style::default().add_modifier(ratatui::style::Modifier::DIM);
    let buf = frame.buffer_mut();
    for &x in sep_xs {
        for y in weeks.y..weeks.y + weeks.height {
            buf.set_string(x, y, "│", style);
        }
    }
    for hsep in hseps {
        let mut line = vec!['─'; hsep.width as usize];
        for &x in sep_xs {
            if x >= hsep.x
                && let Some(slot) = line.get_mut((x - hsep.x) as usize)
            {
                *slot = '┼';
            }
        }
        let line: String = line.into_iter().collect();
        buf.set_string(hsep.x, hsep.y, &line, style);
    }
}

/// One grid cell: title above the number, no borders of its own.
fn cell_widget<'a>(app: &App, index: usize, width: u16) -> Paragraph<'a> {
    let cell = &app.cells[index];
    let marks = &app.marks[index];
    let width = width as usize;
    let title = marks.title().unwrap_or_default();
    let title_style = if marks.is_day_event() {
        Style::default().fg(app.options.day_color)
    } else if marks.repeat_title.is_some() {
        Style::default().fg(app.options.repeat_color)
    } else {
        Style::default()
    };
    let mut number_style = Style::default();
    if !cell.current {
        number_style = number_style.add_modifier(ratatui::style::Modifier::DIM);
    }
    let today = today();
    if cell.date == app.selected {
        number_style = number_style.add_modifier(ratatui::style::Modifier::REVERSED);
    } else if cell.date == today {
        number_style = number_style.add_modifier(ratatui::style::Modifier::BOLD);
    }
    let title_style = if cell.current {
        title_style
    } else {
        title_style.add_modifier(ratatui::style::Modifier::DIM)
    };
    Paragraph::new(vec![
        Line::from(Span::styled(fit(title, width), title_style)).alignment(Alignment::Center),
        Line::from(Span::styled(cell.date.day().to_string(), number_style))
            .alignment(Alignment::Center),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use std::path::PathBuf;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    #[test]
    fn grid_starts_on_sunday_and_covers_42_days() {
        // October 2026: the 1st is a Thursday.
        let grid = month_grid(2026, 10).unwrap();
        assert_eq!(grid.len(), 42);
        assert_eq!(grid[0].date, date(2026, 9, 27));
        assert!(!grid[0].current);
        assert_eq!(grid[4].date, date(2026, 10, 1));
        assert!(grid[4].current);
        assert_eq!(grid[41].date, date(2026, 11, 7));
        assert!(!grid[41].current);
        // Consecutive days, Sundays at week starts.
        for pair in grid.windows(2) {
            assert_eq!(pair[1].date, pair[0].date.succ_opt().unwrap());
        }
        for week in grid.chunks(7) {
            assert_eq!(
                week[0].date.weekday().num_days_from_sunday(),
                0,
                "week must start on Sunday"
            );
        }
    }

    #[test]
    fn grid_handles_february_leap_year() {
        let grid = month_grid(2024, 2).unwrap();
        let current: Vec<_> = grid.iter().filter(|cell| cell.current).collect();
        assert_eq!(current.len(), 29);
        assert_eq!(current[28].date, date(2024, 2, 29));
    }

    #[test]
    fn grid_rejects_out_of_range_month() {
        assert!(month_grid(2026, 13).is_none());
        assert!(month_grid(2026, 0).is_none());
    }

    #[test]
    fn marks_prefer_day_events_over_repeatables() {
        let tmp = tempfile::tempdir().unwrap();
        let root: PathBuf = tmp.path().to_path_buf();
        let store = Store::new(root.clone());
        store.ensure_layout().unwrap();
        // 2026-11-23 is a Monday with a day event and a repeatable.
        std::fs::write(
            store.events_dir().join("2026-11-23.md"),
            "# 2026-11-23\n\n## Standup\nTime: 10:00\nDuration: 1h\n",
        )
        .unwrap();
        let repeatable = store.events_dir().join("repeatable");
        std::fs::create_dir_all(&repeatable).unwrap();
        std::fs::write(
            repeatable.join("monday.md"),
            "# Monday\n\n## Teeth\nTime: 09:00\nDuration: 10m\n",
        )
        .unwrap();

        let marks = load_marks(&root, 2026, 11, true).unwrap();
        assert_eq!(marks.len(), 42);
        let grid = month_grid(2026, 11).unwrap();
        let at = |day: u32| {
            marks
                .iter()
                .zip(grid.iter())
                .find(|(_, cell)| cell.date == date(2026, 11, day))
                .map(|(mark, _)| mark)
                .unwrap()
        };
        let monday = at(23);
        assert_eq!(monday.day_title.as_deref(), Some("Standup"));
        assert_eq!(monday.repeat_title.as_deref(), Some("Teeth"));
        // Day event wins the title and the color.
        assert_eq!(monday.title(), Some("Standup"));
        assert!(monday.is_day_event());

        // A Monday without a day file still shows the repeatable.
        let other = at(30);
        assert_eq!(other.day_title, None);
        assert_eq!(other.repeat_title.as_deref(), Some("Teeth"));
        assert_eq!(other.title(), Some("Teeth"));
        assert!(!other.is_day_event());
    }

    #[test]
    fn hidden_repeatables_leave_day_events_only() {
        let (_tmp, root) = seeded_root();
        let marks = load_marks(root.as_path(), 2026, 10, false).unwrap();
        assert_eq!(marks.len(), 42);
        assert!(marks.iter().all(|mark| mark.repeat_title.is_none()));
        // The day event is untouched.
        let grid = month_grid(2026, 10).unwrap();
        let day = marks
            .iter()
            .zip(grid.iter())
            .find(|(_, cell)| cell.date == date(2026, 10, 7))
            .map(|(mark, _)| mark)
            .unwrap();
        assert_eq!(day.title(), Some("Demo Day"));
    }

    #[test]
    fn parse_color_accepts_terminal_palette() {
        for (raw, expected) in [
            ("green", Color::Green),
            (" Blue ", Color::Blue),
            ("MAGENTA", Color::Magenta),
            ("darkgray", Color::DarkGray),
            ("lightcyan", Color::LightCyan),
            ("white", Color::White),
        ] {
            assert_eq!(parse_color(raw).unwrap(), expected, "{raw}");
        }
    }

    #[test]
    fn parse_color_rejects_rgb_and_garbage() {
        for raw in ["", "#ff0000", "chartreuse", "rgb(1,2,3)"] {
            assert!(parse_color(raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn fit_truncates_multibyte_safely() {
        assert_eq!(fit("Standup", 4), "Stan");
        assert_eq!(fit("Teeth", 10), "Teeth");
        assert_eq!(fit("Почистить", 5), "Почис");
        assert_eq!(fit("anything", 0), "");
    }

    fn test_options() -> Options {
        Options {
            day_color: Color::Green,
            repeat_color: Color::Blue,
            show_repeat: true,
            fixed_cells: false,
            header: String::from("%A, %d.%m.%Y"),
            show_today: true,
        }
    }

    fn seeded_root() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let store = Store::new(root.clone());
        store.ensure_layout().unwrap();
        std::fs::write(
            store.events_dir().join("2026-10-07.md"),
            "# 2026-10-07\n\n## Demo Day\nTime: 15:00\nDuration: 1h\n",
        )
        .unwrap();
        let repeatable = store.events_dir().join("repeatable");
        std::fs::create_dir_all(&repeatable).unwrap();
        std::fs::write(
            repeatable.join("wednesday.md"),
            "# Wednesday\n\n## Gym\nTime: 19:00\nDuration: 1h\n",
        )
        .unwrap();
        (tmp, root)
    }

    #[test]
    fn selection_moves_across_month_border() {
        let (_tmp, root) = seeded_root();
        let mut app = App::new(&root, test_options()).unwrap();
        app.selected = date(2026, 10, 31);
        app.year = 2026;
        app.month = 10;
        app.reload().unwrap();
        app.handle_key(KeyCode::Right).unwrap();
        assert_eq!(app.selected, date(2026, 11, 1));
        assert_eq!((app.year, app.month), (2026, 11));
        app.handle_key(KeyCode::Char('q')).unwrap();
        assert!(app.done);
    }

    #[test]
    fn enter_requests_the_selected_day() {
        let (_tmp, root) = seeded_root();
        let mut app = App::new(&root, test_options()).unwrap();
        app.selected = date(2026, 10, 7);
        app.handle_key(KeyCode::Enter).unwrap();
        assert!(app.done);
        assert_eq!(app.open_requested, Some(date(2026, 10, 7)));
    }

    #[test]
    fn quit_requests_nothing() {
        let (_tmp, root) = seeded_root();
        let mut app = App::new(&root, test_options()).unwrap();
        app.handle_key(KeyCode::Char('q')).unwrap();
        assert!(app.done);
        assert_eq!(app.open_requested, None);

        let mut app = App::new(&root, test_options()).unwrap();
        app.handle_key_event(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL))
            .unwrap();
        assert!(app.done);
        assert_eq!(app.open_requested, None);
    }

    fn screen_text(terminal: &ratatui::Terminal<ratatui::backend::TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    #[test]
    fn render_shows_titles_numbers_and_month() {
        use ratatui::backend::TestBackend;

        let (_tmp, root) = seeded_root();
        let mut app = App::new(&root, test_options()).unwrap();
        app.selected = date(2026, 10, 7);
        app.year = 2026;
        app.month = 10;
        app.reload().unwrap();

        let backend = TestBackend::new(80, 26);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let text = screen_text(&terminal);
        for token in [
            "October 2026",
            "Wednesday",
            "07.10.2026",
            "Demo Day",
            "Gym",
            "Sun",
            "Sat",
            "enter open",
            // Continuous inner grid, no outer frame.
            "─",
            "│",
            "┼",
        ] {
            assert!(text.contains(token), "screen misses `{token}`");
        }
        for corner in ["┌", "┐", "└", "┘"] {
            assert!(!text.contains(corner), "outer frame leaked `{corner}`");
        }
    }

    #[test]
    fn render_warns_on_tiny_terminal() {
        use ratatui::backend::TestBackend;

        let (_tmp, root) = seeded_root();
        let app = App::new(&root, test_options()).unwrap();

        let backend = TestBackend::new(40, 16);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        assert!(screen_text(&terminal).contains("too small"));
    }

    #[test]
    fn render_marks_day_cells_with_day_color() {
        use ratatui::backend::TestBackend;

        let (_tmp, root) = seeded_root();
        let mut app = App::new(&root, test_options()).unwrap();
        app.selected = date(2026, 10, 7);
        app.year = 2026;
        app.month = 10;
        app.reload().unwrap();

        let backend = TestBackend::new(80, 26);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let green_title = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .any(|cell| cell.symbol() == "D" && cell.fg == Color::Green);
        assert!(green_title, "day event title must use the day color");
    }

    #[test]
    fn render_fixed_cells_form_a_centered_grid() {
        use ratatui::backend::TestBackend;

        let (_tmp, root) = seeded_root();
        let mut options = test_options();
        options.fixed_cells = true;
        let mut app = App::new(&root, options).unwrap();
        app.selected = date(2026, 10, 7);
        app.year = 2026;
        app.month = 10;
        app.reload().unwrap();

        let backend = TestBackend::new(80, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer();
        // Six inner vertical separators over six 3-tall week rows,
        // five horizontal ones: a 48x23 grid centered in 80 columns.
        assert_eq!(
            buffer
                .content()
                .iter()
                .filter(|cell| cell.symbol() == "┼")
                .count(),
            30
        );
        assert_eq!(
            buffer
                .content()
                .iter()
                .filter(|cell| cell.symbol() == "│")
                .count(),
            108
        );
        // Weekday names sit on the 6-wide cells: ratatui centers
        // `(6 / 2) - (3 / 2)`, so `Sun` starts at 16 + 2.
        assert_eq!(buffer[(18, 2)].symbol(), "S");
        // Wednesday Oct 7: third cell of the second week, number centered.
        assert_eq!(buffer[(40, 8)].symbol(), "7");
    }

    #[test]
    fn header_shows_today_hint_only_on_other_dates() {
        use ratatui::backend::TestBackend;

        let (_tmp, root) = seeded_root();
        let mut app = App::new(&root, test_options()).unwrap();
        app.selected = date(2026, 10, 7);
        app.year = 2026;
        app.month = 10;
        app.reload().unwrap();

        let backend = TestBackend::new(80, 26);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let text = screen_text(&terminal);
        let hint = format!("today {}", today().format("%A, %d.%m.%Y"));
        if today() == date(2026, 10, 7) {
            assert!(!text.contains(&hint), "hint duplicates today");
        } else {
            assert!(text.contains(&hint), "missing today hint");
        }
    }

    #[test]
    fn today_hint_logic() {
        let (_tmp, root) = seeded_root();
        let mut app = App::new(&root, test_options()).unwrap();
        // Other date + enabled: the exact hint string.
        app.selected = date(2020, 1, 1);
        assert_eq!(
            today_hint(&app),
            Some(format!("today {}", today().format("%A, %d.%m.%Y")))
        );
        // NONE hides the hint too.
        app.options.header = String::from("NONE");
        assert_eq!(today_hint(&app), None);
        // Today itself: no duplication.
        app.options.header = String::from("%A, %d.%m.%Y");
        app.selected = today();
        assert_eq!(today_hint(&app), None);
        // Switched off.
        app.selected = date(2020, 1, 1);
        app.options.show_today = false;
        assert_eq!(today_hint(&app), None);
    }

    #[test]
    fn none_header_renders_empty_row() {
        use ratatui::backend::TestBackend;

        let (_tmp, root) = seeded_root();
        let mut options = test_options();
        options.header = String::from("none");
        let mut app = App::new(&root, options).unwrap();
        app.selected = date(2026, 10, 10);
        app.year = 2026;
        app.month = 10;
        app.reload().unwrap();

        let backend = TestBackend::new(80, 26);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let text = screen_text(&terminal);
        assert!(!text.contains("10.10.2026"));
    }

    #[test]
    fn header_uses_custom_format() {
        use ratatui::backend::TestBackend;

        let (_tmp, root) = seeded_root();
        let mut options = test_options();
        options.header = String::from("%Y/%m/%d");
        options.show_today = false;
        let mut app = App::new(&root, options).unwrap();
        app.selected = date(2026, 10, 7);
        app.year = 2026;
        app.month = 10;
        app.reload().unwrap();

        let backend = TestBackend::new(80, 26);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let text = screen_text(&terminal);
        assert!(text.contains("2026/10/07"));
        assert!(!text.contains("today 2026"));
    }
}
