//! Per-user file locking with both uptime and wall time. Clock adjustments can
//! delay admission, but cannot grant two reservations inside five seconds.
use super::{Admission, Stamp};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    os::windows::{
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    },
    path::Path,
    time::Duration,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT,
    GetFileInformationByHandle,
};

const INTERVAL: u64 = 5000;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    uptime: u64,
    wall: u64,
}
pub(crate) struct Coordinator {
    file: File,
}
struct Unlock<'a>(&'a File);
impl Drop for Unlock<'_> {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

fn remaining(saved: &Record, now: Stamp) -> u64 {
    // After an uptime rollback (reboot), require five seconds of new uptime.
    let uptime_elapsed = now.millis.checked_sub(saved.uptime).unwrap_or(now.millis);
    let wall_elapsed = now.wall_millis.saturating_sub(saved.wall);
    INTERVAL.saturating_sub(uptime_elapsed.min(wall_elapsed))
}

impl Coordinator {
    pub(crate) fn open_default() -> Result<Self, &'static str> {
        let dirs = directories::ProjectDirs::from("", "", "waifu-studio")
            .ok_or("Cannot locate Discord coordination directory")?;
        Self::open(&dirs.data_local_dir().join("discord-identify"))
    }
    fn open(root: &Path) -> Result<Self, &'static str> {
        fs::create_dir_all(root).map_err(|_| "Cannot create Discord coordination directory")?;
        let meta = fs::symlink_metadata(root)
            .map_err(|_| "Cannot inspect Discord coordination directory")?;
        if !meta.is_dir() || meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err("Discord coordination directory must be an ordinary directory");
        }
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(root.join("identify-windows.lock"))
            .map_err(|_| "Cannot open Discord coordination file")?;
        let meta = file
            .metadata()
            .map_err(|_| "Cannot inspect Discord coordination file")?;
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        if !meta.is_file()
            || meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0
            || info.nNumberOfLinks != 1
        {
            return Err("Discord coordination file must be an ordinary unlinked file");
        }
        Ok(Self { file })
    }
    pub(crate) fn reserve(&mut self, now: Stamp) -> Result<Admission, &'static str> {
        match self.file.try_lock() {
            Ok(()) => (),
            Err(std::fs::TryLockError::WouldBlock) => {
                return Ok(Admission::Wait(Duration::from_millis(100)));
            }
            Err(_) => return Err("Cannot lock Discord coordination file"),
        }
        let _unlock = Unlock(&self.file);
        let mut file = &self.file;
        file.seek(SeekFrom::Start(0))
            .map_err(|_| "Cannot read Discord coordination state")?;
        let mut bytes = Vec::new();
        (&mut file)
            .take(1025)
            .read_to_end(&mut bytes)
            .map_err(|_| "Cannot read Discord coordination state")?;
        if bytes.len() > 1024 {
            return Err("Discord coordination state exceeds its limit");
        }
        if !bytes.is_empty() {
            let saved: Record =
                serde_json::from_slice(&bytes).map_err(|_| "Invalid Discord coordination state")?;
            if saved.version != 1 {
                return Err("Unsupported Discord coordination state");
            }
            let delay = remaining(&saved, now);
            if delay > 0 {
                return Ok(Admission::Wait(Duration::from_millis(delay)));
            }
        }
        if now.millis < INTERVAL {
            return Ok(Admission::Wait(Duration::from_millis(
                INTERVAL - now.millis,
            )));
        }
        let bytes = serde_json::to_vec(&Record {
            version: 1,
            uptime: now.millis,
            wall: now.wall_millis,
        })
        .map_err(|_| "Cannot encode Discord coordination state")?;
        file.seek(SeekFrom::Start(0))
            .and_then(|_| file.set_len(0))
            .and_then(|_| file.write_all(&bytes))
            .and_then(|_| file.sync_all())
            .map_err(|_| "Cannot save Discord coordination reservation")?;
        Ok(Admission::Reserved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn stamp(uptime: u64, wall: u64) -> Stamp {
        Stamp {
            millis: uptime,
            wall_millis: wall,
        }
    }
    #[test]
    fn clock_changes_and_reboot_do_not_bypass_interval() {
        let record = Record {
            version: 1,
            uptime: 10000,
            wall: 100000,
        };
        assert_eq!(remaining(&record, stamp(11000, 999999)), 4000);
        assert_eq!(remaining(&record, stamp(20000, 90000)), 5000);
        assert_eq!(remaining(&record, stamp(1000, 110000)), 4000);
        assert_eq!(remaining(&record, stamp(5000, 110000)), 0);
    }
    #[test]
    fn independent_handles_share_reservations() {
        let root = std::env::temp_dir().join(format!("waifu-identify-{}", uuid::Uuid::new_v4()));
        let mut first = Coordinator::open(&root).unwrap();
        let mut second = Coordinator::open(&root).unwrap();
        assert_eq!(
            first.reserve(stamp(10000, 100000)).unwrap(),
            Admission::Reserved
        );
        assert_eq!(
            second.reserve(stamp(10001, 100001)).unwrap(),
            Admission::Wait(Duration::from_millis(4999))
        );
        drop((first, second));
        fs::remove_dir_all(root).unwrap();
    }
}
