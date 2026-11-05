use anyhow::Result;
use std::io::{self, Write};
use ysunethelper::shutdown;

/// Keep terminal restoration on the caller thread, so cancellation can restore
/// echo even while the input thread is blocked inside the OS console read.
pub fn line(prompt: &str, secret: bool) -> Result<String> {
    shutdown::check()?;
    let _echo = if secret { Some(NoEcho::new()?) } else { None };
    eprint!("{prompt}");
    io::stderr().flush()?;
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut line = String::new();
        let result = io::stdin().read_line(&mut line).and_then(|n| {
            if n == 0 {
                Err(io::ErrorKind::UnexpectedEof.into())
            } else {
                Ok(line)
            }
        });
        let _ = tx.send(result);
    });
    let result = shutdown::receive(&rx);
    if secret {
        eprintln!();
    }
    Ok(result??.trim_end_matches(['\r', '\n']).to_owned())
}

#[cfg(unix)]
struct NoEcho(libc::termios);
#[cfg(unix)]
impl NoEcho {
    fn new() -> io::Result<Self> {
        let mut original = std::mem::MaybeUninit::uninit();
        // stdin is a live terminal, checked by the login command before prompting.
        if unsafe { libc::tcgetattr(libc::STDIN_FILENO, original.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let original = unsafe { original.assume_init() };
        let guard = Self(original);
        let mut hidden = original;
        hidden.c_lflag &= !(libc::ECHO | libc::ECHONL);
        if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &hidden) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(guard)
    }
}
#[cfg(unix)]
impl Drop for NoEcho {
    fn drop(&mut self) {
        loop {
            if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.0) } == 0
                || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted
            {
                break;
            }
        }
    }
}

#[cfg(windows)]
struct NoEcho {
    handle: windows_sys::Win32::Foundation::HANDLE,
    original: u32,
}
#[cfg(windows)]
impl NoEcho {
    fn new() -> io::Result<Self> {
        use windows_sys::Win32::{Foundation::INVALID_HANDLE_VALUE, System::Console::*};
        let handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
        let mut original = 0;
        if handle.is_null()
            || handle == INVALID_HANDLE_VALUE
            || unsafe { GetConsoleMode(handle, &mut original) } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if unsafe { SetConsoleMode(handle, original & !ENABLE_ECHO_INPUT) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { handle, original })
    }
}
#[cfg(windows)]
impl Drop for NoEcho {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::System::Console::SetConsoleMode(self.handle, self.original);
        }
    }
}
