//! Attach a Windows GUI build to the parent console, and record fatal errors.
//!
//! Release binaries use the Windows subsystem so double-clicking does not open
//! a terminal. `--version` and `--smoke-test` still print when a console
//! parent started the process.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

static APP: OnceLock<&'static str> = OnceLock::new();
static NO_DIALOG: AtomicBool = AtomicBool::new(false);

/// Attach to a parent console when this is a Windows release build, then
/// install the panic log. Call this before any `println!`.
pub fn init(app: &'static str) {
    let _ = APP.set(app);
    attach_parent_console();
    install_crash_hook();
}

pub fn suppress_dialog() {
    NO_DIALOG.store(true, Ordering::Relaxed);
}

pub fn report_fatal(message: &str) -> ! {
    record(message);
    std::process::exit(1);
}

fn app_name() -> &'static str {
    APP.get().copied().unwrap_or("mithu")
}

fn install_crash_hook() {
    std::panic::set_hook(Box::new(|info| {
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
            .unwrap_or("unknown panic");
        let loc = info
            .location()
            .map(|location| format!(" ({location})"))
            .unwrap_or_default();
        record(&format!("panic: {payload}{loc}"));
    }));
}

fn record(message: &str) {
    let line = format!("{message}\n");
    eprintln!("{}: {message}", app_name());
    if let Some(path) = log_path() {
        let _ = std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")));
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut file| file.write_all(line.as_bytes()));
    }
    show_dialog(message);
}

pub(crate) fn log_path() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let base = std::env::var_os("LOCALAPPDATA")?;
        Some(log_file(Path::new(&base)))
    }
    #[cfg(not(windows))]
    {
        let home = std::env::var_os("HOME")?;
        Some(
            PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("Mithu")
                .join("mithu.log"),
        )
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn log_file(local_app_data: &Path) -> PathBuf {
    local_app_data.join("Mithu").join("mithu.log")
}

fn show_dialog(message: &str) {
    if NO_DIALOG.load(Ordering::Relaxed)
        || std::env::var_os("MITHU_NO_DIALOG").is_some()
        || std::env::var_os("CI").is_some()
    {
        return;
    }
    #[cfg(windows)]
    {
        // Timeout so a fatal error cannot sit forever on a hidden desktop.
        let script = format!(
            "$w = New-Object -ComObject WScript.Shell; [void]$w.Popup({}, 20, '{}', 16)",
            powershell_string(message),
            app_name()
        );
        let _ = std::process::Command::new("powershell")
            .arg("-NoProfile")
            .arg("-Command")
            .arg(script)
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = message;
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn powershell_string(message: &str) -> String {
    let mut out = String::from("\"");
    for ch in message.chars() {
        match ch {
            '"' => out.push_str("\"\""),
            '\r' | '\n' => out.push(' '),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// A Windows GUI process has no console. If a terminal launched it, take that
/// console and point the standard handles at it so `println!` is visible.
/// Double-clicking has no console parent, so this returns and no terminal appears.
///
/// A redirected stdout (the pipe PowerShell uses for `$output = & mithu.exe`)
/// must be left alone. Replacing it with `CONOUT$` prints on the console and
/// leaves the pipe empty, which is why CI saw a successful smoke test with
/// no captured text.
pub fn attach_parent_console() {
    #[cfg(all(windows, not(debug_assertions)))]
    unsafe {
        const ATTACH_PARENT_PROCESS: u32 = 0xFFFF_FFFF;
        const STD_OUTPUT_HANDLE: u32 = 0xFFFF_FFF5; // -11
        const STD_ERROR_HANDLE: u32 = 0xFFFF_FFF4; // -12
        const GENERIC_READ: u32 = 0x8000_0000;
        const GENERIC_WRITE: u32 = 0x4000_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        const OPEN_EXISTING: u32 = 3;
        const INVALID_HANDLE_VALUE: *mut core::ffi::c_void = -1isize as *mut core::ffi::c_void;

        unsafe extern "system" {
            fn AttachConsole(process_id: u32) -> i32;
            fn GetStdHandle(std_handle: u32) -> *mut core::ffi::c_void;
            fn GetFileType(handle: *mut core::ffi::c_void) -> u32;
            fn SetStdHandle(std_handle: u32, handle: *mut core::ffi::c_void) -> i32;
            fn CreateFileA(
                file_name: *const u8,
                desired_access: u32,
                share_mode: u32,
                security: *mut core::ffi::c_void,
                disposition: u32,
                flags: u32,
                template: *mut core::ffi::c_void,
            ) -> *mut core::ffi::c_void;
        }

        let existing = GetStdHandle(STD_OUTPUT_HANDLE);
        let valid = !existing.is_null() && existing != INVALID_HANDLE_VALUE;
        let kind = if valid { GetFileType(existing) } else { 0 };
        if !console_needs_attach(valid, kind) {
            return;
        }
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return;
        }
        let handle = CreateFileA(
            b"CONOUT$\0".as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            core::ptr::null_mut(),
            OPEN_EXISTING,
            0,
            core::ptr::null_mut(),
        );
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return;
        }
        SetStdHandle(STD_OUTPUT_HANDLE, handle);
        SetStdHandle(STD_ERROR_HANDLE, handle);
    }
}

/// Keep a pipe or a file. Attach a console only when this GUI process has no
/// stdout yet (double-click, or a console parent that did not redirect).
pub(crate) fn console_needs_attach(handle_valid: bool, file_type: u32) -> bool {
    const FILE_TYPE_DISK: u32 = 1;
    const FILE_TYPE_CHAR: u32 = 2;
    const FILE_TYPE_PIPE: u32 = 3;
    if !handle_valid {
        return true;
    }
    !matches!(file_type, FILE_TYPE_DISK | FILE_TYPE_CHAR | FILE_TYPE_PIPE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_file_is_mithu_log_under_localappdata() {
        let path = log_file(Path::new(r"C:\Users\a\AppData\Local"));
        assert_eq!(
            path,
            Path::new(r"C:\Users\a\AppData\Local").join("Mithu").join("mithu.log")
        );
    }

    #[test]
    fn powershell_string_escapes_quotes_and_newlines() {
        assert_eq!(powershell_string("a \"b\"\nc"), "\"a \"\"b\"\" c\"");
    }

    #[test]
    fn attach_is_callable_on_this_host() {
        attach_parent_console();
    }

    #[test]
    fn redirected_stdout_is_not_replaced_with_the_console() {
        const FILE_TYPE_DISK: u32 = 1;
        const FILE_TYPE_CHAR: u32 = 2;
        const FILE_TYPE_PIPE: u32 = 3;
        assert!(console_needs_attach(false, 0));
        assert!(!console_needs_attach(true, FILE_TYPE_PIPE));
        assert!(!console_needs_attach(true, FILE_TYPE_DISK));
        assert!(!console_needs_attach(true, FILE_TYPE_CHAR));
        assert!(console_needs_attach(true, 0));
    }
}
