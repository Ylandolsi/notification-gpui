//! Window flags for the overlay and the panel (design note §5), after Scrawl's `src/win/overlay.rs`.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::geom::PhysicalRect;
use windows::Win32::Foundation::{COLORREF, E_FAIL, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMNCRP_DISABLED, DWMWA_BORDER_COLOR, DWMWA_NCRENDERING_POLICY, DWMWA_WINDOW_CORNER_PREFERENCE,
    DWMWCP_DONOTROUND, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, CreateRectRgn, DeleteObject, HGDIOBJ, SetWindowRgn,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentProcessId, GetCurrentThreadId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GA_ROOT, GWL_EXSTYLE, GetAncestor, GetClientRect, GetForegroundWindow, GetSystemMetrics,
    GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId, HWND_TOPMOST, IsWindow,
    IsWindowVisible, LWA_ALPHA, PostMessageW, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SWP_NOZORDER, SetForegroundWindow, SetLayeredWindowAttributes,
    SetWindowDisplayAffinity, SetWindowLongPtrW, SetWindowPos, ShowWindow, WDA_EXCLUDEFROMCAPTURE,
    WDA_NONE, WINDOW_EX_STYLE, WM_CLOSE, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WindowFromPoint,
};
use windows::core::{PCSTR, Result, w};

/// Whether our own windows are hidden from screenshots and screen recordings (`WDA_EXCLUDEFROMCAPTURE`).
/// Off by default, so a demo recording shows the island and the glow. The R8 proof turns it on, and a later
/// settings switch can: hiding keeps the overlay out of an agent's screenshot, OCR or detector input.
static HIDE_FROM_CAPTURE: AtomicBool = AtomicBool::new(false);

pub fn hide_from_capture() -> bool {
    HIDE_FROM_CAPTURE.load(Ordering::Relaxed)
}

/// Sets the policy for windows styled from now on. Callers re-apply it to open windows with
/// `apply_capture_policy`.
pub fn set_hide_from_capture(hide: bool) {
    HIDE_FROM_CAPTURE.store(hide, Ordering::Relaxed);
}

/// Applies the current capture policy to a window we own.
pub fn apply_capture_policy(hwnd: Hwnd) -> Result<()> {
    set_capture_excluded(hwnd, hide_from_capture())
}

/// A window handle as the app sees it (`isize` keeps raw-window-handle out of this crate).
pub type Hwnd = isize;

fn handle(hwnd: Hwnd) -> HWND {
    HWND(hwnd as *mut c_void)
}

/// Current extended style, for read-back checks.
pub fn ex_style(hwnd: Hwnd) -> u32 {
    // SAFETY: plain Win32 getter on a handle; an invalid handle returns 0.
    unsafe { GetWindowLongPtrW(handle(hwnd), GWL_EXSTYLE) as u32 }
}

fn add_ex_style(hwnd: Hwnd, extra: WINDOW_EX_STYLE) {
    // SAFETY: get-modify-set of the extended style of a window we own.
    unsafe {
        let style = GetWindowLongPtrW(handle(hwnd), GWL_EXSTYLE);
        SetWindowLongPtrW(handle(hwnd), GWL_EXSTYLE, style | extra.0 as isize);
    }
}

fn remove_ex_style(hwnd: Hwnd, gone: WINDOW_EX_STYLE) {
    // SAFETY: get-modify-set of the extended style of a window we own.
    unsafe {
        let style = GetWindowLongPtrW(handle(hwnd), GWL_EXSTYLE);
        SetWindowLongPtrW(handle(hwnd), GWL_EXSTYLE, style & !(gone.0 as isize));
    }
}

/// Moves and resizes a window we own, in physical pixels, without activating it or changing its z-order.
pub fn set_bounds(hwnd: Hwnd, x: i32, y: i32, w: i32, h: i32) {
    // SAFETY: position and size of a window we own; no activation, no z-order change.
    unsafe {
        let _ = SetWindowPos(
            handle(hwnd),
            None,
            x,
            y,
            w,
            h,
            SWP_NOACTIVATE | SWP_NOZORDER,
        );
    }
}

