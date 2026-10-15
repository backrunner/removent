use crossterm::event::{self, Event, KeyEventKind};
use std::io::{self, Write};
use std::time::Duration;

/// One live prompt. It is always shorter than the terminal, so relative cursor
/// movement cannot reach shell history. Completed results are printed separately.
pub(crate) struct Region {
    lines: usize,
    columns: u16,
    restore_raw: bool,
    blocked: bool,
}

impl Region {
    pub fn new() -> io::Result<Self> {
        io::stdout().flush()?;
        let restore_raw = !crossterm::terminal::is_raw_mode_enabled()?;
        if restore_raw {
            crossterm::terminal::enable_raw_mode()?;
        }
        let region = Self {
            lines: 0,
            columns: console::Term::stderr().size().1,
            restore_raw,
            blocked: false,
        };
        write!(io::stderr(), "\x1b[?25l\x1b[?2004h")?;
        Ok(region)
    }

    fn clear(&mut self) -> io::Result<()> {
        if self.lines > 0 {
            // On resize, terminals reflow prior rows differently. Preserve the
            // previous snapshot rather than risk erasing shell history.
            if self.columns == console::Term::stderr().size().1 {
                console::Term::stderr().clear_last_lines(self.lines)?;
            }
            self.lines = 0;
        }
        Ok(())
    }

    pub fn draw(&mut self, lines: &[String]) -> io::Result<()> {
        self.clear()?;
        let (rows, columns) = console::Term::stderr().size();
        self.columns = columns;
        self.blocked = lines.len() >= usize::from(rows)
            || lines
                .iter()
                .any(|line| console::measure_text_width(line) >= usize::from(columns));
        let fallback = vec![super::fit(
            &rust_i18n::t!("tui.resize"),
            usize::from(columns).saturating_sub(1),
        )];
        let lines = if self.blocked { &fallback } else { lines };
        let mut output = io::stderr().lock();
        for line in lines {
            write!(output, "{line}\r\n")?;
        }
        self.lines = lines.len();
        output.flush()
    }

    pub async fn next(&self) -> io::Result<Event> {
        loop {
            let event = event().await?;
            if !self.blocked
                || matches!(&event, Event::Resize(_, _))
                || matches!(&event, Event::Key(key) if key.code == event::KeyCode::Esc || key.code == event::KeyCode::Char('q') || (key.code == event::KeyCode::Char('c') && key.modifiers.contains(event::KeyModifiers::CONTROL)))
            {
                return Ok(event);
            }
        }
    }
}

impl Drop for Region {
    fn drop(&mut self) {
        let _ = self.clear();
        let _ = write!(io::stderr(), "\x1b[?25h\x1b[?2004l");
        let _ = io::stderr().flush();
        if self.restore_raw {
            let _ = crossterm::terminal::disable_raw_mode();
        }
    }
}

pub(crate) async fn event() -> io::Result<Event> {
    loop {
        if event::poll(Duration::ZERO)? {
            let event = event::read()?;
            if matches!(&event, Event::Key(key) if key.kind == KeyEventKind::Release) {
                continue;
            }
            return Ok(event);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

pub(crate) fn cancelled() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "cancelled")
}

/// Keep input uncooked across prompt/result transitions, so fast typing cannot
/// be echoed or discarded between components. Preserve normal output newlines
/// and SIGINT for long-running commands.
pub(crate) struct Session;
impl Session {
    pub fn new() -> io::Result<Self> {
        crossterm::terminal::enable_raw_mode()?;
        let session = Self;
        let mut mode = std::mem::MaybeUninit::<libc::termios>::uninit();
        // SAFETY: interactive entry requires an open terminal on stdin.
        if unsafe { libc::tcgetattr(libc::STDIN_FILENO, mode.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: tcgetattr initialized the structure.
        let mut mode = unsafe { mode.assume_init() };
        mode.c_oflag |= libc::OPOST | libc::ONLCR;
        mode.c_lflag |= libc::ISIG;
        // SAFETY: initialized termios for this open terminal.
        if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &mode) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(session)
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}
