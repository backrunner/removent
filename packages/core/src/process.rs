//! Time-bounded system commands used by service management and recovery.
use anyhow::{Result, bail};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::{
    io::Read,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

pub fn run_bounded(command: &mut Command, timeout: Duration) -> Result<Output> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            if self.0.try_wait().ok().flatten().is_none() {
                #[cfg(unix)]
                unsafe {
                    libc::kill(-(self.0.id() as i32), libc::SIGKILL);
                }
                #[cfg(windows)]
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }
    let mut child = Child(command.spawn()?);
    // Drain concurrently with a fixed memory cap. Recovery commands must not
    // need new temporary files when the application volume is full.
    let (send, receive) = std::sync::mpsc::channel();
    for (stderr, stream) in [
        (
            false,
            Box::new(child.0.stdout.take().unwrap()) as Box<dyn Read + Send>,
        ),
        (
            true,
            Box::new(child.0.stderr.take().unwrap()) as Box<dyn Read + Send>,
        ),
    ] {
        let send = send.clone();
        std::thread::Builder::new()
            .name("command-output".into())
            .spawn(move || {
                let _ = send.send((stderr, drain(stream)));
            })?;
    }
    drop(send);
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.0.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            bail!("Update subprocess timed out: {:?}", command.get_program());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut output = Output {
        status,
        stdout: Vec::new(),
        stderr: Vec::new(),
    };
    for _ in 0..2 {
        let (stderr, bytes) = receive.recv_timeout(Duration::from_secs(2))?;
        if stderr {
            output.stderr = bytes?;
        } else {
            output.stdout = bytes?;
        }
    }
    Ok(output)
}

fn drain(mut stream: Box<dyn Read + Send>) -> std::io::Result<Vec<u8>> {
    const LIMIT: usize = 1024 * 1024;
    let mut output = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let count = match stream.read(&mut buffer) {
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            return Ok(output);
        }
        let retained = count.min(LIMIT.saturating_sub(output.len()));
        output.extend_from_slice(&buffer[..retained]);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[test]
    fn timeout_reaps_a_wedged_command_and_its_descendants() {
        let started = Instant::now();
        let error = run_bounded(
            Command::new("/bin/sh").args(["-c", "sleep 30 & wait"]),
            Duration::from_millis(100),
        )
        .unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(3));
    }
    #[test]
    fn bounded_command_preserves_exit_status_and_diagnostics() {
        let output = run_bounded(
            Command::new("/bin/sh").args(["-c", "printf ready; printf failed >&2; exit 7"]),
            Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(output.status.code(), Some(7));
        assert_eq!(output.stdout, b"ready");
        assert_eq!(output.stderr, b"failed");
    }
}