/// Moves and sizes a window so its client area is exactly `rect` (physical pixels), without activating it.
/// GPUI keeps a frame around the client area, so the window rect is larger; a few passes settle it.
pub fn place_client(hwnd: Hwnd, rect: PhysicalRect) {
    let (mut x, mut y, mut w, mut h) = (rect.x, rect.y, rect.w, rect.h);
    for _ in 0..4 {
        set_bounds(hwnd, x, y, w, h);
        let client = client_rect(hwnd);
        if client == rect {
            break;
        }
        x += rect.x - client.x;
        y += rect.y - client.y;
        w += rect.w - client.w;
        h += rect.h - client.h;
    }
}

/// Clips a window we own to the rectangle (`x`, `y`, `w`, `h`) in its own physical pixels: nothing outside is
/// drawn or hit-tested. The island uses it instead of resizing, which blanks a shrunk GPUI window.
pub fn set_clip(hwnd: Hwnd, x: i32, y: i32, w: i32, h: i32) {
    // SAFETY: the region is handed to the window on success (the system owns it then) and freed otherwise.
    unsafe {
        let region = CreateRectRgn(x, y, x + w, y + h);
        if SetWindowRgn(handle(hwnd), Some(region), true) == 0 {
            let _ = DeleteObject(HGDIOBJ(region.0));
        }
    }
}

/// Click-through, topmost, non-activating, layered, excluded from capture. Must run before the
/// window's first frame (Scrawl: layered after the first frame never shows under DirectComposition).
pub fn style_overlay(hwnd: Hwnd) -> Result<()> {
    let extra =
        WS_EX_TRANSPARENT | WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST;
    add_ex_style(hwnd, extra);
    // SetWindowLongPtrW reports no error for a bad handle, so read the two click-through styles back.
    // Reading all five back failed on healthy windows (live run), so the others are not checked here.
    let must_hold = (WS_EX_TRANSPARENT | WS_EX_LAYERED).0;
    if ex_style(hwnd) & must_hold != must_hold {
        return Err(windows::core::Error::from_hresult(E_FAIL));
    }
    // SAFETY: alpha 255 keeps GPUI's per-pixel alpha; the colour key is unused under LWA_ALPHA.
    let layered = unsafe { SetLayeredWindowAttributes(handle(hwnd), COLORREF(0), 255, LWA_ALPHA) };
    // A failure above must not skip the chrome strip and the capture exclusion.
    strip_chrome(hwnd);
    let excluded = apply_capture_policy(hwnd);
    layered.and(excluded)
}

/// Takes focus (no NOACTIVATE, no TRANSPARENT): the prompt box the user types into (R13 Q6). Clears
/// `WS_EX_NOACTIVATE`, so the always-present island can return to the prompt after `style_clickable_no_focus`.
pub fn style_panel(hwnd: Hwnd) -> Result<()> {
    remove_ex_style(hwnd, WS_EX_NOACTIVATE);
    add_ex_style(hwnd, WS_EX_TOOLWINDOW | WS_EX_TOPMOST);
    make_topmost(hwnd);
    strip_chrome(hwnd);
    apply_capture_policy(hwnd)
}

/// Non-activating but clickable (the R13 card and pill): no TRANSPARENT.
pub fn style_clickable_no_focus(hwnd: Hwnd) -> Result<()> {
    add_ex_style(hwnd, WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE);
    make_topmost(hwnd);
    strip_chrome(hwnd);
    apply_capture_policy(hwnd)
}

