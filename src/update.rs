//! Updates. ICE can be installed two ways and updates the same way it was
//! installed:
//! - source: the installer cloned the repository and built it with cargo;
//!   `ice update` pulls and rebuilds.
//! - binary: a prebuilt release binary; `ice update` downloads the release
//!   named in LATEST, verifies its SHA-256 and swaps the executable.
//!
//! The install record lives at ~/.ice/install.json.

use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// Where release artifacts live (raw repo contents).
pub fn base_url() -> String {
    std::env::var("ICE_UPDATE_BASE_URL")
        .unwrap_or_else(|_| "https://raw.githubusercontent.com/loayabdalslam/ICE/main".into())
        .trim_end_matches('/')
        .to_string()
}

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn install_record() -> Value {
    std::fs::read_to_string(crate::settings::user_dir().join("install.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null)
}

/// The git checkout ICE was built from, if it was installed from source.
pub fn source_dir() -> Option<PathBuf> {
    let rec = install_record();
    let dir = std::env::var("ICE_SOURCE_DIR")
        .ok()
        .map(PathBuf::from)
        .or_else(|| rec["source_dir"].as_str().map(PathBuf::from))?;
    dir.join(".git").exists().then_some(dir)
}

pub fn install_method() -> String {
    match source_dir() {
        Some(d) => format!("source ({})", d.display()),
        None => {
            if install_record()["method"].as_str() == Some("binary") {
                "binary release".into()
            } else {
                "unknown (built manually?)".into()
            }
        }
    }
}

/// The (directory, filename) of the release asset for this platform.
fn target_asset() -> Result<(&'static str, &'static str)> {
    let pair = if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        ("windows-x86_64", "ice.exe")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        ("linux-x86_64", "ice")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        ("linux-aarch64", "ice")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        ("macos-x86_64", "ice")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        ("macos-aarch64", "ice")
    } else {
        bail!("no ICE release build for this OS/architecture");
    };
    Ok(pair)
}

fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let mut it = v.trim().trim_start_matches('v').split('.');
    let a = it.next()?.parse().ok()?;
    let b = it.next()?.parse().ok()?;
    let c = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some((a, b, c))
}

/// True when `latest` is a strictly newer semver than `current`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

/// Read the published LATEST version string.
pub fn fetch_latest() -> Result<String> {
    let (_, t) = crate::http::get_text(
        &format!("{}/LATEST", base_url()),
        &[],
        Duration::from_secs(20),
        1024,
    )?;
    let v = t.trim().to_string();
    if parse_version(&v).is_none() {
        bail!("invalid remote version: {v}");
    }
    Ok(v)
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let o = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .context("git is required for source updates")?;
    if !o.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&o.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
}

fn source_ref() -> String {
    install_record()["ref"]
        .as_str()
        .unwrap_or("main")
        .to_string()
}

/// For source installs: fetch the tracked ref and say whether it moved.
fn source_behind(dir: &Path) -> Result<bool> {
    git(
        dir,
        &["fetch", "--quiet", "--depth", "1", "origin", &source_ref()],
    )?;
    let head = git(dir, &["rev-parse", "HEAD"])?;
    let remote = git(dir, &["rev-parse", "FETCH_HEAD"])?;
    Ok(head != remote)
}

/// Check out the fetched ref, rebuild, then replace the installed binary.
pub fn update_from_source(dir: &Path, log: &dyn Fn(&str)) -> Result<PathBuf> {
    log(&format!(
        "Updating source checkout in {} ({})",
        dir.display(),
        source_ref()
    ));
    git(
        dir,
        &["fetch", "--quiet", "--depth", "1", "origin", &source_ref()],
    )?;
    git(dir, &["checkout", "--quiet", "--force", "FETCH_HEAD"])?;
    log("Building (cargo build --release --locked)…");
    let status = Command::new("cargo")
        .args(["build", "--release", "--locked"])
        .current_dir(dir)
        .status()
        .context(
            "cargo is required to build ICE from source (install Rust from https://rustup.rs)",
        )?;
    if !status.success() {
        bail!("cargo build failed");
    }
    let built = dir
        .join("target/release")
        .join(if cfg!(windows) { "ice.exe" } else { "ice" });
    let exe = std::env::current_exe().context("cannot locate the running ICE binary")?;
    let target = install_record()["bin"]
        .as_str()
        .map(PathBuf::from)
        .unwrap_or(exe);
    if built != target {
        let staged = target.with_file_name(format!(".ice-new-{}.tmp", nonce()));
        std::fs::copy(&built, &staged).context("staging the new binary")?;
        swap_exe(&staged, &target)?;
    }
    Ok(target)
}

