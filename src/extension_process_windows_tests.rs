use super::*;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use windows_sys::Win32::{
    Foundation::WAIT_OBJECT_0,
    System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
};

struct Fixture {
    root: PathBuf,
    handle: ProcessHandle,
}
impl Fixture {
    fn new(script: &str, lifetime: Option<Duration>) -> Self {
        let root = std::env::temp_dir().join(format!(
            "waifu windows space \u{65e5} {}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let program = root.join("fixture.py");
        std::fs::write(&program, script).unwrap();
        let handle = ProcessHandle::spawn(ProcessSpec {
            program,
            args: Vec::new(),
            working_directory: root.clone(),
            data_directory: root.join("data"),
            lifetime,
        })
        .unwrap();
        Self { root, handle }
    }
    fn line(&self) -> Vec<u8> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            while let Some(event) = self.handle.poll() {
                if let ProcessEvent::Line(line) = event {
                    return line;
                }
            }
            assert!(
                Instant::now() < deadline,
                "No protocol line; terminal={:?}",
                self.handle.termination()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn wait(&mut self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while !self.handle.finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        self.handle.reap_finished()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.handle.stop();
        assert!(self.handle.shutdown(Duration::from_secs(3)));
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn windows_script_echo_preserves_unicode_paths_and_literal_input() {
    let fixture = Fixture::new(
        "import sys\nfor line in sys.stdin:\n print(line.rstrip('\\n'), flush=True)\n",
        None,
    );
    fixture.handle.send_line(b"literal & | % ! spaces").unwrap();
    assert_eq!(fixture.line(), b"literal & | % ! spaces");
}

#[test]
fn windows_stop_terminates_descendants_before_reporting_completion() {
    let fixture = Fixture::new(
        "import subprocess,sys,time\np=subprocess.Popen([sys.executable,'-c','import time; time.sleep(60)'])\nprint(p.pid,flush=True)\ntime.sleep(60)\n",
        None,
    );
    let pid: u32 = String::from_utf8(fixture.line()).unwrap().parse().unwrap();
    let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    assert!(!raw.is_null());
    let child = unsafe { OwnedHandle::from_raw_handle(raw) };
    fixture.handle.stop();
    let mut fixture = fixture;
    assert!(fixture.wait(Duration::from_secs(3)));
    assert_eq!(
        unsafe { WaitForSingleObject(child.as_raw_handle(), 2000) },
        WAIT_OBJECT_0
    );
}

#[test]
fn windows_full_stdin_does_not_block_timeout() {
    let mut fixture = Fixture::new(
        "import time\nprint('ready',flush=True)\ntime.sleep(60)\n",
        Some(Duration::from_secs(2)),
    );
    assert_eq!(fixture.line(), b"ready");
    for _ in 0..32 {
        let _ = fixture.handle.send_line(&vec![b'x'; MAX_LINE - 1]);
    }
    assert!(fixture.wait(Duration::from_secs(5)));
    assert_eq!(
        fixture.handle.termination().unwrap().reason,
        "Hook timed out"
    );
}
