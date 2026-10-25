//! Vulkan/X11 surfaces on the tested Linux backend provide opaque alpha only.
//! Keep Studio usable and explain the Desktop limitation before GPU configure.
use crate::studio::{Studio, ViewMode};
use bevy::{
    prelude::*,
    render::renderer::RenderAdapterInfo,
    window::{CompositeAlphaMode, PrimaryWindow, RawHandleWrapper},
};

pub struct WindowAlphaPlugin;
impl Plugin for WindowAlphaPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Last, guard_x11_vulkan_alpha);
    }
}
fn needs_x11_guard(handle: wgpu::rwh::RawWindowHandle, backend: wgpu::Backend) -> bool {
    cfg!(target_os = "linux")
        && backend == wgpu::Backend::Vulkan
        && matches!(
            handle,
            wgpu::rwh::RawWindowHandle::Xlib(_) | wgpu::rwh::RawWindowHandle::Xcb(_)
        )
}
fn guard_x11_vulkan_alpha(
    adapter: Option<Res<RenderAdapterInfo>>,
    mut windows: Query<(&mut Window, &RawHandleWrapper), With<PrimaryWindow>>,
    mut studio: ResMut<Studio>,
    mut explained: Local<bool>,
) {
    let Some(adapter) = adapter else {
        return;
    };
    let Ok((mut window, handle)) = windows.single_mut() else {
        return;
    };
    if !needs_x11_guard(handle.get_window_handle(), adapter.backend) {
        return;
    }
    // Auto lets wgpu choose a supported mode, avoiding the unconditional
    // PreMultiplied request that crashed on the tested opaque-only surface.
    // No extra GPU surface is created and no off-thread native handles are used.
    if window.composite_alpha_mode != CompositeAlphaMode::Auto {
        window.composite_alpha_mode = CompositeAlphaMode::Auto;
        warn!(
            "Using automatic surface alpha on Vulkan/X11; transparent Desktop mode requires native Wayland in this build"
        );
    }
    if studio.mode == ViewMode::Desktop || !*explained {
        studio.mode = ViewMode::Studio;
        studio.notice = Some("Transparent Desktop mode is unavailable with Vulkan/X11 in this build. Studio remains available. On Linux, use native Wayland for Desktop mode.".into());
        *explained = true;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr::NonNull;
    #[test]
    fn guard_is_specific_to_linux_vulkan_x11_and_keeps_wayland_unchanged() {
        use wgpu::rwh::{RawWindowHandle, WaylandWindowHandle, XcbWindowHandle, XlibWindowHandle};
        let xlib = RawWindowHandle::Xlib(XlibWindowHandle::new(1));
        let xcb = RawWindowHandle::Xcb(XcbWindowHandle::new(std::num::NonZeroU32::new(1).unwrap()));
        // Opaque test pointer is never dereferenced or passed to a native API.
        let wayland = RawWindowHandle::Wayland(WaylandWindowHandle::new(NonNull::dangling()));
        assert_eq!(
            needs_x11_guard(xlib, wgpu::Backend::Vulkan),
            cfg!(target_os = "linux")
        );
        assert_eq!(
            needs_x11_guard(xcb, wgpu::Backend::Vulkan),
            cfg!(target_os = "linux")
        );
        assert!(!needs_x11_guard(wayland, wgpu::Backend::Vulkan));
        assert!(!needs_x11_guard(xlib, wgpu::Backend::Gl));
    }
}
