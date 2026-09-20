//! Self-update: read releases from the ice-binaries GitHub repo, verify the
//! SHA-256, and atomically swap the running executable. A background loop keeps
//! checking so a new version is always fetched as soon as it ships.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::Sender;
use std::time::Duration;

/// Where release artifacts live. Mirrors the installer's default and can be
/// overridden for local testing.
pub fn base_url() -> String {
    std::env::var("ICE_UPDATE_BASE_URL")
        .or_else(|_| std::env::var("ICE_BASE_URL"))
        .unwrap_or_else(|_| "https://raw.githubusercontent.com/loayabdalslam/ICE/main".into())
        .trim_end_matches('/')
        .to_string()
}

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Progress and results streamed from the updater to the UI/CLI.
#[derive(Debug, Clone)]
pub enum UpdateEvent {
    Checking,
    UpToDate(String),
    Available { version: String },
    Downloading { version: String },
    Installed { version: String },
    Failed(String),
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
    let mut it = v.trim().split('.');
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

fn curl_text(url: &str) -> Result<String> {
    let out = Command::new("curl")
        .args(["-sSL", "--fail", "--max-time", "60", url])
        .output()
        .context("curl missing — required for updates")?;
    if !out.status.success() {
        bail!("fetch failed: {}", url);
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn curl_download(url: &str, dest: &Path) -> Result<()> {
    let out = Command::new("curl")
        .args([
            "-sSL",
            "--fail",
            "--max-time",
            "300",
            "-o",
            &dest.to_string_lossy(),
            url,
        ])
        .output()
        .context("curl missing — required for updates")?;
    if !out.status.success() {
        bail!(
            "download failed: {} ({})",
            url,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Read the published LATEST version string.
pub fn fetch_latest() -> Result<String> {
    let v = curl_text(&format!("{}/LATEST", base_url()))?
        .trim()
        .to_string();
    if parse_version(&v).is_none() {
        bail!("invalid remote version: {v}");
    }
    Ok(v)
}

/// Download, verify against SHA256SUMS.txt, and swap the running binary.
pub fn install(version: &str) -> Result<PathBuf> {
    if parse_version(version).is_none() {
        bail!("invalid version: {version}");
    }
    let (dir, file) = target_asset()?;
    let asset = format!("{dir}/{file}");
    let base = base_url();
    let release = format!("{base}/releases/{version}");

    // Expected checksum for our asset.
    let sums = curl_text(&format!("{release}/SHA256SUMS.txt"))?;
    let expected = sums
        .lines()
        .find_map(|line| {
            let line = line.trim();
            let (hash, path) = line.split_once("  ")?;
            if path.trim() == asset && hash.len() == 64 {
                Some(hash.to_ascii_lowercase())
            } else {
                None
            }
        })
        .with_context(|| format!("no checksum for {asset} in ICE {version}"))?;

    // Download to a temp file next to the target so the final swap is a rename.
    let exe = std::env::current_exe().context("cannot locate the running ICE binary")?;
    let install_dir = exe
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let staged = install_dir.join(format!(".ice-new-{}.tmp", nonce()));
    curl_download(&format!("{release}/{asset}"), &staged)?;

    let bytes = std::fs::read(&staged).context("reading staged download")?;
    let actual = sha256_hex(&bytes);
    if actual != expected {
        let _ = std::fs::remove_file(&staged);
        bail!("SHA-256 mismatch — refusing to install");
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&staged)?.permissions();
        perms.set_mode(0o755);
        let _ = std::fs::set_permissions(&staged, perms);
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
        std::fs::rename(target, &backup).context("moving the current binary aside")?;
        if let Err(e) = std::fs::rename(staged, target) {
            // Roll back so ICE stays runnable.
            let _ = std::fs::rename(&backup, target);
            let _ = std::fs::remove_file(staged);
            return Err(e).context("installing the new binary");
        }
        // Best-effort cleanup; the OS may still hold the old image open.
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

/// One check-and-install pass. Returns the version installed, if any.
pub fn check_and_install(send: &dyn Fn(UpdateEvent)) -> Result<Option<String>> {
    send(UpdateEvent::Checking);
    let latest = fetch_latest()?;
    if !is_newer(&latest, current_version()) {
        send(UpdateEvent::UpToDate(latest));
        return Ok(None);
    }
    send(UpdateEvent::Available {
        version: latest.clone(),
    });
    send(UpdateEvent::Downloading {
        version: latest.clone(),
    });
    install(&latest)?;
    send(UpdateEvent::Installed {
        version: latest.clone(),
    });
    Ok(Some(latest))
}

/// Background loop: check now, then every `ICE_UPDATE_INTERVAL_SECS` (default
/// 6h). Runs until the process exits or the receiver is dropped.
pub fn auto_update_loop(tx: Sender<UpdateEvent>) {
    let interval = std::env::var("ICE_UPDATE_INTERVAL_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|s| *s >= 60)
        .unwrap_or(6 * 60 * 60);
    // Small initial delay so startup stays snappy.
    std::thread::sleep(Duration::from_secs(3));
    loop {
        let send = |e: UpdateEvent| {
            let _ = tx.send(e);
        };
        if let Err(e) = check_and_install(&send) {
            if tx.send(UpdateEvent::Failed(e.to_string())).is_err() {
                return;
            }
        }
        std::thread::sleep(Duration::from_secs(interval));
    }
}

// ---------------------------------------------------------------------------
// Minimal, dependency-free SHA-256 (FIPS 180-4) for verifying downloads.
// ---------------------------------------------------------------------------

pub fn sha256_hex(data: &[u8]) -> String {
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

    let mut out = String::with_capacity(64);
    for word in h {
        out.push_str(&format!("{word:08x}"));
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
