//! Terminal frontend built on ratatui — used when there is no Wayland session
//! (`foto` auto-falls back to it) or when forced with `foto --tui`.
//! The mouse works here too: click a row to launch it, scroll the wheel to
//! move the selection — the same behavior the Wayland GUI has.

use crate::state::{run_action, Action, App, Flow, Key};
use anyhow::Result;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    },
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
    install_panic_hook();
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

/// The release profile uses `panic = "abort"`, which skips unwinding — so
/// no Drop guard on the terminal would run after a panic. The hook below
/// runs *before* the abort and puts the terminal back into a usable state
/// (raw mode off, alternate screen closed, mouse capture released), then
/// delegates to the previous hook for the standard panic message.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore();
        previous(info);
    }));
}

fn setup() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout());
    Ok(Terminal::new(backend)?)
}

fn restore() -> Result<()> {
    disable_raw_mode()?;
    execute!(stdout(), LeaveAlternateScreen, DisableMouseCapture)?;
    Ok(())
}

fn event_loop(
    app: &mut App,
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
) -> Result<Option<String>> {
    // held across frames so ratatui remembers the scroll offset the list was
    // last drawn with — a click maps through it back to a match index
    let mut list_state = ListState::default();
    loop {
        terminal.draw(|frame| ui(frame, app, &mut list_state))?;

        if event::poll(Duration::from_millis(200))? {
            let outcome = match event::read()? {
                Event::Key(ev) if ev.kind == KeyEventKind::Press => {
                    // Wayland terminals can send Press+Release; only Press
                    let flow = app.on_key(to_key(ev));
                    interpret(flow)?
                }
                Event::Mouse(me) => {
                    let s = terminal.size()?;
                    let size = Rect {
                        x: 0,
                        y: 0,
                        width: s.width,
                        height: s.height,
                    };
                    match mouse_flow(app, me, &list_state, size) {
                        Some(flow) => interpret(flow)?,
                        None => Outcome::Continue,
                    }
                }
                _ => Outcome::Continue,
            };
            match outcome {
                Outcome::Continue => {}
                Outcome::Quit => return Ok(None),
                Outcome::Print(line) => return Ok(Some(line)),
            }
        }
    }
}

/// What one input event asks the TUI to do next.
enum Outcome {
    Continue,
    /// leave the TUI (Esc, or a launch happened)
    Quit,
    /// dmenu mode: print this line after the terminal is restored
    Print(String),
}

/// Turn a [`Flow`] from the shared state into a TUI [`Outcome`].
fn interpret(flow: Flow) -> Result<Outcome> {
    Ok(match flow {
        Flow::Continue => Outcome::Continue,
        Flow::Exit => Outcome::Quit,
        Flow::Launch(Action::Print(line)) => Outcome::Print(line),
        Flow::Launch(action) => {
            run_action(&action)?;
            Outcome::Quit
        }
    })
}

/// Mouse events → an optional [`Flow`]: a left click selects the row under
/// the pointer and activates it (exactly like the GUI), the wheel scrolls.
fn mouse_flow(app: &mut App, me: MouseEvent, list_state: &ListState, size: Rect) -> Option<Flow> {
    match me.kind {
        MouseEventKind::ScrollUp => Some(app.on_key(Key::Up)),
        MouseEventKind::ScrollDown => Some(app.on_key(Key::Down)),
        MouseEventKind::Down(MouseButton::Left) => {
            let index = clicked_index(
                list_top(size),
                list_state.offset(),
                app.matches.len(),
                visible_rows(size),
                me.row,
            )?;
            app.selected = index;
            Some(app.on_key(Key::Enter))
        }
        _ => None,
    }
}

/// Terminal row where the first list item is drawn: top border (1) + the
/// prompt line (1) below the frame's top edge.
fn list_top(size: Rect) -> u16 {
    size.y + 2
}

/// How many list rows fit: the frame minus both borders and the prompt.
fn visible_rows(size: Rect) -> usize {
    size.height.saturating_sub(3) as usize
}

/// Match index for a click at terminal `row`, or None when the click is on
/// a border, the prompt, an undrawn row or empty space.
fn clicked_index(
    list_top: u16,
    offset: usize,
    len: usize,
    visible: usize,
    row: u16,
) -> Option<usize> {
    let row = row.checked_sub(list_top)? as usize;
    if row >= visible {
        return None;
    }
    let index = offset + row;
    (index < len).then_some(index)
}

/// Map a crossterm key event onto our frontend-neutral [`Key`].
fn to_key(ev: KeyEvent) -> Key {
    let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
    match ev.code {
        KeyCode::Esc => Key::Escape,
        // Shift+Enter opens an app's actions submenu (when it has any)
        KeyCode::Enter => {
            if ev.modifiers.contains(KeyModifiers::SHIFT) {
                Key::Actions
            } else {
                Key::Enter
            }
        }
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

fn ui(frame: &mut Frame, app: &App, list_state: &mut ListState) {
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

    list_state.select(Some(app.selected));

    let list = List::new(items)
        .highlight_style(Style::new().bg(rgb(app.cfg.colors.selected)))
        .highlight_symbol("› ");
    let list_area = Rect {
        y: inner.y + 1,
        height: inner.height.saturating_sub(1),
        ..inner
    };
    frame.render_stateful_widget(list, list_area, list_state);
}

fn rgb(c: crate::config::Rgba) -> Color {
    Color::Rgb(c[0], c[1], c[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clicks_map_to_rows_and_reject_the_rest() {
        // list_top = 2 (border + prompt), 20 matches, 10 rows visible
        let (top, len, vis) = (2, 20, 10);
        assert_eq!(clicked_index(top, 0, len, vis, 2), Some(0), "first row");
        assert_eq!(clicked_index(top, 5, len, vis, 4), Some(7), "offset + row");
        assert_eq!(clicked_index(top, 10, len, vis, 11), Some(19), "last row");
        assert_eq!(clicked_index(top, 10, len, vis, 12), None, "past the list");
        assert_eq!(clicked_index(top, 0, len, vis, 1), None, "the prompt line");
        assert_eq!(clicked_index(top, 0, len, vis, 0), None, "the border");
        assert_eq!(clicked_index(top, 0, 3, vis, 5), None, "empty space");
        // no underflow panic on clicks above the frame
        assert_eq!(clicked_index(top, 0, len, vis, 0), None);
    }

    #[test]
    fn shift_enter_maps_to_actions() {
        let ev = |mods: KeyModifiers| KeyEvent {
            code: KeyCode::Enter,
            modifiers: mods,
            kind: KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        };
        assert_eq!(to_key(ev(KeyModifiers::SHIFT)), Key::Actions);
        assert_eq!(to_key(ev(KeyModifiers::NONE)), Key::Enter);
    }
}
