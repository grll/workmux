//! Codex status tracking setup.
//!
//! Detects Codex via the `~/.codex/` directory.
//! Installs hooks by merging into `~/.codex/hooks.json`.

use anyhow::{Context, Result};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

use super::StatusCheck;

/// Hooks config embedded from `.codex/hooks.json` at compile time.
const HOOKS_JSON: &str = include_str!("../../.codex/hooks.json");

fn codex_dir() -> Option<PathBuf> {
    home::home_dir().map(|h| h.join(".codex"))
}

fn hooks_path() -> Option<PathBuf> {
    codex_dir().map(|d| d.join("hooks.json"))
}

/// Detect if Codex is present via filesystem.
/// Returns the reason string if detected, None otherwise.
pub fn detect() -> Option<&'static str> {
    if codex_dir().is_some_and(|d| d.is_dir()) {
        return Some("found ~/.codex/");
    }

    None
}

/// Check if workmux hooks are installed in Codex hooks.json.
pub fn check() -> Result<StatusCheck> {
    let Some(path) = hooks_path() else {
        return Ok(StatusCheck::NotInstalled);
    };

    if !path.exists() {
        return Ok(StatusCheck::NotInstalled);
    }

    let content = fs::read_to_string(&path).context("Failed to read ~/.codex/hooks.json")?;
    let config: Value =
        serde_json::from_str(&content).context("~/.codex/hooks.json is not valid JSON")?;

    Ok(check_hooks_config(&config))
}

/// Check a parsed hooks.json value for workmux status tracking configuration.
fn check_hooks_config(config: &Value) -> StatusCheck {
    if has_workmux_hooks(config) {
        StatusCheck::Installed
    } else {
        StatusCheck::NotInstalled
    }
}

/// Check if the hooks config contains any workmux set-window-status commands.
fn has_workmux_hooks(config: &Value) -> bool {
    let Some(hooks) = config.get("hooks").and_then(|v| v.as_object()) else {
        return false;
    };

    for (_event, groups) in hooks {
        let Some(groups_arr) = groups.as_array() else {
            continue;
        };
        for group in groups_arr {
            let Some(hook_list) = group.get("hooks").and_then(|v| v.as_array()) else {
                continue;
            };
            for hook in hook_list {
                if let Some(cmd) = hook.get("command").and_then(|v| v.as_str())
                    && cmd.contains("workmux set-window-status")
                {
                    return true;
                }
            }
        }
    }

    false
}

/// Load the hooks object from the embedded hooks.json.
fn load_hooks_from_embedded() -> Result<Value> {
    let config: Value =
        serde_json::from_str(HOOKS_JSON).expect("embedded hooks.json is valid JSON");
    config
        .get("hooks")
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("hooks.json missing hooks key"))
}

