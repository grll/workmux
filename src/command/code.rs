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

    if let Ok(alias) = std::env::var("SSH_ALIAS") {
        ensure_kitty_remote()?;
        let code_cmd = format!("code --remote 'ssh-remote+{}' '{}'", alias, path_str);
        info!(path = %path_str, alias = %alias, "code:open via kitty remote");
        run_kitten_background(&["zsh", "-lc", &code_cmd])?;
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

    if std::env::var("SSH_ALIAS").is_ok() {
        ensure_kitty_remote()?;
        info!(pr_url = %pr_url, "code:open PR via kitty remote");
        run_kitten_background(&["open", &vscode_uri])?;
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
fn run_kitten_background(cmd_args: &[&str]) -> Result<()> {
    let mut args = vec!["@", "launch", "--type=background", "--"];
    args.extend_from_slice(cmd_args);
    let output = Command::new("kitten")
        .args(&args)
        .output()
        .context("Failed to run kitten @ launch")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("kitten @ launch failed: {}", stderr.trim());
    }
    Ok(())
}

/// Verify that kitty remote control is available.
fn ensure_kitty_remote() -> Result<()> {
    let output = Command::new("kitten")
        .args(["@", "ls"])
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
