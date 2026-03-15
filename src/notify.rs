use std::process::Command;
use tracing::debug;

/// Send a desktop notification via `kitten notify` when running inside kitty.
///
/// Fire-and-forget: spawns the process without waiting, and silently ignores
/// errors so that notification failures never interrupt the status flow.
pub fn notify_agent_status(status: &str, window_name: Option<&str>) {
    if std::env::var_os("KITTY_LISTEN_ON").is_none() {
        return;
    }

    let body = match window_name {
        Some(name) => format!("Agent {status} — wm:{name}"),
        None => format!("Agent {status}"),
    };

    match Command::new("kitten")
        .args(["notify", "--title", "workmux", &body])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(_) => {}
        Err(e) => debug!(error = %e, "failed to spawn kitten notify"),
    }
}
