//! Per-OS-user coordination for this app's Discord workers. No token/account IDs.
//! Reservations persist before network writes; uncertain writes consume the slot.
use std::time::Duration;
#[cfg(not(windows))]
use uuid::Uuid;

/// Local admission only. Used by non-Rust supervised workers before each
/// Identify; it never authenticates or opens a service connection.
pub fn admission_for_worker() -> Result<serde_json::Value, &'static str> {
    let mut coordinator = Coordinator::open_default()?;
    match coordinator.reserve(Stamp::now()?)? {
        Admission::Reserved => Ok(serde_json::json!({"version":1,"status":"reserved"})),
        Admission::Wait(wait) => {
            let millis = wait.as_millis();
            if !(1..=5000).contains(&millis) {
                return Err("Invalid Discord admission wait");
            }
            Ok(serde_json::json!({"version":1,"status":"wait","wait_ms":millis as u64}))
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Admission {
    Reserved,
    Wait(Duration),
}
#[derive(Clone, Copy)]
pub(crate) struct Stamp {
    #[cfg(not(windows))]
    boot: Uuid,
    millis: u64,
    #[cfg(windows)]
    wall_millis: u64,
}
impl Stamp {
    pub(crate) fn now() -> Result<Self, &'static str> {
        #[cfg(target_os = "linux")]
        {
            use std::sync::OnceLock;
            static BOOT: OnceLock<Result<Uuid, &'static str>> = OnceLock::new();
            let boot = *BOOT
                .get_or_init(|| {
                    let value = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
                        .map_err(|_| "Cannot read Discord coordination boot identity")?;
                    Uuid::parse_str(value.trim())
                        .map_err(|_| "Invalid Discord coordination boot identity")
                })
                .as_ref()
                .map_err(|error| *error)?;
            let mut time = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            // SAFETY: valid writable timespec; CLOCK_BOOTTIME includes suspend
            // and cannot move backward with wall-clock/NTP changes.
            if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut time) } != 0
                || time.tv_sec < 0
                || !(0..1_000_000_000).contains(&time.tv_nsec)
            {
                return Err("Cannot read Discord coordination clock");
            }
            let millis = (time.tv_sec as u64)
                .checked_mul(1000)
                .and_then(|ms| ms.checked_add(time.tv_nsec as u64 / 1_000_000))
                .ok_or("Discord coordination clock overflow")?;
            Ok(Self { boot, millis })
        }
        #[cfg(windows)]
        {
            let wall_millis = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| "Cannot read Discord coordination clock")?
                .as_millis();
            Ok(Self {
                millis: unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() },
                wall_millis: u64::try_from(wall_millis)
                    .map_err(|_| "Discord coordination clock overflow")?,
            })
        }
        #[cfg(not(any(target_os = "linux", windows)))]
        Err("Shared Discord Identify coordination currently requires Linux")
    }
}
#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use serde::{Deserialize, Serialize};
    use std::{
        fs::{self, File},
        io::{Read, Seek, SeekFrom, Write},
        os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        path::Path,
    };
    const INTERVAL_MS: u64 = 5000;
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Record {
        version: u32,
        boot: Uuid,
        next_ms: u64,
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
    impl Coordinator {
        pub(crate) fn open_default() -> Result<Self, &'static str> {
            let dirs = directories::ProjectDirs::from("", "", "waifu-studio")
                .ok_or("Cannot locate Discord coordination directory")?;
            Self::open(&dirs.data_local_dir().join("discord-identify"))
        }
        pub(super) fn open(root: &Path) -> Result<Self, &'static str> {
            // Only this dedicated subdirectory is managed; never chmod a profile root.
            match fs::symlink_metadata(root) {
                Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                    return Err("Discord coordination directory is not a real directory");
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    fs::create_dir_all(root)
                        .map_err(|_| "Cannot create Discord coordination directory")?
                }
                Err(_) => return Err("Cannot inspect Discord coordination directory"),
            }
            // SAFETY: geteuid has no arguments and returns this process's owner.
            let owner = unsafe { libc::geteuid() };
            if fs::metadata(root)
                .map_err(|_| "Cannot inspect Discord coordination directory")?
                .uid()
                != owner
            {
                return Err("Discord coordination directory has another owner");
            }
            fs::set_permissions(root, fs::Permissions::from_mode(0o700))
                .map_err(|_| "Cannot protect Discord coordination directory")?;
            let file = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(root.join("identify.lock"))
                .map_err(|_| "Cannot open Discord coordination file")?;
            let meta = file
                .metadata()
                .map_err(|_| "Cannot inspect Discord coordination file")?;
            if !meta.is_file()
                || meta.uid() != owner
                || meta.mode() & 0o077 != 0
                || meta.nlink() != 1
            {
                return Err(
                    "Discord coordination file must be private, owned and unlinked elsewhere",
                );
            }
            Ok(Self { file })
        }
        pub(crate) fn reserve(&mut self, now: Stamp) -> Result<Admission, &'static str> {
            match self.file.try_lock() {
                Ok(()) => {}
                Err(std::fs::TryLockError::WouldBlock) => {
                    return Ok(Admission::Wait(Duration::from_millis(100)));
                }
                Err(_) => return Err("Cannot lock Discord coordination file"),
            }
            let _unlock = Unlock(&self.file);
            // A separate reference permits cursor I/O while Unlock owns &File.
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
                let saved: Record = serde_json::from_slice(&bytes)
                    .map_err(|_| "Invalid Discord coordination state")?;
                if saved.version != 1 || saved.boot.is_nil() || saved.next_ms < INTERVAL_MS {
                    return Err("Unsupported Discord coordination state");
                }
                if saved.boot == now.boot && saved.next_ms > now.millis {
                    let remaining = saved.next_ms - now.millis;
                    if remaining > INTERVAL_MS {
                        return Err("Discord coordination clock state is inconsistent");
                    }
                    return Ok(Admission::Wait(Duration::from_millis(remaining)));
                }
                // After reboot, wait for five seconds of new-boot uptime so
                // even a very fast reboot cannot bypass the previous reservation.
                if saved.boot != now.boot && now.millis < INTERVAL_MS {
                    return Ok(Admission::Wait(Duration::from_millis(
                        INTERVAL_MS - now.millis,
                    )));
                }
            }
            if now.millis < INTERVAL_MS {
                return Ok(Admission::Wait(Duration::from_millis(
                    INTERVAL_MS - now.millis,
                )));
            }
            let next_ms = now
                .millis
                .checked_add(INTERVAL_MS)
                .ok_or("Discord coordination clock overflow")?;
            let bytes = serde_json::to_vec(&Record {
                version: 1,
                boot: now.boot,
                next_ms,
            })
            .map_err(|_| "Cannot encode Discord coordination state")?;
            file.seek(SeekFrom::Start(0))
                .map_err(|_| "Cannot write Discord coordination state")?;
            file.set_len(0)
                .map_err(|_| "Cannot write Discord coordination state")?;
            file.write_all(&bytes)
                .and_then(|()| file.sync_all())
                .map_err(|_| "Cannot save Discord coordination reservation")?;
            Ok(Admission::Reserved)
        }
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        use std::path::PathBuf;
        struct Temp(PathBuf);
        impl Temp {
            fn new() -> Self {
                Self(std::env::temp_dir().join(format!("discord-identify-{}", Uuid::new_v4())))
            }
        }
        impl Drop for Temp {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        fn stamp(ms: u64) -> Stamp {
            Stamp {
                boot: Uuid::from_u128(1),
                millis: ms,
            }
        }
        #[test]
        fn native_boot_clock_is_available_and_monotonic() {
            let first = Stamp::now().unwrap();
            let second = Stamp::now().unwrap();
            assert!(!first.boot.is_nil());
            assert_eq!(first.boot, second.boot);
            assert!(second.millis >= first.millis);
        }
        #[test]
        fn independent_process_observes_previous_workers_reservation() {
            const ROOT: &str = "WAIFU_DISCORD_IDENTIFY_CHILD_FIXTURE";
            if let Some(root) = std::env::var_os(ROOT) {
                let mut gate = Coordinator::open(std::path::Path::new(&root)).unwrap();
                assert_eq!(
                    gate.reserve(stamp(10000)).unwrap(),
                    Admission::Wait(Duration::from_secs(5))
                );
                return;
            }
            let dir = Temp::new();
            let mut gate = Coordinator::open(&dir.0).unwrap();
            assert_eq!(gate.reserve(stamp(10000)).unwrap(), Admission::Reserved);
            drop(gate);
            let name = format!(
                "{}::independent_process_observes_previous_workers_reservation",
                module_path!().split_once("::").unwrap().1
            );
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", &name, "--test-threads=1", "--nocapture"])
                .env(ROOT, &dir.0)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            let until = std::time::Instant::now() + Duration::from_secs(10);
            let mut timed_out = false;
            while child.try_wait().unwrap().is_none() {
                if std::time::Instant::now() >= until {
                    timed_out = true;
                    let _ = child.kill();
                    break;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            let result = child.wait_with_output().unwrap();
            let stdout = String::from_utf8_lossy(&result.stdout);
            assert!(
                !timed_out
                    && result.status.success()
                    && stdout.contains(&name)
                    && stdout.contains("test result: ok. 1 passed;"),
                "child quota check failed: {stdout} {}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        #[test]
        fn first_five_seconds_of_boot_are_conservative_even_without_saved_state() {
            let dir = Temp::new();
            let mut gate = Coordinator::open(&dir.0).unwrap();
            assert_eq!(
                gate.reserve(stamp(1)).unwrap(),
                Admission::Wait(Duration::from_millis(4999))
            );
            assert_eq!(gate.reserve(stamp(5000)).unwrap(), Admission::Reserved);
        }

        #[test]
        fn workers_and_restarts_share_persisted_five_second_reservation() {
            let dir = Temp::new();
            let mut first = Coordinator::open(&dir.0).unwrap();
            let mut second = Coordinator::open(&dir.0).unwrap();
            assert_eq!(first.reserve(stamp(10000)).unwrap(), Admission::Reserved);
            assert_eq!(
                second.reserve(stamp(10000)).unwrap(),
                Admission::Wait(Duration::from_secs(5))
            );
            drop(first);
            let mut restarted = Coordinator::open(&dir.0).unwrap();
            assert_eq!(
                restarted.reserve(stamp(14999)).unwrap(),
                Admission::Wait(Duration::from_millis(1))
            );
            assert_eq!(second.reserve(stamp(15000)).unwrap(), Admission::Reserved);
            assert_eq!(
                restarted.reserve(stamp(15001)).unwrap(),
                Admission::Wait(Duration::from_millis(4999))
            );
            let data = fs::read_to_string(dir.0.join("identify.lock")).unwrap();
            assert!(!data.contains("token"));
        }
        #[test]
        fn lock_contention_does_not_block_and_early_errors_unlock() {
            let dir = Temp::new();
            let first = Coordinator::open(&dir.0).unwrap();
            let mut second = Coordinator::open(&dir.0).unwrap();
            first.file.lock().unwrap();
            assert_eq!(
                second.reserve(stamp(0)).unwrap(),
                Admission::Wait(Duration::from_millis(100))
            );
            first.file.unlock().unwrap();
            fs::write(dir.0.join("identify.lock"), b"broken").unwrap();
            assert!(second.reserve(stamp(0)).is_err());
            assert!(first.file.try_lock().is_ok());
            first.file.unlock().unwrap();
        }
        #[test]
        fn reboot_and_clock_inconsistency_cannot_bypass_reservation() {
            let dir = Temp::new();
            let mut gate = Coordinator::open(&dir.0).unwrap();
            gate.reserve(stamp(10000)).unwrap();
            assert!(gate.reserve(stamp(9999)).is_err());
            let reboot = |millis| Stamp {
                boot: Uuid::from_u128(2),
                millis,
            };
            assert_eq!(
                gate.reserve(reboot(1000)).unwrap(),
                Admission::Wait(Duration::from_secs(4))
            );
            assert_eq!(gate.reserve(reboot(5000)).unwrap(), Admission::Reserved);
        }
        #[test]
        fn simultaneous_workers_get_only_one_reservation() {
            let dir = Temp::new();
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    let root = dir.0.clone();
                    let barrier = barrier.clone();
                    std::thread::spawn(move || {
                        let mut gate = Coordinator::open(&root).unwrap();
                        barrier.wait();
                        gate.reserve(stamp(20000)).unwrap()
                    })
                })
                .collect();
            let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
            assert_eq!(
                results
                    .iter()
                    .filter(|a| **a == Admission::Reserved)
                    .count(),
                1
            );
        }
        #[test]
        fn rejects_symlinks_nonprivate_and_oversized_state() {
            use std::os::unix::fs::symlink;
            let dir = Temp::new();
            fs::create_dir_all(&dir.0).unwrap();
            let outside = dir.0.join("outside");
            fs::write(&outside, b"untouched").unwrap();
            symlink(&outside, dir.0.join("identify.lock")).unwrap();
            assert!(Coordinator::open(&dir.0).is_err());
            assert_eq!(fs::read(&outside).unwrap(), b"untouched");
            fs::remove_file(dir.0.join("identify.lock")).unwrap();
            let mut gate = Coordinator::open(&dir.0).unwrap();
            fs::write(dir.0.join("identify.lock"), vec![b' '; 1025]).unwrap();
            assert!(gate.reserve(stamp(0)).is_err());
            fs::set_permissions(
                dir.0.join("identify.lock"),
                fs::Permissions::from_mode(0o644),
            )
            .unwrap();
            assert!(Coordinator::open(&dir.0).is_err());
        }
    }
}
#[cfg(target_os = "linux")]
pub(crate) use platform::Coordinator;
#[cfg(windows)]
#[path = "discord_identify_windows.rs"]
mod windows;
#[cfg(windows)]
pub(crate) use windows::Coordinator;
#[cfg(not(any(target_os = "linux", windows)))]
pub(crate) struct Coordinator;
#[cfg(not(any(target_os = "linux", windows)))]
impl Coordinator {
    pub(crate) fn open_default() -> Result<Self, &'static str> {
        Err("Shared Discord Identify coordination currently requires Linux")
    }
    pub(crate) fn reserve(&mut self, _: Stamp) -> Result<Admission, &'static str> {
        Err("Shared Discord Identify coordination currently requires Linux")
    }
}
