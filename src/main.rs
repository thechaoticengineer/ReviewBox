mod app;
mod cli;
mod comment;
pub mod comment_draft;
mod day;
mod detail;
mod diff_view;
mod event;
mod external_editor;
mod fixture;
pub mod github;
mod inbox;
mod loader;
mod private_file;
mod render;
pub mod review_state;
mod smoke;
mod terminal;
mod ui_layout;

use std::io;
use std::process::ExitCode;

use app::App;
use cli::Command;
use comment::CommentSession;
use comment_draft::{DraftStore, FileDraftStore, MemoryDraftStore};
use day::DaySelection;
use detail::DetailSession;
use event::{
    CrosstermEventSource, DetailRequester, ExternalEditorSession, FakeComments, LoaderEventSource,
    NoDetails,
};
use external_editor::{CommandEditorProcess, PrivateEditorTempFiles};
use fixture::{DemoFixture, DemoHistoryPager};
use github::{CommandRunner, GitHubLoader};
use inbox::{BacklogOrigin, Inbox, InboxSource};
use loader::{BacklogSession, LoaderSession};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use review_state::{FileReviewStore, ReviewStore};
use terminal::{CrosstermOps, with_terminal_session};

fn main() -> ExitCode {
    match cli::parse(std::env::args_os().skip(1)) {
        Ok(Command::Backlog) => match run_inbox(Inbox::backlog(BacklogOrigin::GitHub)) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("reviewbox: {error}");
                ExitCode::FAILURE
            }
        },
        Ok(Command::Live(selection)) => match run_inbox(Inbox::live(selection)) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("reviewbox: {error}");
                ExitCode::FAILURE
            }
        },
        Ok(Command::Demo) => match run_inbox(Inbox::backlog(BacklogOrigin::Demo)) {
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

enum Services {
    Day(DaySelection),
    GitHubBacklog,
    DemoBacklog,
    StaticDemo,
}

fn run_inbox(inbox: Inbox) -> io::Result<()> {
    let services = match &inbox.source {
        InboxSource::Live { selection } => Services::Day(selection.clone()),
        InboxSource::Backlog {
            origin: BacklogOrigin::GitHub,
        } => Services::GitHubBacklog,
        InboxSource::Backlog {
            origin: BacklogOrigin::Demo,
        } => Services::DemoBacklog,
        InboxSource::Demo => Services::StaticDemo,
    };
    let mut app = match services {
        Services::Day(_) | Services::GitHubBacklog => App::with_store_results(
            inbox,
            FileReviewStore::from_process_env()
                .map(|store| Box::new(store) as Box<dyn ReviewStore>),
            FileDraftStore::from_process_env().map(|store| Box::new(store) as Box<dyn DraftStore>),
        ),
        Services::DemoBacklog => App::with_stores(
            inbox,
            Box::new(DemoFixture::review_store()),
            Box::new(MemoryDraftStore::default()),
        ),
        Services::StaticDemo => App::new(inbox),
    };
    with_terminal_session(CrosstermOps, |terminal_session| {
        let backend = CrosstermBackend::new(io::stdout());
        let mut terminal = Terminal::new(backend)?;
        terminal.clear()?;

        let mut events = CrosstermEventSource;
        let mut editor_process = CommandEditorProcess;
        let mut editor_temp_files = PrivateEditorTempFiles::from_process_env();
        let editor_lookup = |name: &str| std::env::var_os(name);
        let mut editor = ExternalEditorSession::new(
            terminal_session,
            &mut editor_process,
            &mut editor_temp_files,
            &editor_lookup,
        );
        match services {
            Services::Day(selection) => {
                let mut loader = LoaderSession::start(selection);
                run_live(
                    &mut terminal,
                    &mut app,
                    &mut events,
                    &mut loader,
                    &mut editor,
                )
            }
            Services::GitHubBacklog => {
                let mut loader = BacklogSession::start(GitHubLoader::new(CommandRunner));
                run_live(
                    &mut terminal,
                    &mut app,
                    &mut events,
                    &mut loader,
                    &mut editor,
                )
            }
            Services::DemoBacklog => {
                let mut loader = BacklogSession::start(DemoHistoryPager::interactive());
                let mut details = NoDetails;
                let mut comments = FakeComments::default();
                let result = event::run_with_services_and_editor(
                    &mut terminal,
                    &mut app,
                    &mut events,
                    &mut loader,
                    &mut details,
                    &mut comments,
                    &mut editor,
                );
                loader.shutdown();
                result
            }
            Services::StaticDemo => {
                event::run_with_editor(&mut terminal, &mut app, &mut events, &mut editor)
            }
        }
    })
}

/// Run a GitHub-backed session with real detail and comment services, then
/// cancel and join every worker.
fn run_live<B: ratatui::backend::Backend, L: LoaderEventSource>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    events: &mut CrosstermEventSource,
    loader: &mut L,
    editor: &mut ExternalEditorSession<'_>,
) -> io::Result<()>
where
    B::Error: std::error::Error + Send + Sync + 'static,
{
    let mut details = DetailSession::new();
    let mut comments = CommentSession::new();
    let result = event::run_with_services_and_editor(
        terminal,
        app,
        events,
        loader,
        &mut details,
        &mut comments,
        editor,
    );
    loader.cancel();
    details.shutdown();
    event::CommentRequester::shutdown(&mut comments);
    result
}
