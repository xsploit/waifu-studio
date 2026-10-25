//! Win32 desktop geometry. The app supplies its primary HWND; no window titles
//! or synchronous window messages are read from Bevy worker threads.
use super::*;
use std::{
    mem::size_of,
    ptr::null_mut,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, POINT, RECT},
    Graphics::{
        Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute},
        Gdi::{ClientToScreen, EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO},
    },
    UI::{HiDpi::GetDpiForWindow, WindowsAndMessaging::*},
};

pub struct WindowsDesktopBridge {
    cached: Mutex<Option<DesktopSnapshot>>,
    own: AtomicUsize,
}

struct Sample {
    own_handle: HWND,
    own: Option<(HWND, RECT, usize)>,
    windows: Vec<(HWND, RECT, usize)>,
    panels: Vec<(RECT, usize)>,
    monitors: Vec<(u64, RECT)>,
    order: usize,
}

fn native_rect(window: HWND) -> Option<RECT> {
    let mut rect = RECT::default();
    // SAFETY: Win32 validates this borrowed HWND; rect is writable for its size.
    unsafe {
        if DwmGetWindowAttribute(
            window,
            DWMWA_EXTENDED_FRAME_BOUNDS as u32,
            (&mut rect as *mut RECT).cast(),
            size_of::<RECT>() as u32,
        ) < 0
            && GetWindowRect(window, &mut rect) == 0
        {
            return None;
        }
    }
    (rect.right > rect.left && rect.bottom > rect.top).then_some(rect)
}

