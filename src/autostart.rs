//! Start at login: a value in the per-user `Run` key, so it needs no admin rights.

use anyhow::Context;
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RegDeleteKeyValueW, RegSetKeyValueW,
};
use windows::core::{HSTRING, w};

const RUN_KEY: windows::core::PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
const NAME: windows::core::PCWSTR = w!("notchd");

pub fn set(enabled: bool) -> anyhow::Result<()> {
    if enabled {
        let exe = std::env::current_exe().context("cannot find notchd.exe")?;
        let command = HSTRING::from(format!("\"{}\"", exe.display()));
        // The size is in bytes and includes the terminating NUL.
        let bytes = (command.len() + 1) * 2;
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                NAME,
                REG_SZ.0,
                Some(command.as_ptr().cast()),
                bytes as u32,
            )
        }
        .ok()
        .context("cannot write the Run key")
    } else {
        // Already absent is the state asked for.
        let _ = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, NAME) };
        Ok(())
    }
}
