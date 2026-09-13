use std::io;

use crossterm::cursor::{Hide, Show};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

pub trait TerminalOps {
    fn enable_raw_mode(&mut self) -> io::Result<()>;
    fn enter_alternate_screen(&mut self) -> io::Result<()>;
    fn hide_cursor(&mut self) -> io::Result<()>;
    fn show_cursor(&mut self) -> io::Result<()>;
    fn leave_alternate_screen(&mut self) -> io::Result<()>;
    fn disable_raw_mode(&mut self) -> io::Result<()>;
}

pub trait TerminalSuspend {
    fn suspend(&mut self) -> io::Result<()>;
    fn resume(&mut self) -> io::Result<()>;
}

pub struct CrosstermOps;

impl TerminalOps for CrosstermOps {
    fn enable_raw_mode(&mut self) -> io::Result<()> {
        enable_raw_mode()
    }

    fn enter_alternate_screen(&mut self) -> io::Result<()> {
        execute!(io::stdout(), EnterAlternateScreen)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        execute!(io::stdout(), Hide)
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        execute!(io::stdout(), Show)
    }

    fn leave_alternate_screen(&mut self) -> io::Result<()> {
        execute!(io::stdout(), LeaveAlternateScreen)
    }

    fn disable_raw_mode(&mut self) -> io::Result<()> {
        disable_raw_mode()
    }
}

pub struct TerminalGuard<O: TerminalOps> {
    ops: O,
    raw_mode: bool,
    alternate_screen: bool,
    cursor_hidden: bool,
    restored: bool,
}

impl<O: TerminalOps> TerminalGuard<O> {
    pub fn acquire(ops: O) -> io::Result<Self> {
        let mut guard = Self {
            ops,
            raw_mode: false,
            alternate_screen: false,
            cursor_hidden: false,
            restored: false,
        };

        if let Err(setup_error) = guard.setup() {
            let cleanup_error = guard.restore().err();
            return Err(combine_errors(
                "terminal setup failed",
                setup_error,
                cleanup_error,
            ));
        }

        Ok(guard)
    }

    fn setup(&mut self) -> io::Result<()> {
        if !self.raw_mode {
            self.ops.enable_raw_mode()?;
            self.raw_mode = true;
        }

        if !self.alternate_screen {
            self.ops.enter_alternate_screen()?;
            self.alternate_screen = true;
        }

        if !self.cursor_hidden {
            self.ops.hide_cursor()?;
            self.cursor_hidden = true;
        }
        Ok(())
    }

    pub fn restore(&mut self) -> io::Result<()> {
        if self.restored {
            return Ok(());
        }

        let result = self.release("terminal restoration failed");
        self.restored = true;
        result
    }

    fn release(&mut self, context: &str) -> io::Result<()> {
        let mut errors = Vec::new();
        if self.cursor_hidden {
            if let Err(error) = self.ops.show_cursor() {
                errors.push(error);
            }
            self.cursor_hidden = false;
        }
        if self.alternate_screen {
            if let Err(error) = self.ops.leave_alternate_screen() {
                errors.push(error);
            }
            self.alternate_screen = false;
        }
        if self.raw_mode {
            if let Err(error) = self.ops.disable_raw_mode() {
                errors.push(error);
            }
            self.raw_mode = false;
        }
        if errors.is_empty() {
            Ok(())
        } else {
            let message = errors
                .into_iter()
                .map(|error| error.to_string())
                .collect::<Vec<_>>()
                .join("; ");
            Err(io::Error::other(format!("{context}: {message}")))
        }
    }
}

impl<O: TerminalOps> TerminalSuspend for TerminalGuard<O> {
    fn suspend(&mut self) -> io::Result<()> {
        if self.restored {
            return Err(io::Error::other("terminal session is already restored"));
        }
        self.release("terminal suspension failed")
    }

    fn resume(&mut self) -> io::Result<()> {
        if self.restored {
            return Err(io::Error::other("terminal session is already restored"));
        }
        self.setup()
    }
}

impl<O: TerminalOps> Drop for TerminalGuard<O> {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

#[cfg(test)]
pub fn with_terminal<O, F, T>(ops: O, run: F) -> io::Result<T>
where
    O: TerminalOps,
    F: FnOnce() -> io::Result<T>,
{
    with_terminal_session(ops, |_| run())
}

pub fn with_terminal_session<O, F, T>(ops: O, run: F) -> io::Result<T>
where
    O: TerminalOps,
    F: FnOnce(&mut TerminalGuard<O>) -> io::Result<T>,
{
    let mut guard = TerminalGuard::acquire(ops)?;
    let run_result = run(&mut guard);
    let restore_result = guard.restore();

    match (run_result, restore_result) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Err(run_error), Err(restore_error)) => Err(io::Error::other(format!(
            "application failed: {run_error}; {restore_error}"
        ))),
    }
}

