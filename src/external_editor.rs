use std::env;
use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};

use crate::comment_draft::MAX_DRAFT_CHARACTERS;
use crate::terminal::TerminalSuspend;

const MAX_EDITOR_BYTES: u64 = 1024 * 1024;
const CREATE_ATTEMPTS: u64 = 128;
static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditorCommand {
    pub program: OsString,
    pub args: Vec<OsString>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditorError {
    NotConfigured,
    Invalid,
    TempFile,
    Launch,
    Read,
    Decode,
    TooLarge,
    Terminal,
}

impl fmt::Display for EditorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NotConfigured => "no external editor is configured",
            Self::Invalid => "external editor configuration is invalid",
            Self::TempFile => "could not prepare a private editor file",
            Self::Launch => "external editor could not be run",
            Self::Read => "external editor content could not be read",
            Self::Decode => "external editor content is not valid UTF-8",
            Self::TooLarge => "external editor content exceeds the draft limit",
            Self::Terminal => "terminal could not switch to or from the external editor",
        })
    }
}

impl std::error::Error for EditorError {}

pub fn resolve_editor(
    lookup: impl Fn(&str) -> Option<OsString>,
) -> Result<EditorCommand, EditorError> {
    let visual = lookup("VISUAL");
    let editor = match visual {
        Some(value) if !os_value_is_blank(&value)? => Some(value),
        _ => lookup("EDITOR"),
    };
    let value = editor.ok_or(EditorError::NotConfigured)?;
    let value = value.into_string().map_err(|_| EditorError::Invalid)?;
    if value.trim().is_empty() {
        return Err(EditorError::NotConfigured);
    }
    let mut words = split_command(&value)?;
    if words.is_empty() || words[0].is_empty() {
        return Err(EditorError::Invalid);
    }
    let program = OsString::from(words.remove(0));
    let args = words.into_iter().map(OsString::from).collect();
    Ok(EditorCommand { program, args })
}

fn os_value_is_blank(value: &OsString) -> Result<bool, EditorError> {
    value
        .to_str()
        .map(|value| value.trim().is_empty())
        .ok_or(EditorError::Invalid)
}

fn split_command(value: &str) -> Result<Vec<String>, EditorError> {
    #[derive(Clone, Copy, Eq, PartialEq)]
    enum Quote {
        None,
        Single,
        Double,
    }

    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = Quote::None;
    let mut escaped = false;
    let mut started = false;

    for character in value.chars() {
        if escaped {
            word.push(character);
            escaped = false;
            started = true;
            continue;
        }
        match quote {
            Quote::Single => {
                if character == '\'' {
                    quote = Quote::None;
                } else {
                    word.push(character);
                }
                started = true;
            }
            Quote::Double => {
                if character == '"' {
                    quote = Quote::None;
                } else if character == '\\' {
                    escaped = true;
                } else {
                    word.push(character);
                }
                started = true;
            }
            Quote::None => match character {
                '\'' => {
                    quote = Quote::Single;
                    started = true;
                }
                '"' => {
                    quote = Quote::Double;
                    started = true;
                }
                '\\' => {
                    escaped = true;
                    started = true;
                }
                character if character.is_whitespace() => {
                    if started {
                        words.push(std::mem::take(&mut word));
                        started = false;
                    }
                }
                character => {
                    word.push(character);
                    started = true;
                }
            },
        }
    }
    if escaped || quote != Quote::None {
        return Err(EditorError::Invalid);
    }
    if started {
        words.push(word);
    }
    Ok(words)
}

pub trait EditorProcess {
    fn run(&mut self, command: &EditorCommand, draft_path: &Path) -> Result<bool, EditorError>;
}

#[derive(Default)]
pub struct CommandEditorProcess;

impl EditorProcess for CommandEditorProcess {
    fn run(&mut self, command: &EditorCommand, draft_path: &Path) -> Result<bool, EditorError> {
        Command::new(&command.program)
            .args(&command.args)
            .arg(draft_path)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .map(|status| status.success())
            .map_err(|_| EditorError::Launch)
    }
}

pub trait EditorTempFiles {
    fn create(&mut self, body: &str) -> Result<PathBuf, EditorError>;
    fn read(&mut self, path: &Path) -> Result<String, EditorError>;
    fn cleanup(&mut self, path: &Path) -> Result<(), EditorError>;
}

#[derive(Debug)]
pub struct PrivateEditorTempFiles {
    base: PathBuf,
}

