use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::Command;
use tracing::info;

use crate::git;

/// Open a path in VS Code, handling SSH + kitty remote scenarios.
///
/// Detection logic:
/// - `SSH_ALIAS` env var set → remote host, use kitty remote control
///   to launch VS Code on the local machine with `--remote ssh-remote+HOST`
/// - Otherwise → local, use `code` directly
///
/// The `SSH_ALIAS` env var is set via SSH config (`SetEnv SSH_ALIAS=host`).
pub fn open_in_editor(path: &Path) -> Result<()> {
    let path_str = path.to_string_lossy();
    let env = resolve_remote_env();

    if let Some(alias) = env.ssh_alias {
        let listen_on = env
            .kitty_listen_on
            .context("KITTY_LISTEN_ON not available")?;
        ensure_kitty_remote(&listen_on)?;
        let code_cmd = format!("code --remote 'ssh-remote+{}' '{}'", alias, path_str);
        info!(path = %path_str, alias = %alias, "code:open via kitty remote");
        run_kitten_background(&["zsh", "-lic", &code_cmd], &listen_on)?;
    } else {
        info!(path = %path_str, "code:open locally");
        let output = Command::new("code")
            .arg(path)
            .output()
            .context("Failed to run VS Code. Is 'code' in your PATH?")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("VS Code failed: {}", stderr.trim());
        }
    }

    Ok(())
}

/// Open a PR in VS Code's GitHub Pull Request extension.
///
/// Looks up the PR URL for the worktree via `gh pr view`, then opens it
/// using the `vscode://` URI scheme.
pub fn open_pr_in_editor(worktree_path: &Path) -> Result<()> {
    let pr_url = get_pr_url(worktree_path)
        .ok_or_else(|| anyhow::anyhow!("No PR found for this worktree"))?;

    let vscode_uri = format!(
        "vscode://github.vscode-pull-request-github/checkout-pull-request?uri={}",
        pr_url
    );

    let env = resolve_remote_env();
    if let Some(_alias) = env.ssh_alias {
        let listen_on = env
            .kitty_listen_on
            .context("KITTY_LISTEN_ON not available")?;
        ensure_kitty_remote(&listen_on)?;
        info!(pr_url = %pr_url, "code:open PR via kitty remote");
        run_kitten_background(&["open", &vscode_uri], &listen_on)?;
    } else {
        info!(pr_url = %pr_url, "code:open PR locally");
        let output = Command::new("open")
            .arg(&vscode_uri)
            .output()
            .context("Failed to open PR URL")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Failed to open PR: {}", stderr.trim());
        }
    }

    Ok(())
}

/// Get the PR URL for a worktree using `gh pr view`.
fn get_pr_url(worktree_path: &Path) -> Option<String> {
    Command::new("gh")
        .args(["pr", "view", "--json", "url", "-q", ".url"])
        .current_dir(worktree_path)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Run a command on the local machine via kitty remote control.
fn run_kitten_background(cmd_args: &[&str], listen_on: &str) -> Result<()> {
    let mut args = vec!["@", "launch", "--type=background", "--"];
    args.extend_from_slice(cmd_args);
    let output = Command::new("kitten")
        .args(&args)
        .env("KITTY_LISTEN_ON", listen_on)
        .output()
        .context("Failed to run kitten @ launch")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("kitten @ launch failed: {}", stderr.trim());
    }
    Ok(())
}

/// Verify that kitty remote control is available.
fn ensure_kitty_remote(listen_on: &str) -> Result<()> {
    let output = Command::new("kitten")
        .args(["@", "ls"])
        .env("KITTY_LISTEN_ON", listen_on)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .context("Failed to run kitten. Is kitty installed?")?;
    if !output.success() {
        bail!(
            "Kitty remote control not available.\n\
             SSH_ALIAS is set but kitten @ is not reachable.\n\
             Connect via `kitten ssh` or configure kitty remote control."
        );
    }
    Ok(())
}

/// SSH-related environment resolved for the current process.
struct RemoteEnv {
    ssh_alias: Option<String>,
    kitty_listen_on: Option<String>,
}

/// Resolve SSH_ALIAS and KITTY_LISTEN_ON, preferring fresh values from the
/// tmux client process over potentially stale inherited env vars.
///
/// Inside tmux over SSH, the inherited env vars may be stale from a previous
/// SSH connection: the kitty SSH kitten assigns a new reverse-forwarded port
/// on each reconnection, and SSH_ALIAS may not be in the tmux server's
/// environment at all. This resolves fresh values by reading the tmux client
/// process's environment from procfs (one tmux call + one file read).
fn resolve_remote_env() -> RemoteEnv {
    let inherited = RemoteEnv {
        ssh_alias: std::env::var("SSH_ALIAS").ok(),
        kitty_listen_on: std::env::var("KITTY_LISTEN_ON").ok(),
    };

    if std::env::var("TMUX").is_err() {
        return inherited;
    }

    let Some(environ) = read_tmux_client_environ() else {
        return inherited;
    };

    let find_var = |name: &str| -> Option<String> {
        let prefix = format!("{name}=");
        environ
            .split(|&b| b == 0)
            .filter_map(|entry| std::str::from_utf8(entry).ok())
            .find_map(|s| s.strip_prefix(&prefix).map(String::from))
    };

    let fresh_listen_on = find_var("KITTY_LISTEN_ON");
    if let (Some(fresh), Some(stale)) = (&fresh_listen_on, &inherited.kitty_listen_on) {
        if fresh != stale {
            info!(stale = %stale, fresh = %fresh, "resolved fresh KITTY_LISTEN_ON from tmux client");
        }
    }

    RemoteEnv {
        ssh_alias: find_var("SSH_ALIAS").or(inherited.ssh_alias),
        kitty_listen_on: fresh_listen_on.or(inherited.kitty_listen_on),
    }
}

/// Read raw environ bytes from the tmux client process via procfs.
fn read_tmux_client_environ() -> Option<Vec<u8>> {
    let output = Command::new("tmux")
        .args(["display-message", "-p", "#{client_pid}"])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let pid = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if pid.is_empty() {
        return None;
    }
    std::fs::read(format!("/proc/{pid}/environ")).ok()
}

/// CLI entry point
pub fn run(name: Option<&str>, pr: bool) -> Result<()> {
    let resolved_name = super::resolve_name(name)
        .context("Could not determine worktree. Provide a name or run from inside a worktree.")?;

    let (path, _branch) = git::find_worktree(&resolved_name).with_context(|| {
        format!(
            "No worktree found with name '{}'. Use 'workmux list' to see available worktrees.",
            resolved_name
        )
    })?;

    if pr {
        open_pr_in_editor(&path)?;
        println!("✓ Opened PR in VS Code");
    } else {
        open_in_editor(&path)?;
        println!("✓ Opened '{}' in VS Code", resolved_name);
    }

    Ok(())
}
