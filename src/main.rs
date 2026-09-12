mod app;
mod cli;
mod event;
mod fixture;
mod render;
mod smoke;
mod terminal;

use std::io;
use std::process::ExitCode;

use app::App;
use cli::Command;
use event::CrosstermEventSource;
use fixture::DemoFixture;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use terminal::{CrosstermOps, with_terminal};

fn main() -> ExitCode {
    match cli::parse(std::env::args_os().skip(1)) {
        Ok(Command::Demo) => match run_demo() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("reviewbox: {error}");
                ExitCode::FAILURE
            }
        },
        Ok(Command::DemoSmoke) => match smoke::run() {
            Ok(report) => {
                println!("ReviewBox demo smoke: ok ({} frames)", report.frames);
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("reviewbox demo smoke: {error}");
                ExitCode::FAILURE
            }
        },
        Ok(Command::Help) => {
            print!("{}", cli::USAGE);
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("reviewbox: {error}\n\n{}", cli::USAGE);
            ExitCode::from(2)
        }
    }
}

fn run_demo() -> io::Result<()> {
    with_terminal(CrosstermOps, || {
        let backend = CrosstermBackend::new(io::stdout());
        let mut terminal = Terminal::new(backend)?;
        terminal.clear()?;

        let fixture = DemoFixture::load();
        let mut app = App::new(fixture);
        let mut events = CrosstermEventSource;
        event::run(&mut terminal, &mut app, &mut events)
    })
}
