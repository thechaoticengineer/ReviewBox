use crate::fixture::DemoFixture;

#[derive(Debug)]
pub struct App {
    pub fixture: DemoFixture,
    pub terminal_width: u16,
    pub terminal_height: u16,
    should_quit: bool,
}

impl App {
    pub fn new(fixture: DemoFixture) -> Self {
        Self {
            fixture,
            terminal_width: 0,
            terminal_height: 0,
            should_quit: false,
        }
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        self.terminal_width = width;
        self.terminal_height = height;
    }

    pub fn quit(&mut self) {
        self.should_quit = true;
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }
}