/// Install workmux hooks into `~/.codex/hooks.json`.
///
/// Merges hook groups into existing hooks without clobbering or creating
/// duplicates. Returns a description of what was done.
pub fn install() -> Result<String> {
    let path = hooks_path().ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?;

    // Read existing config or start fresh
    let mut config: Value = if path.exists() {
        let content = fs::read_to_string(&path).context("Failed to read ~/.codex/hooks.json")?;
        serde_json::from_str(&content).context("~/.codex/hooks.json is not valid JSON")?
    } else {
        // Ensure ~/.codex/ directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).context("Failed to create ~/.codex/ directory")?;
        }
        Value::Object(serde_json::Map::new())
    };

    let hooks_to_add = load_hooks_from_embedded()?;

    // Ensure config.hooks exists as an object
    let config_obj = config
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("hooks.json root is not an object"))?;

    if !config_obj.contains_key("hooks") {
        config_obj.insert("hooks".to_string(), Value::Object(serde_json::Map::new()));
    }

    let existing_hooks = config_obj
        .get_mut("hooks")
        .and_then(|v| v.as_object_mut())
        .ok_or_else(|| anyhow::anyhow!("hooks.json hooks is not an object"))?;

    // Merge each hook event, deduplicating by value equality
    let hooks_map = hooks_to_add
        .as_object()
        .expect("embedded hooks is an object");
    for (event, hook_groups) in hooks_map {
        let Some(new_groups) = hook_groups.as_array() else {
            continue;
        };

        if let Some(existing_groups) = existing_hooks.get_mut(event) {
            let arr = existing_groups
                .as_array_mut()
                .ok_or_else(|| anyhow::anyhow!("hooks.{event} is not an array"))?;
            for group in new_groups {
                if !arr.contains(group) {
                    arr.push(group.clone());
                }
            }
        } else {
            existing_hooks.insert(event.clone(), hook_groups.clone());
        }
    }

    // Write back with pretty formatting
    let output = serde_json::to_string_pretty(&config)?;
    fs::write(&path, output + "\n").context("Failed to write ~/.codex/hooks.json")?;

    Ok("Installed hooks to ~/.codex/hooks.json".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_has_workmux_hooks_empty() {
        let config = json!({});
        assert!(!has_workmux_hooks(&config));
    }

    #[test]
    fn test_has_workmux_hooks_present() {
        let config = json!({
            "hooks": {
                "Stop": [{
                    "hooks": [{
                        "type": "command",
                        "command": "workmux set-window-status done"
                    }]
                }]
            }
        });
        assert!(has_workmux_hooks(&config));
    }

    #[test]
    fn test_has_workmux_hooks_other_hooks_only() {
        let config = json!({
            "hooks": {
                "Stop": [{
                    "hooks": [{
                        "type": "command",
                        "command": "echo done"
                    }]
                }]
            }
        });
        assert!(!has_workmux_hooks(&config));
    }

    #[test]
    fn test_load_hooks_from_embedded() {
        let hooks = load_hooks_from_embedded().unwrap();
        let obj = hooks.as_object().unwrap();
        assert!(obj.contains_key("UserPromptSubmit"));
        assert!(obj.contains_key("Stop"));
    }

    #[test]
    fn test_check_hooks_config_empty() {
        let config = json!({});
        assert!(matches!(
            check_hooks_config(&config),
            StatusCheck::NotInstalled
        ));
    }

    #[test]
    fn test_check_hooks_config_installed() {
        let config = json!({
            "hooks": {
                "Stop": [{
                    "hooks": [{
                        "type": "command",
                        "command": "workmux set-window-status done"
                    }]
                }]
            }
        });
        assert!(matches!(
            check_hooks_config(&config),
            StatusCheck::Installed
        ));
    }

    #[test]
    fn test_merge_into_empty_config() {
        let hooks_to_add = load_hooks_from_embedded().unwrap();
        let hooks_map = hooks_to_add.as_object().unwrap();

        let mut config = json!({});
        let config_obj = config.as_object_mut().unwrap();
        config_obj.insert("hooks".to_string(), Value::Object(serde_json::Map::new()));
        let existing_hooks = config_obj
            .get_mut("hooks")
            .unwrap()
            .as_object_mut()
            .unwrap();

        for (event, hook_groups) in hooks_map {
            existing_hooks.insert(event.clone(), hook_groups.clone());
        }

        let hooks = config.get("hooks").unwrap().as_object().unwrap();
        assert_eq!(hooks.len(), 2);
    }

    #[test]
    fn test_merge_deduplicates() {
        let mut config = json!({
            "hooks": {
                "Stop": [{
                    "hooks": [{
                        "type": "command",
                        "command": "workmux set-window-status done"
                    }]
                }]
            }
        });

        let hooks_to_add = load_hooks_from_embedded().unwrap();
        let hooks_map = hooks_to_add.as_object().unwrap();

        let existing_hooks = config.get_mut("hooks").unwrap().as_object_mut().unwrap();

        for (event, hook_groups) in hooks_map {
            let new_groups = hook_groups.as_array().unwrap();
            if let Some(existing_groups) = existing_hooks.get_mut(event) {
                let arr = existing_groups.as_array_mut().unwrap();
                for group in new_groups {
                    if !arr.contains(group) {
                        arr.push(group.clone());
                    }
                }
            } else {
                existing_hooks.insert(event.clone(), hook_groups.clone());
            }
        }

        // Stop should still have exactly 1 group (not duplicated)
        let stop = config
            .get("hooks")
            .unwrap()
            .get("Stop")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(stop.len(), 1);
    }

    #[test]
    fn test_merge_preserves_existing_hooks() {
        let mut config = json!({
            "hooks": {
                "Stop": [{
                    "hooks": [{
                        "type": "command",
                        "command": "echo done"
                    }]
                }]
            }
        });

        let hooks_to_add = load_hooks_from_embedded().unwrap();
        let hooks_map = hooks_to_add.as_object().unwrap();

        let existing_hooks = config.get_mut("hooks").unwrap().as_object_mut().unwrap();

        for (event, hook_groups) in hooks_map {
            let new_groups = hook_groups.as_array().unwrap();
            if let Some(existing_groups) = existing_hooks.get_mut(event) {
                let arr = existing_groups.as_array_mut().unwrap();
                for group in new_groups {
                    if !arr.contains(group) {
                        arr.push(group.clone());
                    }
                }
            } else {
                existing_hooks.insert(event.clone(), hook_groups.clone());
            }
        }

        // Stop should have 2 groups (original echo + workmux)
        let stop = config
            .get("hooks")
            .unwrap()
            .get("Stop")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(stop.len(), 2);

        // Both events should be present
        let hooks = config.get("hooks").unwrap().as_object().unwrap();
        assert_eq!(hooks.len(), 2);
    }
}