/// `WS_EX_TOPMOST` set through the style word has no effect; the z-order needs `SetWindowPos`.
fn make_topmost(hwnd: Hwnd) {
    // SAFETY: z-order change of a window we own, no move, resize or activation.
    unsafe {
        let _ = SetWindowPos(
            handle(hwnd),
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

pub fn set_capture_excluded(hwnd: Hwnd, excluded: bool) -> Result<()> {
    let affinity = if excluded {
        WDA_EXCLUDEFROMCAPTURE
    } else {
        WDA_NONE
    };
    // SAFETY: display affinity of a window in our process.
    unsafe { SetWindowDisplayAffinity(handle(hwnd), affinity) }
}

/// Windows 11 rounded corners, border and shadow give a ghost box; the accent veil dims the desktop.
fn strip_chrome(hwnd: Hwnd) {
    let h = handle(hwnd);
    let corner = DWMWCP_DONOTROUND;
    let border: u32 = 0xFFFF_FFFE; // DWMWA_COLOR_NONE
    let policy = DWMNCRP_DISABLED;
    // SAFETY: each attribute value outlives the call and `size_of` matches its type.
    unsafe {
        let _ = DwmSetWindowAttribute(
            h,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &raw const corner as *const c_void,
            size_of_val(&corner) as u32,
        );
        let _ = DwmSetWindowAttribute(
            h,
            DWMWA_BORDER_COLOR,
            &raw const border as *const c_void,
            size_of_val(&border) as u32,
        );
        let _ = DwmSetWindowAttribute(
            h,
            DWMWA_NCRENDERING_POLICY,
            &raw const policy as *const c_void,
            size_of_val(&policy) as u32,
        );
    }
    disable_accent(h);
}

/// `SetWindowCompositionAttribute(WCA_ACCENT_POLICY = 19)` with a zeroed policy (Scrawl `overlay.rs:97-122`).
fn disable_accent(hwnd: HWND) {
    #[repr(C)]
    struct AccentPolicy {
        state: u32,
        flags: u32,
        gradient: u32,
        animation: u32,
    }
    #[repr(C)]
    struct CompositionAttribData {
        attribute: u32,
        data: *mut c_void,
        size: usize,
    }
    type SetWca = unsafe extern "system" fn(HWND, *mut CompositionAttribData) -> i32;

    // SAFETY: user32 is always loaded; the export is undocumented but stable and has this signature.
    unsafe {
        let Ok(user32) = GetModuleHandleW(w!("user32.dll")) else {
            return;
        };
        let Some(proc) = GetProcAddress(
            user32,
            PCSTR(c"SetWindowCompositionAttribute".as_ptr().cast()),
        ) else {
            return;
        };
        let set_wca: SetWca = std::mem::transmute(proc);
        let mut policy = AccentPolicy {
            state: 0,
            flags: 0,
            gradient: 0,
            animation: 0,
        };
        let mut data = CompositionAttribData {
            attribute: 19,
            data: (&raw mut policy).cast(),
            size: size_of::<AccentPolicy>(),
        };
        set_wca(hwnd, &mut data);
    }
}

/// The client area of a window in physical screen pixels (what GPUI draws into).
pub fn client_rect(hwnd: Hwnd) -> PhysicalRect {
    let h = handle(hwnd);
    let mut client = RECT::default();
    let mut origin = windows::Win32::Foundation::POINT { x: 0, y: 0 };
    // SAFETY: out-parameters are locals.
    unsafe {
        let _ = GetClientRect(h, &mut client);
        let _ = ClientToScreen(h, &mut origin);
    }
    PhysicalRect {
        x: origin.x,
        y: origin.y,
        w: client.right,
        h: client.bottom,
    }
}

/// Moves the window so its client area covers `rect` (physical pixels), then shows it without
/// activating, and returns the client rect it ended up with. GPUI keeps a frame around the client area
/// and resizes to Windows' suggested rect after a DPI change, so placement repeats until the client
/// area matches. Call outside a GPUI update: a resize inside one is lost (Scrawl gotcha).
pub fn place_overlay(hwnd: Hwnd, rect: PhysicalRect) -> PhysicalRect {
    let h = handle(hwnd);
    let mut want = rect;
    let mut got = client_rect(hwnd);
    for _ in 0..4 {
        // SAFETY: placement of a window we own.
        unsafe {
            let _ = SetWindowPos(
                h,
                Some(HWND_TOPMOST),
                want.x,
                want.y,
                want.w,
                want.h,
                SWP_NOACTIVATE,
            );
        }
        got = client_rect(hwnd);
        if got == rect {
            break;
        }
        want = PhysicalRect {
            x: want.x + rect.x - got.x,
            y: want.y + rect.y - got.y,
            w: want.w + rect.w - got.w,
            h: want.h + rect.h - got.h,
        };
    }
    // SAFETY: visibility change of a window we own.
    unsafe {
        let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
    }
    got
}

pub fn show_no_activate(hwnd: Hwnd) {
    // SAFETY: visibility change of a window we own.
    unsafe {
        let _ = ShowWindow(handle(hwnd), SW_SHOWNOACTIVATE);
    }
}

pub fn hide(hwnd: Hwnd) {
    // SAFETY: visibility change of a window we own.
    unsafe {
        let _ = ShowWindow(handle(hwnd), SW_HIDE);
    }
}

/// Outer rect of a window in physical pixels (virtual-screen space).
pub fn window_rect(hwnd: Hwnd) -> PhysicalRect {
    let mut r = RECT::default();
    // SAFETY: out-parameter is a local; a bad handle leaves the zero rect.
    let _ = unsafe { GetWindowRect(handle(hwnd), &mut r) };
    PhysicalRect {
        x: r.left,
        y: r.top,
        w: r.right - r.left,
        h: r.bottom - r.top,
    }
}

/// The union bounding box of all monitors, in virtual-screen space.
pub fn virtual_screen() -> PhysicalRect {
    // SAFETY: plain getters.
    unsafe {
        PhysicalRect {
            x: GetSystemMetrics(SM_XVIRTUALSCREEN),
            y: GetSystemMetrics(SM_YVIRTUALSCREEN),
            w: GetSystemMetrics(SM_CXVIRTUALSCREEN),
            h: GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    }
}

/// The window in front right now (0 when none).
pub fn foreground_window() -> Hwnd {
    // SAFETY: plain getter.
    unsafe { GetForegroundWindow().0 as isize }
}

/// Asks a window to close (`WM_CLOSE`), for cleaning up a window that never took focus.
pub fn post_close(hwnd: Hwnd) {
    // SAFETY: posts a message to a window of ours.
    unsafe {
        let _ = PostMessageW(Some(handle(hwnd)), WM_CLOSE, WPARAM(0), LPARAM(0));
    }
}

/// Whether `hwnd` belongs to this process, so synthetic input is only sent into our own windows.
pub fn is_own_window(hwnd: Hwnd) -> bool {
    let mut pid = 0u32;
    // SAFETY: out-parameter is a local.
    unsafe {
        GetWindowThreadProcessId(handle(hwnd), Some(&mut pid));
        pid != 0 && pid == GetCurrentProcessId()
    }
}

pub fn is_visible(hwnd: Hwnd) -> bool {
    // SAFETY: plain getter.
    unsafe { IsWindowVisible(handle(hwnd)).as_bool() }
}

/// The top-level window under a physical screen point; windows that are click-through
/// (`WS_EX_TRANSPARENT`) are skipped, so this is also what a click would reach.
pub fn root_window_at(x: i32, y: i32) -> Hwnd {
    // SAFETY: plain getters.
    unsafe {
        let hit = WindowFromPoint(windows::Win32::Foundation::POINT { x, y });
        GetAncestor(hit, GA_ROOT).0 as isize
    }
}

pub fn is_window(hwnd: Hwnd) -> bool {
    // SAFETY: plain getter.
    unsafe { IsWindow(Some(handle(hwnd))).as_bool() }
}

/// Puts `hwnd` in front. Windows only lets the foreground owner do this, so the call is retried
/// attached to the foreground thread's input queue when the plain call is refused.
pub fn set_foreground(hwnd: Hwnd) -> bool {
    let target = handle(hwnd);
    // SAFETY: foreground juggling on handles; the input attachment is undone before returning.
    unsafe {
        if SetForegroundWindow(target).as_bool() {
            return GetForegroundWindow() == target;
        }
        let front = GetForegroundWindow();
        let front_thread = GetWindowThreadProcessId(front, None);
        let this_thread = GetCurrentThreadId();
        let attached = front_thread != 0
            && front_thread != this_thread
            && AttachThreadInput(this_thread, front_thread, true).as_bool();
        let ok = SetForegroundWindow(target).as_bool();
        if attached {
            let _ = AttachThreadInput(this_thread, front_thread, false);
        }
        ok && GetForegroundWindow() == target
    }
}
