//! Monitor geometry.

use std::ffi::c_void;

use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::core::BOOL;

use crate::geom::PhysicalRect;

/// One display: `rect` in physical pixels in virtual-screen space (negative left of the primary).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Monitor {
    pub rect: PhysicalRect,
    /// `GetDpiForMonitor(MDT_EFFECTIVE_DPI) / 96`: 1.0, 1.5, 2.0…
    pub scale: f32,
    pub primary: bool,
}

pub fn monitors() -> Vec<Monitor> {
    unsafe extern "system" fn collect(
        monitor: HMONITOR,
        _: HDC,
        _: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        // SAFETY: `data` is the `&mut Vec<Monitor>` passed below, alive for the whole enumeration.
        let out = unsafe { &mut *(data.0 as *mut Vec<Monitor>) };
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: `info.cbSize` is set; `monitor` comes from the enumeration.
        if unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
            let (mut dx, mut dy) = (96u32, 96u32);
            // SAFETY: out-parameters are locals.
            let _ = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) };
            let r = info.rcMonitor;
            out.push(Monitor {
                rect: PhysicalRect {
                    x: r.left,
                    y: r.top,
                    w: r.right - r.left,
                    h: r.bottom - r.top,
                },
                scale: dx as f32 / 96.0,
                primary: info.dwFlags & 1 != 0,
            });
        }
        BOOL(1)
    }

    let mut out: Vec<Monitor> = Vec::new();
    // SAFETY: the callback only touches `out`, which outlives the call.
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(collect),
            LPARAM((&raw mut out).cast::<c_void>() as isize),
        );
    }
    out
}

/// The primary display, or the first one found.
pub fn primary() -> Option<Monitor> {
    let all = monitors();
    all.iter()
        .copied()
        .find(|m| m.primary)
        .or(all.first().copied())
}