/// Download, verify against SHA256SUMS.txt, and swap the running binary.
pub fn install(version: &str) -> Result<PathBuf> {
    if parse_version(version).is_none() {
        bail!("invalid version: {version}");
    }
    let (dir, file) = target_asset()?;
    let asset = format!("{dir}/{file}");
    let release = format!("{}/releases/{version}", base_url());
    let (_, sums) = crate::http::get_text(
        &format!("{release}/SHA256SUMS.txt"),
        &[],
        Duration::from_secs(30),
        1 << 20,
    )?;
    let expected = sums
        .lines()
        .find_map(|line| {
            let (hash, path) = line.trim().split_once("  ")?;
            (path.trim() == asset && hash.len() == 64).then(|| hash.to_ascii_lowercase())
        })
        .with_context(|| format!("no checksum for {asset} in ICE {version}"))?;
    let exe = std::env::current_exe().context("cannot locate the running ICE binary")?;
    let install_dir = exe
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let bytes = crate::http::get_bytes(&format!("{release}/{asset}"), Duration::from_secs(300))?;
    if sha256_hex(&bytes) != expected {
        bail!("SHA-256 mismatch — refusing to install");
    }
    let staged = install_dir.join(format!(".ice-new-{}.tmp", nonce()));
    std::fs::write(&staged, &bytes).context("writing the staged download")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755));
    }
    swap_exe(&staged, &exe)?;
    Ok(exe)
}

/// Atomically replace `target` (the running exe) with `staged`.
fn swap_exe(staged: &Path, target: &Path) -> Result<()> {
    if cfg!(windows) {
        // A running .exe cannot be overwritten, but it can be renamed away.
        let backup = target.with_file_name(format!(".ice-old-{}.exe", nonce()));
        let _ = std::fs::remove_file(&backup);
        if target.exists() {
            std::fs::rename(target, &backup).context("moving the current binary aside")?;
        }
        if let Err(e) = std::fs::rename(staged, target) {
            let _ = std::fs::rename(&backup, target);
            let _ = std::fs::remove_file(staged);
            return Err(e).context("installing the new binary");
        }
        let _ = std::fs::remove_file(&backup);
    } else {
        std::fs::rename(staged, target).context("installing the new binary")?;
    }
    Ok(())
}

fn nonce() -> String {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{n:x}")
}

/// `ice update [--check]`.
pub fn run_cli(check_only: bool) -> Result<()> {
    println!("Current version: {}", current_version());
    if let Some(dir) = source_dir() {
        println!("Checking for updates to {}…", dir.display());
        if !source_behind(&dir)? {
            println!(
                "ICE is up to date ({}).",
                git(&dir, &["rev-parse", "--short", "HEAD"]).unwrap_or_default()
            );
            return Ok(());
        }
        println!("New commits are available on {}.", source_ref());
        if check_only {
            println!("Run `ice update` to install.");
            return Ok(());
        }
        let path = update_from_source(&dir, &|l| println!("{l}"))?;
        println!(
            "Successfully updated ICE at {}. Restart to use the new version.",
            path.display()
        );
        return Ok(());
    }
    println!("Checking for updates…");
    let latest = fetch_latest()?;
    if !is_newer(&latest, current_version()) {
        println!("ICE is up to date ({latest}).");
        return Ok(());
    }
    println!("New version available: {latest}");
    if check_only {
        println!("Run `ice update` to install it.");
        return Ok(());
    }
    let path = install(&latest)?;
    println!(
        "Successfully updated from {} to {latest} ({}). Restart to use the new version.",
        current_version(),
        path.display()
    );
    Ok(())
}

/// Quiet startup check for the REPL footer. Binary installs update in the
/// background (like Claude Code); source installs only report, because a
/// rebuild takes minutes of CPU.
pub fn background_check() -> Option<String> {
    std::thread::sleep(Duration::from_secs(2));
    if let Some(dir) = source_dir() {
        let behind = source_behind(&dir).ok()?;
        return behind.then(|| "Update available · run `ice update`".to_string());
    }
    let latest = fetch_latest().ok()?;
    if !is_newer(&latest, current_version()) {
        return None;
    }
    if install_record()["method"].as_str() == Some("binary") && install(&latest).is_ok() {
        return Some(format!("✓ Updated to {latest} · restart to apply"));
    }
    Some(format!("Update {latest} available · run `ice update`"))
}

// ---------------------------------------------------------------------------
// Minimal, dependency-free SHA-256 (FIPS 180-4) for verifying downloads.
// ---------------------------------------------------------------------------

pub fn sha256_hex(data: &[u8]) -> String {
    sha256(data).iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for block in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in w.iter_mut().take(16).enumerate() {
            *word = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ ((!v[4]) & v[6]);
            let t1 = v[7]
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v[7] = v[6];
            v[6] = v[5];
            v[5] = v[4];
            v[4] = v[3].wrapping_add(t1);
            v[3] = v[2];
            v[2] = v[1];
            v[1] = v[0];
            v[0] = t1.wrapping_add(t2);
        }
        for (hi, vi) in h.iter_mut().zip(v.iter()) {
            *hi = hi.wrapping_add(*vi);
        }
    }

    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"The quick brown fox jumps over the lazy dog"),
            "d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592"
        );
    }

    #[test]
    fn version_ordering() {
        assert!(is_newer("0.3.1", "0.3.0"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.3.0", "0.3.0"));
        assert!(!is_newer("0.2.9", "0.3.0"));
        assert!(!is_newer("garbage", "0.3.0"));
    }
}
