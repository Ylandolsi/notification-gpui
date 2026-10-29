//! Screen geometry shared by the window helpers and the monitor list.

/// A rectangle in physical pixels, in virtual-screen space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhysicalRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}
