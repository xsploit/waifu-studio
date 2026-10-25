//! Windows child ownership and bounded pipes. Children enter a kill-on-close
//! Job Object before their initial thread is allowed to execute.
use std::{
    fs::File,
    io::{self, Read, Write},
    mem::size_of,
    os::windows::{
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    process::{Child, Command, Stdio},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::*,
    Security::SECURITY_ATTRIBUTES,
    Storage::FileSystem::*,
    System::{Diagnostics::ToolHelp::*, JobObjects::*, Pipes::*, Threading::*},
};

pub(crate) struct Job(OwnedHandle);

/// Windows does not execute shebangs. Keep scripts as literal argv and select
/// only the supported interpreter; never feed extension text to a shell.
pub(crate) fn command(program: &std::path::Path) -> Command {
    match program
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("py") => {
            let mut command = if let Some(python) = std::env::var_os("WAIFU_STUDIO_PYTHON") {
                Command::new(python)
            } else {
                let mut command = Command::new("py.exe");
                command.arg("-3");
                command
            };
            command.arg("-u").arg(program);
            command
        }
        Some("js" | "mjs" | "cjs") => {
            let mut command = Command::new(
                std::env::var_os("WAIFU_STUDIO_NODE").unwrap_or_else(|| "node.exe".into()),
            );
            command.arg(program);
            command
        }
        _ => Command::new(program),
    }
}
impl Job {
    pub fn spawn(command: &mut Command) -> io::Result<(Child, Self)> {
        let handle = unsafe { CreateJobObjectW(null(), null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Self(unsafe { OwnedHandle::from_raw_handle(handle) });
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut child = command
            .creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW)
            .spawn()?;
        let result = (|| {
            if unsafe { AssignProcessToJobObject(handle, child.as_raw_handle()) } == 0 {
                return Err(io::Error::last_os_error());
            }
            let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
            if snapshot == INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error());
            }
            let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };
            let mut entry = THREADENTRY32 {
                dwSize: size_of::<THREADENTRY32>() as u32,
                ..Default::default()
            };
            let mut present = unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) } != 0;
            while present {
                if entry.th32OwnerProcessID == child.id() {
                    let thread =
                        unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
                    if thread.is_null() {
                        return Err(io::Error::last_os_error());
                    }
                    let thread = unsafe { OwnedHandle::from_raw_handle(thread) };
                    if unsafe { ResumeThread(thread.as_raw_handle()) } == u32::MAX {
                        return Err(io::Error::last_os_error());
                    }
                    return Ok(());
                }
                present = unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) } != 0;
            }
            Err(io::Error::other("Cannot find suspended child thread"))
        })();
        if let Err(error) = result {
            job.kill();
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        Ok((child, job))
    }
    pub fn kill(&self) {
        unsafe {
            TerminateJobObject(self.0.as_raw_handle(), 1);
        }
    }
}

/// Peek before synchronous reads, so an idle or inherited pipe cannot block
/// cancellation. Each pipe has exactly one reader.
pub(crate) struct Reader<T>(pub T);
impl<T: Read + AsRawHandle> Read for Reader<T> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let mut available = 0;
        if unsafe {
            PeekNamedPipe(
                self.0.as_raw_handle(),
                null_mut(),
                0,
                null_mut(),
                &mut available,
                null_mut(),
            )
        } == 0
        {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) {
                Ok(0)
            } else {
                Err(error)
            };
        }
        if available == 0 {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let count = buffer.len().min(available as usize);
        self.0.read(&mut buffer[..count])
    }
}

pub(crate) struct Writer(File);
impl Write for Writer {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        match self.0.write(buffer) {
            Ok(0) if !buffer.is_empty() => Err(io::ErrorKind::WouldBlock.into()),
            result => result,
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A byte-mode, nonblocking server writes to the child's ordinary blocking stdin.
pub(crate) fn stdin_pipe() -> io::Result<(Writer, Stdio)> {
    let name: Vec<u16> = format!(
        r"\\.\pipe\waifu-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    )
    .encode_utf16()
    .chain(Some(0))
    .collect();
    let server = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_OUTBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_NOWAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            65536,
            65536,
            0,
            null(),
        )
    };
    if server == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let server = unsafe { File::from_raw_handle(server) };
    let security = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    };
    let client = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_READ,
            0,
            &security,
            OPEN_EXISTING,
            0,
            null_mut(),
        )
    };
    if client == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let client = unsafe { OwnedHandle::from_raw_handle(client) };
    Ok((Writer(server), Stdio::from(client)))
}
