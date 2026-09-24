//! Layered settings, like Claude Code's:
//!   ~/.ice/settings.json             user
//!   <project>/.ice/settings.json       project (commit this)
//!   <project>/.ice/settings.local.json local (personal, gitignored)
//! Later layers win for scalars; permission lists are concatenated.

use crate::tools::home_dir;
use anyhow::Result;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default)]
pub struct Settings {
    pub allow: Vec<String>,
    pub deny: Vec<String>,
    pub ask: Vec<String>,
    pub default_mode: Option<String>,
    pub model: Option<String>,
    pub env: Vec<(String, String)>,
    pub theme: Option<String>,
    pub include_co_authored_by: bool,
    pub cleanup_period_days: u64,
    pub auto_compact: bool,
    pub auto_update: bool,
}

pub fn user_dir() -> PathBuf {
    std::env::var_os("ICE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".ice"))
}

pub fn user_path() -> PathBuf {
    user_dir().join("settings.json")
}

pub fn project_path(root: &Path) -> PathBuf {
    root.join(".ice/settings.json")
}

pub fn local_path(root: &Path) -> PathBuf {
    root.join(".ice/settings.local.json")
}

fn read(p: &Path) -> Value {
    std::fs::read_to_string(p)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null)
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

impl Settings {
    pub fn load(root: &Path) -> Self {
        let mut s = Settings {
            include_co_authored_by: true,
            cleanup_period_days: 30,
            auto_compact: true,
            auto_update: true,
            ..Default::default()
        };
        for p in [user_path(), project_path(root), local_path(root)] {
            let v = read(&p);
            if v.is_null() {
                continue;
            }
            let perms = &v["permissions"];
            s.allow.extend(strs(&perms["allow"]));
            s.deny.extend(strs(&perms["deny"]));
            s.ask.extend(strs(&perms["ask"]));
            if let Some(m) = perms["defaultMode"].as_str() {
                s.default_mode = Some(m.to_string());
            }
            if let Some(m) = v["model"].as_str() {
                s.model = Some(m.to_string());
            }
            if let Some(t) = v["theme"].as_str() {
                s.theme = Some(t.to_string());
            }
            if let Some(o) = v["env"].as_object() {
                for (k, val) in o {
                    if let Some(val) = val.as_str() {
                        s.env.push((k.clone(), val.to_string()));
                    }
                }
            }
            if let Some(b) = v["includeCoAuthoredBy"].as_bool() {
                s.include_co_authored_by = b;
            }
            if let Some(n) = v["cleanupPeriodDays"].as_u64() {
                s.cleanup_period_days = n;
            }
            if let Some(b) = v["autoCompactEnabled"].as_bool() {
                s.auto_compact = b;
            }
            if let Some(b) = v["autoUpdates"].as_bool() {
                s.auto_update = b;
            }
        }
        s
    }

    /// Export `env` entries into the process (before any tool runs).
    pub fn apply_env(&self) {
        for (k, v) in &self.env {
            std::env::set_var(k, v);
        }
    }
}

/// Append an allow rule to the local (personal) project settings.
pub fn add_local_allow(root: &Path, rule: &str) -> Result<()> {
    let p = local_path(root);
    let mut v = read(&p);
    if !v.is_object() {
        v = json!({});
    }
    if !v["permissions"].is_object() {
        v["permissions"] = json!({});
    }
    let mut allow = strs(&v["permissions"]["allow"]);
    if !allow.iter().any(|r| r == rule) {
        allow.push(rule.to_string());
    }
    v["permissions"]["allow"] = json!(allow);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
        ensure_local_ignored(d);
    }
    std::fs::write(&p, serde_json::to_string_pretty(&v)? + "\n")?;
    Ok(())
}

/// Keep settings.local.json out of git, the way Claude Code does.
fn ensure_local_ignored(ice_dir: &Path) {
    let gi = ice_dir.join(".gitignore");
    let cur = std::fs::read_to_string(&gi).unwrap_or_default();
    if !cur.lines().any(|l| l.trim() == "settings.local.json") {
        let mut next = cur;
        if !next.is_empty() && !next.ends_with('\n') {
            next.push('\n');
        }
        next.push_str("settings.local.json\n");
        let _ = std::fs::write(gi, next);
    }
}

/// Set one key in the user settings file (for `ice config set`).
pub fn set_user_value(key: &str, value: Value) -> Result<()> {
    let p = user_path();
    let mut v = read(&p);
    if !v.is_object() {
        v = json!({});
    }
    v[key] = value;
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(&p, serde_json::to_string_pretty(&v)? + "\n")?;
    Ok(())
}

pub fn get_user_value(key: &str) -> Value {
    read(&user_path())[key].clone()
}

pub fn user_json() -> Value {
    read(&user_path())
}

// ─────────────────────────── credentials ───────────────────────────
//
// API keys entered through /login are kept in ~/.ice/credentials.json with
// owner-only permissions, and exported into the environment at startup when
// the variable isn't already set. They never appear in argv or logs.

pub fn credentials_path() -> PathBuf {
    user_dir().join("credentials.json")
}

pub fn load_credentials() {
    let v = read(&credentials_path());
    if let Some(o) = v.as_object() {
        for (k, val) in o {
            if k.contains(':') {
                continue; // structured records (oauth:<provider>)
            }
            if let Some(key) = val.as_str() {
                if std::env::var(k).map(|x| x.is_empty()).unwrap_or(true) {
                    std::env::set_var(k, key);
                }
            }
        }
    }
}

pub fn save_credential(env: &str, key: &str) -> Result<()> {
    let p = credentials_path();
    let mut v = read(&p);
    if !v.is_object() {
        v = json!({});
    }
    v[env] = json!(key);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    write_private(&p, &(serde_json::to_string_pretty(&v)? + "\n"))?;
    std::env::set_var(env, key);
    Ok(())
}

/// Store a structured credential (OAuth tokens) under `key`.
pub fn save_credential_value(key: &str, value: Value) -> Result<()> {
    let p = credentials_path();
    let mut v = read(&p);
    if !v.is_object() {
        v = json!({});
    }
    v[key] = value;
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    write_private(&p, &(serde_json::to_string_pretty(&v)? + "\n"))
}

pub fn credential_value(key: &str) -> Option<Value> {
    let v = read(&credentials_path())[key].clone();
    (!v.is_null()).then_some(v)
}

/// Remove saved keys; returns how many were removed.
pub fn remove_credentials() -> usize {
    let p = credentials_path();
    let n = read(&p).as_object().map(|o| o.len()).unwrap_or(0);
    let _ = std::fs::remove_file(&p);
    n
}

pub fn credential_source(env: &str) -> Option<&'static str> {
    let saved = read(&credentials_path())[env].as_str().map(String::from);
    match (std::env::var(env).ok().filter(|s| !s.is_empty()), saved) {
        (Some(v), Some(s)) if v == s => Some("saved by /login"),
        (Some(_), _) => Some("environment"),
        _ => None,
    }
}

fn write_private(p: &Path, body: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(p)?;
        f.write_all(body.as_bytes())?;
        // Tighten an existing file created with looser permissions.
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(p, body)?;
    }
    Ok(())
}

/// Masked key for display: sk-ant-…a1b2
pub fn mask(key: &str) -> String {
    let n = key.chars().count();
    if n <= 8 {
        return "••••".into();
    }
    let head: String = key.chars().take(6).collect();
    let tail: String = key.chars().skip(n - 4).collect();
    format!("{head}…{tail}")
}
