//! Terminal frontend built on ratatui — used when there is no Wayland session
//! (`foto` auto-falls back to it) or when forced with `foto --tui`.

use crate::state::{run_action, Action, App, Flow, Key};
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
};
use std::io::{stdout, Stdout};
use std::time::Duration;

pub fn run(app: &mut App) -> Result<()> {
    let mut terminal = setup()?;
    let result = event_loop(app, &mut terminal);
    restore()?;
    // dmenu output must wait until we are out of the alternate screen,
    // otherwise the chosen line disappears with the terminal reset
    if let Ok(Some(line)) = &result {
        println!("{line}");
    }
    result.map(|_| ())
}

fn setup() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout());
    Ok(Terminal::new(backend)?)
}

fn restore() -> Result<()> {
    disable_raw_mode()?;
    execute!(stdout(), LeaveAlternateScreen)?;
    Ok(())
}

fn event_loop(
    app: &mut App,
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
) -> Result<Option<String>> {
    loop {
        terminal.draw(|frame| ui(frame, app))?;

        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(ev) = event::read()? {
                // Wayland terminals can send Press+Release; we only want Press.
                if ev.kind == KeyEventKind::Press {
                    match app.on_key(to_key(ev)) {
                        Flow::Exit => return Ok(None),
                        Flow::Launch(Action::Print(line)) => return Ok(Some(line)),
                        Flow::Launch(action) => {
                            run_action(&action)?;
                            return Ok(None);
                        }
                        Flow::Continue => {}
                    }
                }
            }
        }
    }
}

/// Map a crossterm key event onto our frontend-neutral [`Key`].
fn to_key(ev: KeyEvent) -> Key {
    let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
    match ev.code {
        KeyCode::Esc => Key::Escape,
        KeyCode::Enter => Key::Enter,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Home => Key::Top,
        KeyCode::End => Key::Bottom,
        KeyCode::Tab => Key::Tab,
        KeyCode::Char('c') if ctrl => Key::Escape,
        KeyCode::Char('a') if ctrl => Key::Top, // readline: caret to start
        KeyCode::Char('e') if ctrl => Key::Bottom, // readline: caret to end
        KeyCode::Char('w') if ctrl => Key::WordDelete,
        KeyCode::Char('u') if ctrl => Key::ClearLine,
        KeyCode::Char('j') if ctrl => Key::Down,
        KeyCode::Char('k') if ctrl => Key::Up,
        KeyCode::Char(c) if !ctrl && !ev.modifiers.contains(KeyModifiers::ALT) => Key::Char(c),
        _ => Key::Other,
    }
}

fn ui(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // outer frame with config colors; the title shows the active mode
    // (apps / images / menu) so Tab switching is visible in the TUI too
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(rgb(app.cfg.colors.border)))
        .title(format!(" foto · {} ", app.mode_label()));
    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    // prompt line: "> query▏" — the caret ▏ sits at the cursor position
    // (mid-string while editing, at the end otherwise)
    let (before_caret, after_caret) = app.caret_split();
    let prompt = Paragraph::new(Line::from(vec![
        Span::styled("> ", Style::new().fg(rgb(app.cfg.colors.border))),
        Span::raw(before_caret),
        Span::styled("▏", Style::new().fg(rgb(app.cfg.colors.border))),
        Span::raw(after_caret),
    ]));
    let prompt_area = Rect { height: 1, ..inner };
    frame.render_widget(prompt, prompt_area);

    // list of matches that fits below the prompt
    let rows = inner.height.saturating_sub(1) as usize;
    if rows == 0 {
        return;
    }
    let items: Vec<ListItem> = app
        .matches
        .iter()
        .map(|m| {
            let entry = &app.entries[m.index];
            let mut spans = vec![Span::raw(entry.title.as_str())];
            if !entry.subtitle.is_empty() {
                spans.push(Span::styled(
                    format!("  {}", entry.subtitle),
                    Style::new().dim(),
                ));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    let mut list_state = ListState::default();
    list_state.select(Some(app.selected));

    let list = List::new(items)
        .highlight_style(Style::new().bg(rgb(app.cfg.colors.selected)))
        .highlight_symbol("› ");
    let list_area = Rect {
        y: inner.y + 1,
        height: inner.height.saturating_sub(1),
        ..inner
    };
    frame.render_stateful_widget(list, list_area, &mut list_state);
}

fn rgb(c: crate::config::Rgba) -> Color {
    Color::Rgb(c[0], c[1], c[2])
}
