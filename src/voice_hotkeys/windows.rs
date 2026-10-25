//! Opt-in Win32 shortcuts, owned and released by a dedicated message thread.
use super::{Action, PortalEvent};
use std::{
    ptr::null_mut,
    sync::mpsc::{self, SyncSender},
    thread,
    time::Duration,
};
use windows_sys::Win32::UI::{
    Input::KeyboardAndMouse::{
        MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey, UnregisterHotKey, VK_OEM_COMMA,
        VK_OEM_PERIOD, VK_SPACE,
    },
    WindowsAndMessaging::{MSG, PM_REMOVE, PeekMessageW, WM_HOTKEY},
};

pub(super) struct PortalWorker {
    cancel: SyncSender<()>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Drop for PortalWorker {
    fn drop(&mut self) {
        let _ = self.cancel.try_send(());
        // The worker has no blocking I/O; joining releases registrations before
        // a settings toggle can immediately start a replacement worker.
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Registrations(Vec<i32>);
impl Drop for Registrations {
    fn drop(&mut self) {
        for id in &self.0 {
            unsafe {
                UnregisterHotKey(null_mut(), *id);
            }
        }
    }
}

pub(super) fn start(
    generation: u64,
    tx: SyncSender<(u64, PortalEvent)>,
) -> Result<PortalWorker, String> {
    let (cancel, rx) = mpsc::sync_channel(1);
    let worker = thread::Builder::new()
        .name("waifu-windows-hotkeys".into())
        .spawn(move || {
            let mut registered = Registrations(Vec::new());
            let keys = [VK_SPACE, b'M' as u16, VK_OEM_PERIOD, VK_OEM_COMMA];
            for (index, key) in keys.into_iter().enumerate() {
                let id = index as i32 + 1;
                if unsafe {
                    RegisterHotKey(
                        null_mut(),
                        id,
                        MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT,
                        u32::from(key),
                    )
                } == 0
                {
                    let _ = tx.try_send((
                        generation,
                        PortalEvent::Closed(format!(
                            "Could not register {}. Another application may own it.",
                            Action::ALL[index].shortcut()
                        )),
                    ));
                    return;
                }
                registered.0.push(id);
            }
            if tx
                .try_send((
                    generation,
                    PortalEvent::Ready(
                        Action::ALL
                            .into_iter()
                            .map(|action| (action, action.shortcut().into()))
                            .collect(),
                    ),
                ))
                .is_err()
            {
                return;
            }
            loop {
                match rx.try_recv() {
                    Ok(()) | Err(mpsc::TryRecvError::Disconnected) => break,
                    Err(mpsc::TryRecvError::Empty) => (),
                }
                let mut message = MSG::default();
                for _ in 0..32 {
                    if unsafe {
                        PeekMessageW(&mut message, null_mut(), WM_HOTKEY, WM_HOTKEY, PM_REMOVE)
                    } == 0
                    {
                        break;
                    }
                    if let Some(action) = message
                        .wParam
                        .checked_sub(1)
                        .and_then(|index| Action::ALL.get(index))
                    {
                        let _ = tx.try_send((generation, PortalEvent::Trigger(*action)));
                    }
                }
                thread::sleep(Duration::from_millis(10));
            }
        })
        .map_err(|error| format!("Cannot start Windows shortcuts: {error}"))?;
    Ok(PortalWorker {
        cancel,
        worker: Some(worker),
    })
}
