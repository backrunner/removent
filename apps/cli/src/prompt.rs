//! A cancellable hidden PIN prompt. Unlike spawn_blocking(stdin), dropping this
//! future restores the terminal immediately when the handshake ends or times out.
use rust_i18n::t;
use std::fs::{File, OpenOptions};
use std::io::{IsTerminal, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;

struct HiddenInput {
    file: File,
    original: libc::termios,
}

impl HiddenInput {
    fn open() -> std::io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open("/dev/tty")?;
        let mut original = std::mem::MaybeUninit::<libc::termios>::uninit();
        // SAFETY: file is an open terminal; tcgetattr initializes the structure on success.
        if unsafe { libc::tcgetattr(file.as_raw_fd(), original.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: successful tcgetattr initialized original.
        let original = unsafe { original.assume_init() };
        let mut hidden = original;
        hidden.c_lflag &= !(libc::ECHO | libc::ICANON | libc::ISIG);
        hidden.c_cc[libc::VMIN] = 0;
        hidden.c_cc[libc::VTIME] = 0;
        // SAFETY: valid fd and initialized termios structure.
        if unsafe { libc::tcsetattr(file.as_raw_fd(), libc::TCSANOW, &hidden) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self { file, original })
    }
}

impl Drop for HiddenInput {
    fn drop(&mut self) {
        // SAFETY: file stays open through Drop; original was read from this terminal.
        unsafe {
            libc::tcsetattr(self.file.as_raw_fd(), libc::TCSANOW, &self.original);
        }
        let _ = writeln!(self.file);
    }
}

pub(super) async fn pin() -> anyhow::Result<String> {
    hidden(&t!("terminal.pin"), true).await
}

async fn hidden(prompt: &str, digits_only: bool) -> anyhow::Result<String> {
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        anyhow::bail!(t!("terminal.pin_required"));
    }
    // Disable echo BEFORE displaying the prompt. Fast paste must never echo a
    // secret or be lost by a later terminal input flush.
    let mut terminal = HiddenInput::open()?;
    write!(terminal.file, "{prompt}: ")?;
    let mut value = Vec::new();
    loop {
        let mut byte = [0];
        match terminal.file.read(&mut byte) {
            Ok(1) => match byte[0] {
                b'\r' | b'\n' => {
                    return String::from_utf8(value)
                        .map_err(|_| anyhow::anyhow!(t!("settings.stdin_invalid")));
                }
                3 | 4 | 27 => {
                    // SAFETY: notify the interactive/ping cancellation handler.
                    unsafe {
                        libc::raise(libc::SIGINT);
                    }
                    return Err(
                        std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled").into(),
                    );
                }
                8 | 127 => while value.pop().is_some() && std::str::from_utf8(&value).is_err() {},
                character
                    if !character.is_ascii_control()
                        && (!digits_only || character.is_ascii_digit()) =>
                {
                    if value.len() >= 4096 {
                        anyhow::bail!(t!("settings.stdin_invalid"));
                    }
                    value.push(character);
                }
                _ => {}
            },
            Ok(_) => tokio::time::sleep(std::time::Duration::from_millis(30)).await,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

pub(super) struct InterruptGuard(tokio::task::JoinHandle<()>);
impl Drop for InterruptGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(super) fn interrupt_guard() -> std::io::Result<InterruptGuard> {
    let mut signals = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let mut original = std::mem::MaybeUninit::<libc::termios>::uninit();
    // SAFETY: interactive callers require a terminal on stdin.
    if unsafe { libc::tcgetattr(libc::STDIN_FILENO, original.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: successful tcgetattr initialized the structure.
    let original = unsafe { original.assume_init() };
    Ok(InterruptGuard(tokio::spawn(async move {
        signals.recv().await;
        // SAFETY: stdin remains open for this interactive process.
        unsafe {
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &original);
        }
        let _ = console::Term::stderr().show_cursor();
        let _ = write!(std::io::stderr(), "\x1b[?2004l");
        let _ = std::io::stderr().flush();
        std::process::exit(130);
    })))
}