impl PrivateEditorTempFiles {
    pub fn from_env(lookup: impl Fn(&str) -> Option<OsString>) -> Self {
        let base = lookup("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(env::temp_dir);
        Self { base }
    }

    pub fn from_process_env() -> Self {
        Self::from_env(|name| env::var_os(name))
    }

    #[cfg(test)]
    fn at(base: PathBuf) -> Self {
        Self { base }
    }

    fn create_directory(&self) -> Result<PathBuf, EditorError> {
        let process = u64::from(std::process::id());
        let epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        for attempt in 0..CREATE_ATTEMPTS {
            let sequence = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
            let directory = self.base.join(format!(
                "reviewbox-editor-{process}-{epoch:x}-{sequence:x}-{attempt:x}"
            ));
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            builder.mode(0o700);
            match builder.create(&directory) {
                Ok(()) => {
                    #[cfg(unix)]
                    if fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).is_err() {
                        let _ = fs::remove_dir(&directory);
                        return Err(EditorError::TempFile);
                    }
                    return Ok(directory);
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(_) => return Err(EditorError::TempFile),
            }
        }
        Err(EditorError::TempFile)
    }
}

impl EditorTempFiles for PrivateEditorTempFiles {
    fn create(&mut self, body: &str) -> Result<PathBuf, EditorError> {
        if body.len() as u64 >= MAX_EDITOR_BYTES || body.chars().count() > MAX_DRAFT_CHARACTERS {
            return Err(EditorError::TooLarge);
        }
        let directory = self.create_directory()?;
        let path = directory.join("draft.md");
        let result = (|| {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            options.mode(0o600);
            let mut file = options.open(&path).map_err(|_| EditorError::TempFile)?;
            #[cfg(unix)]
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|_| EditorError::TempFile)?;
            file.write_all(body.as_bytes())
                .and_then(|()| file.write_all(b"\n"))
                .and_then(|()| file.sync_all())
                .map_err(|_| EditorError::TempFile)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&path);
            let _ = fs::remove_dir(&directory);
        }
        result.map(|()| path)
    }

    fn read(&mut self, path: &Path) -> Result<String, EditorError> {
        let mut file = File::open(path).map_err(|_| EditorError::Read)?;
        if file.metadata().map_err(|_| EditorError::Read)?.len() > MAX_EDITOR_BYTES {
            return Err(EditorError::TooLarge);
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut file)
            .take(MAX_EDITOR_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| EditorError::Read)?;
        if bytes.len() as u64 > MAX_EDITOR_BYTES {
            return Err(EditorError::TooLarge);
        }
        let mut body = String::from_utf8(bytes).map_err(|_| EditorError::Decode)?;
        if body.ends_with('\n') {
            body.pop();
        }
        if body.chars().count() > MAX_DRAFT_CHARACTERS {
            return Err(EditorError::TooLarge);
        }
        Ok(body)
    }

