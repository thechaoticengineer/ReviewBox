use std::io;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::layout::Rect;

use crate::app::App;
use crate::render;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppEvent {
    Quit,
    Resize(u16, u16),
    Ignore,
}

pub trait EventSource {
    fn next(&mut self) -> io::Result<AppEvent>;
}

pub struct CrosstermEventSource;

impl EventSource for CrosstermEventSource {
    fn next(&mut self) -> io::Result<AppEvent> {
        loop {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    let quit = key.code == KeyCode::Char('q')
                        || (key.code == KeyCode::Char('c')
                            && key.modifiers.contains(KeyModifiers::CONTROL));
                    return Ok(if quit {
                        AppEvent::Quit
                    } else {
                        AppEvent::Ignore
                    });
                }
                Event::Resize(width, height) => return Ok(AppEvent::Resize(width, height)),
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
            AppEvent::Quit => app.quit(),
            AppEvent::Resize(width, height) => {
                terminal
                    .resize(Rect::new(0, 0, width, height))
                    .map_err(io::Error::other)?;
                app.resize(width, height);
            }
            AppEvent::Ignore => {}
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

    #[test]
    fn resize_redraws_and_quit_exits() {
        let backend = TestBackend::new(80, 24);
        let options = TerminalOptions {
            viewport: Viewport::Fixed(Rect::new(0, 0, 80, 24)),
        };
        let mut terminal = Terminal::with_options(backend, options).expect("test terminal");
        let mut app = App::new(DemoFixture::load());
        let mut events = Events(vec![Ok(AppEvent::Resize(40, 10)), Ok(AppEvent::Quit)]);

        run(&mut terminal, &mut app, &mut events).expect("event loop succeeds");

        assert!(app.should_quit());
        assert_eq!((app.terminal_width, app.terminal_height), (40, 10));
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
