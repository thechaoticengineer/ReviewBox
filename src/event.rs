use std::io;

use crossterm::event::{
    self, Event as CrosstermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::layout::Rect;

use crate::app::{App, Input};
use crate::render;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppEvent {
    Input(Input),
    Resize(u16, u16),
}

pub fn translate_key(key: KeyEvent) -> Option<Input> {
    if key.kind != KeyEventKind::Press {
        return None;
    }

    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let command = match (key.code, control, alt) {
        (KeyCode::Char('c'), true, _) => Input::Quit,
        (KeyCode::Char('d'), true, _) => Input::HalfPageDown,
        (KeyCode::Char('u'), true, _) => Input::HalfPageUp,
        (KeyCode::Char(character), false, false) => Input::Character(character),
        (KeyCode::Enter, false, false) => Input::Enter,
        (KeyCode::Esc, false, false) => Input::Escape,
        (KeyCode::Backspace, false, false) => Input::Backspace,
        _ => Input::Unrelated,
    };
    Some(command)
}

pub trait EventSource {
    fn next(&mut self) -> io::Result<AppEvent>;
}

pub struct CrosstermEventSource;

impl EventSource for CrosstermEventSource {
    fn next(&mut self) -> io::Result<AppEvent> {
        loop {
            match event::read()? {
                CrosstermEvent::Key(key) => {
                    if let Some(input) = translate_key(key) {
                        return Ok(AppEvent::Input(input));
                    }
                }
                CrosstermEvent::Resize(width, height) => {
                    return Ok(AppEvent::Resize(width, height));
                }
                _ => {}
            }
        }
    }
}

pub fn run<B: Backend, E: EventSource>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    events: &mut E,
) -> io::Result<()>
where
    B::Error: std::error::Error + Send + Sync + 'static,
{
    while !app.should_quit() {
        terminal
            .draw(|frame| {
                let area = frame.area();
                app.resize(area.width, area.height);
                render::draw(frame, app);
            })
            .map_err(io::Error::other)?;

        match events.next()? {
            AppEvent::Input(input) => app.handle_input(input),
            AppEvent::Resize(width, height) => {
                terminal
                    .resize(Rect::new(0, 0, width, height))
                    .map_err(io::Error::other)?;
                app.resize(width, height);
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::DemoFixture;
    use ratatui::backend::TestBackend;
    use ratatui::{TerminalOptions, Viewport};

    struct Events(Vec<io::Result<AppEvent>>);

    impl EventSource for Events {
        fn next(&mut self) -> io::Result<AppEvent> {
            self.0.remove(0)
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn translates_printable_and_editing_keys_without_assigning_a_mode() {
        for (key, expected) in [
            (key(KeyCode::Char('h')), Input::Character('h')),
            (key(KeyCode::Char('j')), Input::Character('j')),
            (key(KeyCode::Char('/')), Input::Character('/')),
            (key(KeyCode::Char('?')), Input::Character('?')),
            (
                KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT),
                Input::Character('G'),
            ),
            (key(KeyCode::Enter), Input::Enter),
            (key(KeyCode::Esc), Input::Escape),
            (key(KeyCode::Backspace), Input::Backspace),
        ] {
            assert_eq!(translate_key(key), Some(expected));
        }
    }

    #[test]
    fn translates_control_movement_and_quit_keys() {
        for (character, expected) in [
            ('d', Input::HalfPageDown),
            ('u', Input::HalfPageUp),
            ('c', Input::Quit),
        ] {
            let key = KeyEvent::new(KeyCode::Char(character), KeyModifiers::CONTROL);
            assert_eq!(translate_key(key), Some(expected));
        }
    }

    #[test]
    fn printable_press_is_preserved_but_repeat_and_release_are_ignored() {
        assert_eq!(
            translate_key(key(KeyCode::Char('x'))),
            Some(Input::Character('x'))
        );
        for kind in [KeyEventKind::Repeat, KeyEventKind::Release] {
            let key = KeyEvent::new_with_kind(KeyCode::Char('j'), KeyModifiers::NONE, kind);
            assert_eq!(translate_key(key), None);
        }
    }

    #[test]
    fn resize_redraws_and_quit_exits() {
        let backend = TestBackend::new(80, 24);
        let options = TerminalOptions {
            viewport: Viewport::Fixed(Rect::new(0, 0, 80, 24)),
        };
        let mut terminal = Terminal::with_options(backend, options).expect("test terminal");
        let mut app = App::new(DemoFixture::load());
        let mut events = Events(vec![
            Ok(AppEvent::Resize(40, 10)),
            Ok(AppEvent::Input(Input::Character('q'))),
        ]);

        run(&mut terminal, &mut app, &mut events).expect("event loop succeeds");

        assert!(app.should_quit());
        assert_eq!((app.terminal_width, app.terminal_height), (40, 10));
    }

    #[test]
    fn inputs_reach_the_modal_reducer() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let mut app = App::new(DemoFixture::load());
        let mut events = Events(vec![
            Ok(AppEvent::Input(Input::Character('j'))),
            Ok(AppEvent::Input(Input::Character('l'))),
            Ok(AppEvent::Input(Input::Character('q'))),
        ]);

        run(&mut terminal, &mut app, &mut events).expect("event loop succeeds");

        assert_eq!(app.selected(crate::app::Pane::Repository), 1);
        assert_eq!(app.focus(), crate::app::Pane::Commit);
    }

    #[test]
    fn event_source_errors_are_propagated() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let mut app = App::new(DemoFixture::load());
        let mut events = Events(vec![Err(io::Error::other("forced event failure"))]);

        let error = run(&mut terminal, &mut app, &mut events).expect_err("error propagates");

        assert_eq!(error.to_string(), "forced event failure");
    }
}