    fn cleanup(&mut self, path: &Path) -> Result<(), EditorError> {
        let file_result = fs::remove_file(path);
        let directory_result = path.parent().map(fs::remove_dir).transpose();
        if file_result.is_err() || directory_result.is_err() {
            Err(EditorError::TempFile)
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EditorOutcome {
    Replaced(String),
    Unchanged,
    Failed(EditorError),
}

#[derive(Debug)]
pub struct EditorRunResult {
    pub outcome: EditorOutcome,
    pub resume: io::Result<()>,
    pub terminal_touched: bool,
}

pub fn edit_draft(
    body: &str,
    command: &EditorCommand,
    terminal: &mut dyn TerminalSuspend,
    process: &mut dyn EditorProcess,
    temp_files: &mut dyn EditorTempFiles,
) -> EditorRunResult {
    let path = match temp_files.create(body) {
        Ok(path) => path,
        Err(error) => {
            return EditorRunResult {
                outcome: EditorOutcome::Failed(error),
                resume: Ok(()),
                terminal_touched: false,
            };
        }
    };

    if terminal.suspend().is_err() {
        let _ = temp_files.cleanup(&path);
        return EditorRunResult {
            outcome: EditorOutcome::Failed(EditorError::Terminal),
            resume: terminal.resume(),
            terminal_touched: true,
        };
    }

    let outcome = match process.run(command, &path) {
        Ok(true) => match temp_files.read(&path) {
            Ok(body) if body.chars().any(|character| !character.is_whitespace()) => {
                EditorOutcome::Replaced(body)
            }
            Ok(_) => EditorOutcome::Unchanged,
            Err(error) => EditorOutcome::Failed(error),
        },
        Ok(false) => EditorOutcome::Unchanged,
        Err(error) => EditorOutcome::Failed(error),
    };
    let _ = temp_files.cleanup(&path);
    let resume = terminal.resume();
    EditorRunResult {
        outcome,
        resume,
        terminal_touched: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    #[cfg(unix)]
    use std::os::unix::ffi::OsStringExt;

    #[test]
    fn visual_precedes_editor_and_blank_visual_falls_through() {
        let values = BTreeMap::from([
            ("VISUAL", OsString::from("nvim -f")),
            ("EDITOR", OsString::from("vim --nofork")),
        ]);
        let command = resolve_editor(|name| values.get(name).cloned()).unwrap();
        assert_eq!(command.program, "nvim");
        assert_eq!(command.args, ["-f"]);

        let values = BTreeMap::from([
            ("VISUAL", OsString::from("  \t")),
            ("EDITOR", OsString::from("vim --nofork")),
        ]);
        let command = resolve_editor(|name| values.get(name).cloned()).unwrap();
        assert_eq!(command.program, "vim");
        assert_eq!(command.args, ["--nofork"]);
    }

    #[test]
    fn command_parser_supports_quotes_spaces_and_escapes_without_a_shell() {
        let command = resolve_editor(|name| {
            (name == "VISUAL").then(|| {
                OsString::from("'/opt/My Editor/nvim' -f --cmd \"set\\ number\" escaped\\ value")
            })
        })
        .unwrap();
        assert_eq!(command.program, "/opt/My Editor/nvim");
        assert_eq!(command.args, ["-f", "--cmd", "set number", "escaped value"]);
    }

    #[test]
    fn missing_unbalanced_empty_and_non_utf8_configuration_are_rejected() {
        assert_eq!(resolve_editor(|_| None), Err(EditorError::NotConfigured));
        assert_eq!(
            resolve_editor(|name| (name == "VISUAL").then(|| OsString::from("nvim 'oops"))),
            Err(EditorError::Invalid)
        );
        assert_eq!(
            resolve_editor(|name| (name == "VISUAL").then(|| OsString::from("''"))),
            Err(EditorError::Invalid)
        );
        #[cfg(unix)]
        assert_eq!(
            resolve_editor(|name| {
                (name == "VISUAL").then(|| OsString::from_vec(vec![0xff, 0xfe]))
            }),
            Err(EditorError::Invalid)
        );
    }

    struct NoTerminal;

    impl TerminalSuspend for NoTerminal {
        fn suspend(&mut self) -> io::Result<()> {
            Ok(())
        }
        fn resume(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct EditingProcess {
        replacement: Option<Vec<u8>>,
        success: Result<bool, EditorError>,
    }

    impl EditorProcess for EditingProcess {
        fn run(&mut self, command: &EditorCommand, draft_path: &Path) -> Result<bool, EditorError> {
            assert_eq!(command.program, "nvim");
            assert_eq!(command.args, ["-f"]);
            if let Some(replacement) = self.replacement.take() {
                fs::write(draft_path, replacement).unwrap();
            }
            self.success
        }
    }

    fn test_base() -> PathBuf {
        let root = env::temp_dir().join(format!(
            "reviewbox-external-editor-test-{}-{}",
            std::process::id(),
            NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        root
    }

    #[test]
    fn private_temp_file_round_trips_trailing_newline_and_is_cleaned() {
        let base = test_base();
        let mut temp_files = PrivateEditorTempFiles::at(base.clone());
        let path = temp_files.create("alpha\n").unwrap();
        assert!(path.starts_with(&base));
        assert!(!path.starts_with(env::current_dir().unwrap()));
        #[cfg(unix)]
        {
            assert_eq!(
                fs::metadata(path.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert_eq!(temp_files.read(&path).unwrap(), "alpha\n");
        let directory = path.parent().unwrap().to_path_buf();
        temp_files.cleanup(&path).unwrap();
        assert!(!path.exists());
        assert!(!directory.exists());
        fs::remove_dir(base).unwrap();
    }

    #[test]
    fn successful_edit_replaces_but_nonzero_exit_preserves_and_cleans_up() {
        let command = EditorCommand {
            program: "nvim".into(),
            args: vec!["-f".into()],
        };
        let base = test_base();
        let mut temp_files = PrivateEditorTempFiles::at(base.clone());
        let mut process = EditingProcess {
            replacement: Some(b"replacement\n".to_vec()),
            success: Ok(true),
        };
        let result = edit_draft(
            "prior",
            &command,
            &mut NoTerminal,
            &mut process,
            &mut temp_files,
        );
        assert!(matches!(
            result.outcome,
            EditorOutcome::Replaced(ref body) if body == "replacement"
        ));
        assert!(fs::read_dir(&base).unwrap().next().is_none());

        let mut process = EditingProcess {
            replacement: Some(b"ignored\n".to_vec()),
            success: Ok(false),
        };
        let result = edit_draft(
            "prior",
            &command,
            &mut NoTerminal,
            &mut process,
            &mut temp_files,
        );
        assert_eq!(result.outcome, EditorOutcome::Unchanged);
        assert!(fs::read_dir(&base).unwrap().next().is_none());

        let mut process = EditingProcess {
            replacement: Some(b" \n\t\n".to_vec()),
            success: Ok(true),
        };
        let result = edit_draft(
            "prior",
            &command,
            &mut NoTerminal,
            &mut process,
            &mut temp_files,
        );
        assert_eq!(result.outcome, EditorOutcome::Unchanged);
        assert!(fs::read_dir(&base).unwrap().next().is_none());
        fs::remove_dir(base).unwrap();
    }

    #[test]
    fn strict_read_rejects_invalid_utf8_byte_and_character_limits() {
        let base = test_base();
        let mut temp_files = PrivateEditorTempFiles::at(base.clone());
        let path = temp_files.create("prior").unwrap();
        fs::write(&path, [0xff]).unwrap();
        assert_eq!(temp_files.read(&path), Err(EditorError::Decode));
        fs::write(&path, vec![b'a'; MAX_EDITOR_BYTES as usize + 1]).unwrap();
        assert_eq!(temp_files.read(&path), Err(EditorError::TooLarge));
        fs::write(&path, "x".repeat(MAX_DRAFT_CHARACTERS + 1)).unwrap();
        assert_eq!(temp_files.read(&path), Err(EditorError::TooLarge));
        temp_files.cleanup(&path).unwrap();
        fs::remove_dir(base).unwrap();
    }

    #[test]
    fn oversized_input_is_rejected_before_a_temporary_directory_is_created() {
        let base = test_base();
        let mut temp_files = PrivateEditorTempFiles::at(base.clone());

        assert_eq!(
            temp_files.create(&"x".repeat(MAX_DRAFT_CHARACTERS + 1)),
            Err(EditorError::TooLarge)
        );
        assert!(fs::read_dir(&base).unwrap().next().is_none());
        fs::remove_dir(base).unwrap();
    }

    struct RecordingTerminal(Arc<Mutex<Vec<&'static str>>>);

    impl TerminalSuspend for RecordingTerminal {
        fn suspend(&mut self) -> io::Result<()> {
            self.0.lock().unwrap().push("suspend");
            Ok(())
        }
        fn resume(&mut self) -> io::Result<()> {
            self.0.lock().unwrap().push("resume");
            Ok(())
        }
    }

    struct RecordingTemp {
        log: Arc<Mutex<Vec<&'static str>>>,
        cleanup_result: Result<(), EditorError>,
    }

    impl EditorTempFiles for RecordingTemp {
        fn create(&mut self, _body: &str) -> Result<PathBuf, EditorError> {
            self.log.lock().unwrap().push("create");
            Ok(PathBuf::from("/private/draft.md"))
        }
        fn read(&mut self, _path: &Path) -> Result<String, EditorError> {
            self.log.lock().unwrap().push("read");
            Ok("changed".to_owned())
        }
        fn cleanup(&mut self, _path: &Path) -> Result<(), EditorError> {
            self.log.lock().unwrap().push("cleanup");
            self.cleanup_result
        }
    }

    struct RecordingProcess(Arc<Mutex<Vec<&'static str>>>);

    impl EditorProcess for RecordingProcess {
        fn run(
            &mut self,
            _command: &EditorCommand,
            _draft_path: &Path,
        ) -> Result<bool, EditorError> {
            self.0.lock().unwrap().push("launch");
            Ok(true)
        }
    }

    #[test]
    fn temp_creation_precedes_suspend_and_cleanup_precedes_resume() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut terminal = RecordingTerminal(Arc::clone(&log));
        let mut process = RecordingProcess(Arc::clone(&log));
        let mut temp_files = RecordingTemp {
            log: Arc::clone(&log),
            cleanup_result: Ok(()),
        };
        let command = EditorCommand {
            program: "nvim".into(),
            args: Vec::new(),
        };
        let result = edit_draft(
            "body",
            &command,
            &mut terminal,
            &mut process,
            &mut temp_files,
        );
        assert!(matches!(result.outcome, EditorOutcome::Replaced(_)));
        assert_eq!(
            *log.lock().unwrap(),
            ["create", "suspend", "launch", "read", "cleanup", "resume"]
        );
    }

    #[test]
    fn cleanup_failure_is_best_effort_and_does_not_replace_the_editor_outcome() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut terminal = RecordingTerminal(Arc::clone(&log));
        let mut process = RecordingProcess(Arc::clone(&log));
        let mut temp_files = RecordingTemp {
            log,
            cleanup_result: Err(EditorError::TempFile),
        };
        let command = EditorCommand {
            program: "nvim".into(),
            args: Vec::new(),
        };

        let result = edit_draft(
            "body",
            &command,
            &mut terminal,
            &mut process,
            &mut temp_files,
        );

        assert_eq!(
            result.outcome,
            EditorOutcome::Replaced("changed".to_owned())
        );
        assert!(result.resume.is_ok());
    }
}
