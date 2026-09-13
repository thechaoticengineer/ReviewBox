use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const APPLICATION_DIRECTORY: &str = "reviewbox";
static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(0);

pub(crate) fn data_file_path(
    lookup: impl Fn(&str) -> Option<OsString>,
    file_name: &str,
) -> Option<PathBuf> {
    let data_home = lookup("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            lookup("HOME")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .map(|home| home.join(".local").join("share"))
        })?;

    Some(data_home.join(APPLICATION_DIRECTORY).join(file_name))
}

pub(crate) fn atomic_replace(
    path: &Path,
    file_name: &str,
    bytes: &[u8],
    before_rename: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    create_private_directory(parent)?;

    let (temporary_path, mut temporary_file) = create_private_temp_file(parent, file_name)?;
    let result = (|| {
        temporary_file.write_all(bytes)?;
        temporary_file.sync_all()?;
        drop(temporary_file);
        before_rename()?;
        fs::rename(&temporary_path, path)?;
        sync_directory_best_effort(parent);
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

pub(crate) fn create_private_directory(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

pub(crate) fn create_private_temp_file(
    parent: &Path,
    file_name: &str,
) -> io::Result<(PathBuf, File)> {
    loop {
        let sequence = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".{file_name}.tmp-{}-{sequence}",
            std::process::id()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
}

#[cfg(unix)]
fn sync_directory_best_effort(path: &Path) {
    if let Ok(directory) = File::open(path) {
        let _ = directory.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_directory_best_effort(_path: &Path) {}
