//! Bounded serialization and durable file replacement for profiles and memory.
//! Callers retain payload validation and parent-directory ownership.
use serde::Serialize;
use std::{io, io::Write, path::Path};

pub(crate) fn replace(temporary: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        };
        let from: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = destination
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        if from[..from.len() - 1].contains(&0) || to[..to.len() - 1].contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "NUL in file path",
            ));
        }
        // Both paths are owned NUL-terminated UTF-16 buffers for this call.
        if unsafe {
            MoveFileExW(
                from.as_ptr(),
                to.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        std::fs::rename(temporary, destination)?;
        sync_directory(
            destination
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new(".")),
        )
    }
}

pub(crate) fn sync_directory(directory: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::fs::File::open(directory)?.sync_all()
    }
    // Windows has no unprivileged equivalent of Unix directory fsync. File
    // contents are flushed before replacement; replace uses WRITE_THROUGH.
    #[cfg(not(unix))]
    {
        let _ = directory;
        Ok(())
    }
}

/// Replace a file using a private sibling, flushing contents before commit.
/// The caller owns parent-directory creation, payload validation and size bounds.
pub fn write(path: &Path, data: &[u8]) -> io::Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(data)?;
        file.sync_all()?;
        drop(file);
        replace(&temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// Serialize within a hard byte budget before opening any file.
pub fn write_json<T: Serialize + ?Sized>(
    path: &Path,
    data: &T,
    limit: usize,
) -> Result<(), String> {
    struct BoundedBytes {
        bytes: Vec<u8>,
        limit: usize,
    }
    impl Write for BoundedBytes {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
                return Err(std::io::Error::other(
                    "Saved data exceeds its size limit; existing file left intact",
                ));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = BoundedBytes {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer_pretty(&mut writer, data).map_err(|e| e.to_string())?;
    let bytes = writer.bytes;
    write(path, &bytes).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn root() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("waifu-durable-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        path
    }
    #[test]
    fn relative_filenames_commit_without_changing_the_callers_cwd() {
        const CHILD: &str = "WAIFU_DURABLE_RELATIVE_TEST_CHILD";
        if std::env::var_os(CHILD).is_some() {
            write(Path::new("profile.json"), b"relative").unwrap();
            write_json(Path::new("settings.json"), &"settings", 32).unwrap();
            return;
        }
        let root = root();
        let cwd = std::env::current_dir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "durable_file::tests::relative_filenames_commit_without_changing_the_callers_cwd",
            ])
            .current_dir(&root)
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            std::fs::read(root.join("profile.json")).unwrap(),
            b"relative"
        );
        assert_eq!(std::env::current_dir().unwrap(), cwd);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn replacement_is_complete_and_leaves_no_temporary_files() {
        let root = root();
        let path = root.join("profile.json");
        write(&path, b"old").unwrap();
        write(&path, b"new complete contents").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new complete contents");
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn serialization_and_replacement_failures_preserve_existing_data_and_clean_up() {
        let root = root();
        let path = root.join("profile.json");
        write_json(&path, &"original", 32).unwrap();
        let original = std::fs::read(&path).unwrap();
        assert!(write_json(&path, &"x".repeat(100), 32).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let directory = root.join("occupied");
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("keep"), b"keep").unwrap();
        assert!(write(&directory, b"replacement").is_err());
        assert_eq!(std::fs::read(directory.join("keep")).unwrap(), b"keep");
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
        assert!(write(&root.join("missing/child"), b"data").is_err());
        assert!(!root.join("missing").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
