mod app;
mod cli;
mod day;
mod event;
mod fixture;
pub mod github;
mod inbox;
mod loader;
mod render;
mod smoke;
mod terminal;

use std::io;
use std::process::ExitCode;

use app::App;
use cli::Command;
use event::CrosstermEventSource;
use fixture::DemoFixture;
use inbox::Inbox;
use inbox::InboxSource;
use loader::LoaderSession;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use terminal::{CrosstermOps, with_terminal};

fn main() -> ExitCode {
    match cli::parse(std::env::args_os().skip(1)) {
        Ok(Command::Live(selection)) => match run_inbox(Inbox::live(selection)) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("reviewbox: {error}");
                ExitCode::FAILURE
            }
        },
        Ok(Command::Demo) => match run_inbox(DemoFixture::load()) {
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

fn run_inbox(inbox: Inbox) -> io::Result<()> {
    let selection = match &inbox.source {
        InboxSource::Live { selection } => Some(selection.clone()),
        InboxSource::Demo => None,
    };
    with_terminal(CrosstermOps, || {
        let backend = CrosstermBackend::new(io::stdout());
        let mut terminal = Terminal::new(backend)?;
        terminal.clear()?;

        let mut app = App::new(inbox);
        let mut events = CrosstermEventSource;
        if let Some(selection) = selection {
            let mut loader = LoaderSession::start(selection);
            let result = event::run_with_loader(&mut terminal, &mut app, &mut events, &mut loader);
            loader.shutdown();
            result
        } else {
            event::run(&mut terminal, &mut app, &mut events)
        }
    })
}