fn combine_errors(context: &str, primary: io::Error, secondary: Option<io::Error>) -> io::Error {
    match secondary {
        Some(secondary) => io::Error::other(format!("{context}: {primary}; {secondary}")),
        None => io::Error::new(primary.kind(), format!("{context}: {primary}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::app::{App, Input};
    use crate::event::{AppEvent, EventSource, LoaderEventSource};
    use crate::github::LoadEvent;
    use crate::inbox::Inbox;

    #[derive(Clone)]
    struct RecordingOps {
        calls: Arc<Mutex<Vec<&'static str>>>,
        fail_on: Option<&'static str>,
    }

    impl RecordingOps {
        fn new(fail_on: Option<&'static str>) -> (Self, Arc<Mutex<Vec<&'static str>>>) {
            let calls = Arc::new(Mutex::new(Vec::new()));
            (
                Self {
                    calls: Arc::clone(&calls),
                    fail_on,
                },
                calls,
            )
        }

        fn record(&self, name: &'static str) -> io::Result<()> {
            self.calls.lock().expect("calls mutex").push(name);
            if self.fail_on == Some(name) {
                Err(io::Error::other(format!("forced {name} failure")))
            } else {
                Ok(())
            }
        }
    }

    impl TerminalOps for RecordingOps {
        fn enable_raw_mode(&mut self) -> io::Result<()> {
            self.record("enable_raw")
        }
        fn enter_alternate_screen(&mut self) -> io::Result<()> {
            self.record("enter_screen")
        }
        fn hide_cursor(&mut self) -> io::Result<()> {
            self.record("hide_cursor")
        }
        fn show_cursor(&mut self) -> io::Result<()> {
            self.record("show_cursor")
        }
        fn leave_alternate_screen(&mut self) -> io::Result<()> {
            self.record("leave_screen")
        }
        fn disable_raw_mode(&mut self) -> io::Result<()> {
            self.record("disable_raw")
        }
    }

    fn calls(log: &Arc<Mutex<Vec<&'static str>>>) -> Vec<&'static str> {
        log.lock().expect("calls mutex").clone()
    }

    #[test]
    fn normal_return_restores_in_reverse_order_once() {
        let (ops, log) = RecordingOps::new(None);
        with_terminal(ops, || Ok(())).expect("run succeeds");

        assert_eq!(
            calls(&log),
            [
                "enable_raw",
                "enter_screen",
                "hide_cursor",
                "show_cursor",
                "leave_screen",
                "disable_raw"
            ]
        );
    }

    #[test]
    fn propagated_application_error_still_restores_terminal() {
        let (ops, log) = RecordingOps::new(None);
        let error = with_terminal(ops, || -> io::Result<()> {
            Err(io::Error::other("forced application failure"))
        })
        .expect_err("run fails");

        assert_eq!(error.to_string(), "forced application failure");
        assert_eq!(
            &calls(&log)[3..],
            ["show_cursor", "leave_screen", "disable_raw"]
        );
    }

    #[test]
    fn partial_setup_failure_restores_only_acquired_resources() {
        let (ops, log) = RecordingOps::new(Some("hide_cursor"));
        let result = TerminalGuard::acquire(ops);

        assert!(result.is_err());
        assert_eq!(
            calls(&log),
            [
                "enable_raw",
                "enter_screen",
                "hide_cursor",
                "leave_screen",
                "disable_raw"
            ]
        );
    }

    #[test]
    fn explicit_restore_is_idempotent() {
        let (ops, log) = RecordingOps::new(None);
        let mut guard = TerminalGuard::acquire(ops).expect("setup succeeds");

        guard.restore().expect("first restore succeeds");
        guard.restore().expect("second restore is a no-op");
        drop(guard);

        assert_eq!(calls(&log).len(), 6);
    }

    #[test]
    fn suspend_and_resume_use_exact_terminal_order_and_restore_remains_idempotent() {
        let (ops, log) = RecordingOps::new(None);
        let mut guard = TerminalGuard::acquire(ops).expect("setup succeeds");

        guard.suspend().expect("suspend succeeds");
        guard.resume().expect("resume succeeds");
        guard.restore().expect("restore succeeds");
        guard.restore().expect("second restore is a no-op");
        drop(guard);

        assert_eq!(
            calls(&log),
            [
                "enable_raw",
                "enter_screen",
                "hide_cursor",
                "show_cursor",
                "leave_screen",
                "disable_raw",
                "enable_raw",
                "enter_screen",
                "hide_cursor",
                "show_cursor",
                "leave_screen",
                "disable_raw",
            ]
        );
    }

    #[derive(Clone)]
    struct ResumeFailOps {
        calls: Arc<Mutex<Vec<&'static str>>>,
        enable_count: usize,
    }

    impl ResumeFailOps {
        fn record(&mut self, name: &'static str) -> io::Result<()> {
            self.calls.lock().expect("calls mutex").push(name);
            if name == "enable_raw" {
                self.enable_count += 1;
                if self.enable_count == 2 {
                    return Err(io::Error::other("forced resume failure"));
                }
            }
            Ok(())
        }
    }

    impl TerminalOps for ResumeFailOps {
        fn enable_raw_mode(&mut self) -> io::Result<()> {
            self.record("enable_raw")
        }
        fn enter_alternate_screen(&mut self) -> io::Result<()> {
            self.record("enter_screen")
        }
        fn hide_cursor(&mut self) -> io::Result<()> {
            self.record("hide_cursor")
        }
        fn show_cursor(&mut self) -> io::Result<()> {
            self.record("show_cursor")
        }
        fn leave_alternate_screen(&mut self) -> io::Result<()> {
            self.record("leave_screen")
        }
        fn disable_raw_mode(&mut self) -> io::Result<()> {
            self.record("disable_raw")
        }
    }

    #[test]
    fn failed_resume_leaves_final_restore_idempotent_without_repeating_steps() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let ops = ResumeFailOps {
            calls: Arc::clone(&log),
            enable_count: 0,
        };
        let mut guard = TerminalGuard::acquire(ops).expect("setup succeeds");
        guard.suspend().expect("suspend succeeds");

        assert!(guard.resume().is_err());
        guard.restore().expect("no acquired resources remain");
        drop(guard);

        assert_eq!(
            calls(&log),
            [
                "enable_raw",
                "enter_screen",
                "hide_cursor",
                "show_cursor",
                "leave_screen",
                "disable_raw",
                "enable_raw",
            ]
        );
    }

    #[test]
    fn drop_is_a_cleanup_fallback() {
        let (ops, log) = RecordingOps::new(None);
        let guard = TerminalGuard::acquire(ops).expect("setup succeeds");

        drop(guard);

        assert_eq!(
            &calls(&log)[3..],
            ["show_cursor", "leave_screen", "disable_raw"]
        );
    }

    #[test]
    fn cleanup_continues_after_one_restore_operation_fails() {
        let (ops, log) = RecordingOps::new(Some("show_cursor"));
        let error = with_terminal(ops, || Ok(())).expect_err("restore fails");

        assert!(error.to_string().contains("show_cursor"));
        assert_eq!(
            &calls(&log)[3..],
            ["show_cursor", "leave_screen", "disable_raw"]
        );
    }

    struct QuitAfterTick(bool);

    impl EventSource for QuitAfterTick {
        fn poll(&mut self, _timeout: Duration) -> io::Result<Option<AppEvent>> {
            if self.0 {
                Ok(Some(AppEvent::Input(Input::Character('q'))))
            } else {
                self.0 = true;
                Ok(None)
            }
        }
    }

    struct ActiveLoader {
        progress_sent: bool,
        cancelled: Arc<AtomicBool>,
    }

    impl LoaderEventSource for ActiveLoader {
        fn try_next(&mut self) -> Option<LoadEvent> {
            if self.progress_sent {
                None
            } else {
                self.progress_sent = true;
                Some(LoadEvent::DiscoveryPage {
                    page: 1,
                    owned_repositories: 1,
                })
            }
        }

        fn cancel(&mut self) {
            self.cancelled.store(true, Ordering::Release);
        }
    }

    #[test]
    fn quitting_during_loading_cancels_work_and_restores_terminal() {
        let (ops, log) = RecordingOps::new(None);
        let cancelled = Arc::new(AtomicBool::new(false));
        let loader_cancelled = Arc::clone(&cancelled);

        with_terminal(ops, || {
            let backend = TestBackend::new(100, 24);
            let mut terminal = Terminal::new(backend).map_err(io::Error::other)?;
            let selection = crate::day::select_day(
                crate::day::parse_date("2024-01-15").unwrap(),
                crate::day::parse_timezone("Etc/UTC").unwrap(),
                crate::day::TimezoneSource::Explicit,
            )
            .unwrap();
            let mut app = App::new(Inbox::live(selection));
            let mut events = QuitAfterTick(false);
            let mut loader = ActiveLoader {
                progress_sent: false,
                cancelled: loader_cancelled,
            };
            let mut details = crate::event::NoDetails;
            crate::event::run_with_loader(
                &mut terminal,
                &mut app,
                &mut events,
                &mut loader,
                &mut details,
            )
        })
        .expect("quit succeeds");

        assert!(cancelled.load(Ordering::Acquire));
        assert_eq!(
            calls(&log),
            [
                "enable_raw",
                "enter_screen",
                "hide_cursor",
                "show_cursor",
                "leave_screen",
                "disable_raw"
            ]
        );
    }
}
