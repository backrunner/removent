use super::*;

// ---- shell helpers ----

#[derive(Debug)]
pub(super) struct FetchError {
    pub(super) exit_code: Option<i32>,
    pub(super) http_status: Option<u16>,
    pub(super) detail: String,
}

impl FetchError {
    pub(super) fn is_transient(&self) -> bool {
        // Retry interrupted transfers as well as temporary server failures.
        // A missing asset, denied request or invalid certificate needs a fix,
        // not more requests (GitHub also reports API rate limits with 403).
        matches!(
            self.exit_code,
            Some(5 | 6 | 7 | 18 | 28 | 52 | 55 | 56 | 92)
        ) || (self.exit_code == Some(22)
            && matches!(self.http_status, Some(408 | 429 | 500 | 502 | 503 | 504)))
    }

    pub(super) fn user_message(&self) -> String {
        let reason = match self.http_status {
            Some(403) => t!("update.err.http_forbidden").to_string(),
            Some(404) => t!("update.err.http_not_found").to_string(),
            Some(429) => t!("update.err.http_rate_limit").to_string(),
            Some(status) if status >= 400 => format!("HTTP {status}"),
            _ => match self.exit_code {
                Some(5 | 6) => t!("update.err.dns").to_string(),
                Some(7) => t!("update.err.connect").to_string(),
                Some(28) => t!("update.err.timeout").to_string(),
                Some(35 | 60) => t!("update.err.tls").to_string(),
                _ => self.detail.clone(),
            },
        };
        t!("update.err.fetch", err = reason).to_string()
    }
}

pub(super) fn curl_get(url: &str) -> Result<String, String> {
    fetch_with_retry(url, Duration::from_secs(30), curl_get_once)
}

pub(super) fn fetch_with_retry(
    url: &str,
    timeout: Duration,
    mut fetch: impl FnMut(&str, Duration) -> Result<Vec<u8>, FetchError>,
) -> Result<String, String> {
    let deadline = Instant::now() + timeout;
    for attempt in 0..3 {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match fetch(url, remaining) {
            Ok(body) => {
                return String::from_utf8(body)
                    .map_err(|e| t!("update.err.bad_manifest", err = e.to_string()).to_string());
            }
            Err(error) => {
                let delay = Duration::from_secs(1 << attempt);
                let retry = attempt < 2
                    && error.is_transient()
                    && deadline.saturating_duration_since(Instant::now()) > delay;
                // Avoid recording mirror URLs or raw curl stderr: either can
                // contain credentials. Keep enough context to diagnose failures.
                tracing::warn!(
                    source = "manifest",
                    attempt = attempt + 1,
                    exit_code = error.exit_code,
                    http_status = error.http_status,
                    retry,
                    "update request failed"
                );
                if !retry {
                    return Err(error.user_message());
                }
                std::thread::sleep(delay);
            }
        }
    }
    unreachable!("the final attempt always returns")
}

pub(super) fn curl_get_once(url: &str, timeout: Duration) -> Result<Vec<u8>, FetchError> {
    let out = Command::new("/usr/bin/curl")
        .args([
            "-fsSL",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "10",
            "--max-time",
            &format!("{:.3}", timeout.as_secs_f64().max(0.001)),
            "--max-filesize",
            "2097152",
            "--user-agent",
            "Removent-Updater",
            // Keep the status separate from the body, including for errors.
            "--write-out",
            "%{stderr}\n%{http_code}",
            "--url",
            url,
        ])
        .output()
        .map_err(|e| FetchError {
            exit_code: None,
            http_status: None,
            detail: e.to_string(),
        })?;
    decode_fetch_output(out)
}

pub(super) fn decode_fetch_output(out: std::process::Output) -> Result<Vec<u8>, FetchError> {
    let stderr = String::from_utf8_lossy(&out.stderr);
    let (detail, status) = stderr.rsplit_once('\n').unwrap_or((&stderr, ""));
    let http_status = status.trim().parse::<u16>().ok().filter(|s| *s != 0);
    if !out.status.success() {
        let detail = detail.trim().chars().take(512).collect::<String>();
        return Err(FetchError {
            exit_code: out.status.code(),
            http_status,
            detail: if detail.is_empty() {
                format!("curl {}", out.status)
            } else {
                detail
            },
        });
    }
    Ok(out.stdout)
}
