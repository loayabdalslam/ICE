use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

pub fn resolve(root: &Path, raw: &str) -> Result<PathBuf> {
    let canon_root = root.canonicalize()?;
    let p = Path::new(raw);
    let joined = if p.is_absolute() {
        p.to_path_buf()
    } else {
        canon_root.join(p)
    };
    // Resolve existing targets (including the workspace itself) before checking
    // containment. For writes, resolve the nearest existing ancestor and append
    // only normal path components; never accept an unresolved parent traversal.
    let mut ancestor = joined.as_path();
    let mut missing = Vec::new();
    while !ancestor.exists() {
        match ancestor.components().next_back() {
            Some(std::path::Component::Normal(name)) => missing.push(name.to_os_string()),
            _ => bail!("cannot resolve workspace path: {raw}"),
        }
        ancestor = ancestor
            .parent()
            .ok_or_else(|| anyhow::anyhow!("invalid path: {raw}"))?;
    }
    let mut resolved = ancestor.canonicalize()?;
    if !resolved.starts_with(&canon_root) {
        bail!("path escapes workspace: {raw}");
    }
    for name in missing.into_iter().rev() {
        resolved.push(name);
    }
    Ok(resolved)
}

pub fn deny_command(cmd: &str) -> Result<()> {
    let l = cmd.to_ascii_lowercase();
    let banned = [
        "rm -rf /",
        "rm -rf /*",
        "mkfs",
        "dd if=",
        ":(){",
        "fork bomb",
        "shutdown",
        "reboot",
        "halt",
        "init 0",
        "chmod -r 777 /",
        "chown -r",
        "> /dev/sda",
        "curl | sh",
        "curl|sh",
        "wget | sh",
        "wget|sh",
        "drop table",
        "mkfs.ext",
    ];
    for b in banned {
        if l.contains(b) {
            bail!("denied dangerous command matching '{b}'");
        }
    }
    if l.contains("sudo ") || l.starts_with("sudo") {
        bail!("sudo is denied by the ICE sandbox");
    }
    Ok(())
}

pub fn clip(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        let mut t = s[..end].to_string();
        t.push_str("\n… [truncated]");
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn workspace_root_and_new_nested_files_resolve() {
        let root = std::env::current_dir().unwrap();
        assert_eq!(resolve(&root, ".").unwrap(), root.canonicalize().unwrap());
        assert!(resolve(&root, "target/new-nested-example/file.txt")
            .unwrap()
            .starts_with(root.canonicalize().unwrap()));
        assert!(resolve(&root, "..").is_err());
        assert!(resolve(&root, "../outside-file.txt").is_err());
    }
    #[test]
    fn unicode_output_clips_at_character_boundaries() {
        assert!(clip("🧊ICE", 3).contains("truncated"));
        assert!(clip("🧊ICE", 4).starts_with("🧊"));
    }
}