unsafe extern "system" fn window_record(window: HWND, data: LPARAM) -> i32 {
    // SAFETY: EnumWindows synchronously borrows the Sample supplied by sample().
    let sample = unsafe { &mut *(data as *mut Sample) };
    let order = sample.order;
    sample.order += 1;
    if order >= MAX_RECORDS {
        return 0;
    }
    unsafe {
        if IsWindowVisible(window) == 0 || IsIconic(window) != 0 {
            return 1;
        }
        let mut cloaked = 0u32;
        if DwmGetWindowAttribute(
            window,
            DWMWA_CLOAKED as u32,
            (&mut cloaked as *mut u32).cast(),
            size_of::<u32>() as u32,
        ) >= 0
            && cloaked != 0
        {
            return 1;
        }
        let mut pid = 0;
        GetWindowThreadProcessId(window, &mut pid);
        if pid == std::process::id() {
            if window == sample.own_handle {
                let mut rect = RECT::default();
                let mut origin = POINT::default();
                if GetClientRect(window, &mut rect) != 0 && ClientToScreen(window, &mut origin) != 0
                {
                    rect.right += origin.x;
                    rect.bottom += origin.y;
                    rect.left = origin.x;
                    rect.top = origin.y;
                    sample.own = Some((window, rect, order));
                }
            }
            return 1;
        }
        let Some(rect) = native_rect(window) else {
            return 1;
        };
        let mut class = [0u16; 128];
        let len = GetClassNameW(window, class.as_mut_ptr(), class.len() as i32);
        match String::from_utf16_lossy(&class[..len.max(0) as usize]).as_str() {
            "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" => sample.panels.push((rect, order)),
            "Progman" | "WorkerW" => (),
            _ if GetWindowLongW(window, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW == 0 => {
                sample.windows.push((window, rect, order))
            }
            _ => (),
        }
    }
    1
}

unsafe extern "system" fn monitor_record(
    monitor: HMONITOR,
    _: HDC,
    _: *mut RECT,
    data: LPARAM,
) -> i32 {
    let sample = unsafe { &mut *(data as *mut Sample) };
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if sample.monitors.len() < 64 && unsafe { GetMonitorInfoW(monitor, &mut info) } != 0 {
        sample
            .monitors
            .push((monitor as usize as u64, info.rcMonitor));
    }
    1
}

fn logical(rect: RECT, scale: f32) -> ScreenRect {
    ScreenRect {
        origin: Vec2::new(rect.left as f32, rect.top as f32) / scale,
        size: Vec2::new(
            (rect.right - rect.left) as f32,
            (rect.bottom - rect.top) as f32,
        ) / scale,
    }
}

impl WindowsDesktopBridge {
    pub fn start(native_window: usize) -> Result<Self, String> {
        let bridge = Self {
            cached: Mutex::new(None),
            own: AtomicUsize::new(native_window),
        };
        bridge.own_window()?;
        Ok(bridge)
    }

    fn sample(&self) -> Sample {
        let mut sample = Sample {
            own_handle: self.own.load(Ordering::Relaxed) as HWND,
            own: None,
            windows: Vec::new(),
            panels: Vec::new(),
            monitors: Vec::new(),
            order: 0,
        };
        unsafe {
            EnumWindows(Some(window_record), &mut sample as *mut _ as LPARAM);
            EnumDisplayMonitors(
                null_mut(),
                std::ptr::null(),
                Some(monitor_record),
                &mut sample as *mut _ as LPARAM,
            );
        }
        sample
    }

    fn own_window(&self) -> Result<HWND, String> {
        let window = self.own.load(Ordering::Relaxed) as HWND;
        let mut pid = 0;
        unsafe {
            GetWindowThreadProcessId(window, &mut pid);
        };
        if window.is_null() || pid != std::process::id() {
            return Err("Companion window unavailable".into());
        }
        Ok(window)
    }

    pub fn snapshot(&self) -> Result<DesktopSnapshot, String> {
        let mut cached = self
            .cached
            .lock()
            .map_err(|_| "Windows geometry unavailable")?;
        if let Some(snapshot) = &*cached
            && snapshot.received_at.elapsed() < Duration::from_millis(33)
        {
            return Ok(snapshot.clone());
        }
        let sample = self.sample();
        let (window, rect, order) = sample.own.ok_or("Waiting for the companion window")?;
        let scale = unsafe { GetDpiForWindow(window) }.max(96) as f32 / 96.;
        let thread = unsafe { GetWindowThreadProcessId(window, std::ptr::null_mut()) };
        let mut gui = GUITHREADINFO {
            cbSize: size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        let native_move = (unsafe { GetGUIThreadInfo(thread, &mut gui) } != 0)
            .then_some(gui.flags & GUI_INMOVESIZE != 0 && gui.hwndMoveSize == window);
        let mut cursor = POINT::default();
        if unsafe { GetCursorPos(&mut cursor) } == 0 {
            return Err("Cannot read desktop cursor position".into());
        }
        let mut surfaces: Vec<_> = sample
            .windows
            .iter()
            .map(|(handle, rect, order)| DesktopSurface {
                id: SurfaceId::Window(*handle as usize as u64),
                rect: logical(*rect, scale),
                z_order: *order as i32,
            })
            .collect();
        for (id, monitor) in &sample.monitors {
            surfaces.push(DesktopSurface {
                id: SurfaceId::Monitor(*id),
                rect: logical(*monitor, scale),
                z_order: i32::MAX,
            });
            // Only a visible bottom taskbar creates a seat. Work area insets on
            // the sides/top and an auto-hidden bar never invent a bottom seat.
            for (panel, _) in &sample.panels {
                if panel.left == monitor.left
                    && panel.right == monitor.right
                    && panel.bottom == monitor.bottom
                    && panel.top > monitor.top
                    && panel.bottom - panel.top > 4
                {
                    let work = RECT {
                        bottom: panel.top,
                        ..*monitor
                    };
                    surfaces.push(DesktopSurface {
                        id: SurfaceId::WorkArea(*id),
                        rect: logical(work, scale),
                        z_order: i32::MAX,
                    });
                }
            }
        }
        let snapshot = DesktopSnapshot {
            received_at: Instant::now(),
            surfaces,
            own_window_ids: vec![window as usize as u64],
            own_window_rect: Some(logical(rect, scale)),
            own_interactive_move: native_move,
            own_above_panels: Some(
                sample
                    .panels
                    .iter()
                    .all(|(_, panel_order)| order < *panel_order),
            ),
            cursor: Vec2::new(cursor.x as f32, cursor.y as f32) / scale,
        };
        *cached = Some(snapshot.clone());
        Ok(snapshot)
    }

    pub fn request_move(&self, origin: Vec2) -> Result<(), String> {
        if !valid_point(origin) {
            return Err("Invalid companion window origin".into());
        }
        let window = self.own_window()?;
        let mut client = POINT::default();
        if unsafe { ClientToScreen(window, &mut client) } == 0 {
            return Err("Companion window unavailable".into());
        }
        let mut outer = RECT::default();
        let scale = unsafe { GetDpiForWindow(window) }.max(96) as f32 / 96.;
        if unsafe { GetWindowRect(window, &mut outer) } == 0 {
            return Err("Companion window unavailable".into());
        }
        let target = origin * scale;
        let ok = unsafe {
            SetWindowPos(
                window,
                null_mut(),
                target.x.round() as i32 + outer.left - client.x,
                target.y.round() as i32 + outer.top - client.y,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOZORDER | SWP_ASYNCWINDOWPOS,
            )
        };
        if ok == 0 {
            return Err("Could not move companion window".into());
        }
        *self
            .cached
            .lock()
            .map_err(|_| "Windows geometry unavailable")? = None;
        Ok(())
    }

    pub fn request_keep_above(&self, enabled: bool) -> Result<(), String> {
        let window = self.own_window()?;
        let current = unsafe { GetWindowLongW(window, GWL_EXSTYLE) } as u32 & WS_EX_TOPMOST != 0;
        if current == enabled {
            return Ok(());
        }
        if unsafe {
            SetWindowPos(
                window,
                if enabled {
                    HWND_TOPMOST
                } else {
                    HWND_NOTOPMOST
                },
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_ASYNCWINDOWPOS,
            )
        } == 0
        {
            return Err("Could not update companion stacking".into());
        }
        *self
            .cached
            .lock()
            .map_err(|_| "Windows geometry unavailable")? = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn physical_desktop_coordinates_use_one_companion_scale() {
        let rect = logical(
            RECT {
                left: -1920,
                top: 0,
                right: 0,
                bottom: 1080,
            },
            1.5,
        );
        assert_eq!(rect.origin, Vec2::new(-1280., 0.));
        assert_eq!(rect.size, Vec2::new(1280., 720.));
    }
}
