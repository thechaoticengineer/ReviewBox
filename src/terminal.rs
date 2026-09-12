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
        self.ops.enable_raw_mode()?;
        self.raw_mode = true;

        self.ops.enter_alternate_screen()?;
        self.alternate_screen = true;

        self.ops.hide_cursor()?;
        self.cursor_hidden = true;
        Ok(())
    }

    pub fn restore(&mut self) -> io::Result<()> {
        if self.restored {
            return Ok(());
        }

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
        self.restored = true;

        if errors.is_empty() {
            Ok(())
        } else {
            let message = errors
                .into_iter()
                .map(|error| error.to_string())
                .collect::<Vec<_>>()
                .join("; ");
            Err(io::Error::other(format!(
                "terminal restoration failed: {message}"
            )))
        }
    }
}

impl<O: TerminalOps> Drop for TerminalGuard<O> {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

pub fn with_terminal<O, F, T>(ops: O, run: F) -> io::Result<T>
where
    O: TerminalOps,
    F: FnOnce() -> io::Result<T>,
{
    let mut guard = TerminalGuard::acquire(ops)?;
    let run_result = run();
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
    use std::sync::{Arc, Mutex};

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
}
