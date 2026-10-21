//! Keeps a standalone install up to date: every few hours the daemon looks for a newer release and, if there is one,
//! installs it over its own binary (with install.sh, or install.ps1 on Windows) and restarts on it. Nothing from a
//! release runs before its manifest verifies against [`RELEASE_KEY`]: the install script must have the hash the
//! manifest gives it, and so must the archive it installs. Off with `auto_update = false`; never for builds run from a
//! source tree, nor for the build the Reason CLI pins (in `~/.reason/bin`), which updates with the CLI.

use super::{Daemon, log};
use anyhow::{Result, anyhow, bail, ensure};
use std::{collections::HashMap, path::Path, sync::Arc, time::Duration};

const EVERY: Duration = Duration::from_secs(6 * 3600);

/// The Ed25519 key every release manifest is signed with. Its private half is the release workflow's
/// `RELEASE_SIGNING_KEY` secret; `release-signing.pub` is the same key, for that workflow to check what it signed.
const RELEASE_KEY: &str = "1d3e41f8f909e76d64127f15a9143d1b3e90a5111d93d2f4e8aeda89c7d9d5e7";
const MANIFEST: &str = "manifest.txt";
const MANIFEST_HEADER: &str = "reason-network release manifest v1";
const MAX_MANIFEST: usize = 16 * 1024;
const MAX_SCRIPT: usize = 256 * 1024;

impl Daemon {
    pub(super) async fn update_loop(self: Arc<Self>) {
        tokio::time::sleep(Duration::from_secs(90)).await;
        loop {
            match update().await {
                Ok(Some(v)) => {
                    log(&format!("updated to {v}; restarting on it"));
                    self.restart_soon();
                    return;
                }
                Ok(None) => {}
                Err(e) => log(&format!("checking for updates: {e:#}")),
            }
            tokio::time::sleep(EVERY).await;
        }
    }
}

/// Whether this binary may replace itself.
pub(super) fn allowed(enabled: bool) -> bool {
    enabled && !crate::util::from_source() && !crate::managed() && crate::paths::env_var_os("NO_UPDATE").is_none()
}

/// Installs the latest release if it is newer than this one; returns its version.
async fn update() -> Result<Option<String>> {
    let latest = latest_version().await?;
    if !newer(&latest, env!("CARGO_PKG_VERSION")) {
        return Ok(None);
    }
    let dir = crate::util::current_exe()?.parent().map(|d| d.to_path_buf()).unwrap_or_default();
    let exe = dir.join(format!("{}{}", crate::AGENT_NAME, std::env::consts::EXE_SUFFIX));
    let waiting = crate::migrate::waiting(&exe);
    let mut cmd = tokio::process::Command::from(installer(&dir, &latest).await?);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    // In its own process group, so a timeout kills the whole pipeline (curl and the inner sh too). (On Windows the
    // installer is one PowerShell, which `kill_on_drop` ends.)
    #[cfg(unix)]
    cmd.process_group(0);
    let child = cmd.spawn()?;
    #[cfg(unix)]
    let group = child.id();
    let out = match tokio::time::timeout(Duration::from_secs(300), child.wait_with_output()).await {
        Ok(out) => out?,
        Err(_) => {
            #[cfg(unix)]
            if let Some(pgid) = group {
                // SAFETY: signals the process group we just created.
                unsafe { libc::killpg(pgid as libc::pid_t, libc::SIGKILL) };
            }
            bail!("install timed out");
        }
    };
    if !out.status.success() {
        bail!("install failed: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    if waiting {
        crate::migrate::keep_waiting_copy(&exe);
    }
    // Restart only on a binary that really is newer (a release tagged without bumping its version must not loop).
    let installed = tokio::process::Command::new(&exe).arg("--version").kill_on_drop(true).output();
    let installed = tokio::time::timeout(Duration::from_secs(30), installed).await??;
    let installed =
        String::from_utf8_lossy(&installed.stdout).trim().rsplit(' ').next().unwrap_or_default().to_string();
    if !newer(&installed, env!("CARGO_PKG_VERSION")) {
        bail!("the latest release installed as {installed:?}, which is not newer than this one; not restarting");
    }
    Ok(Some(installed))
}

/// The install script of release `version`, ready to install it into `dir`: run only once the release's manifest
/// verified, from a private copy whose hash the manifest gives, and told to take only the archive the manifest names.
pub async fn installer(dir: &Path, version: &str) -> Result<std::process::Command> {
    let base = format!("https://github.com/{}/releases/download/v{version}", crate::REPO);
    let text = fetch(&format!("{base}/{MANIFEST}"), MAX_MANIFEST).await?;
    let sig = fetch(&format!("{base}/{MANIFEST}.sig"), MAX_MANIFEST).await?;
    let manifest = Manifest::verify(&text, &sig, &hex(RELEASE_KEY)?)?;
    ensure!(manifest.version == version, "the manifest of v{version} is for {}", manifest.version);
    let name = if cfg!(windows) { "install.ps1" } else { "install.sh" };
    let script = fetch(&format!("{base}/{name}"), MAX_SCRIPT).await?;
    manifest.check(name, &script)?;

    let work = crate::paths::home().join("update");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work)?;
    #[cfg(unix)]
    crate::util::chmod(&work, 0o700)?;
    let (manifest_path, script_path) = (work.join(MANIFEST), work.join(name));
    std::fs::write(&manifest_path, &text)?;
    std::fs::write(&script_path, &script)?;

    let mut cmd;
    if cfg!(windows) {
        cmd = std::process::Command::new("powershell");
        cmd.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"]).arg(&script_path);
    } else {
        cmd = std::process::Command::new("sh");
        cmd.arg(&script_path);
    }
    cmd.env("REASON_NETWORK_BIN_DIR", dir)
        .env("REASON_NETWORK_NO_PROMPT", "1")
        .env("REASON_NETWORK_VERSION", format!("v{version}"))
        .env("REASON_NETWORK_MANIFEST", &manifest_path)
        .env_remove("REASON_NETWORK_REPO");
    Ok(cmd)
}

/// A release's manifest: its version and the SHA-256 of each file it ships.
#[derive(Debug)]
struct Manifest {
    version: String,
    files: HashMap<String, String>,
}

impl Manifest {
    /// Reads `text` once `sig` (base64) verifies it against `key`; nothing in it counts before that.
    fn verify(text: &[u8], sig: &[u8], key: &[u8]) -> Result<Manifest> {
        let sig = data_encoding::BASE64
            .decode(String::from_utf8_lossy(sig).trim().as_bytes())
            .map_err(|_| anyhow!("the release manifest's signature isn't base64"))?;
        ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key)
            .verify(text, &sig)
            .map_err(|_| anyhow!("the release manifest's signature doesn't verify; not installing"))?;
        let text = std::str::from_utf8(text)?;
        let mut lines = text.lines();
        ensure!(lines.next() == Some(MANIFEST_HEADER), "not a release manifest");
        let version = lines.next().and_then(|l| l.strip_prefix("version ")).ok_or_else(|| anyhow!("no version"))?;
        let mut files = HashMap::new();
        for line in lines {
            let (sum, name) = line.split_once("  ").ok_or_else(|| anyhow!("bad manifest line {line:?}"))?;
            ensure!(sum.len() == 64 && sum.bytes().all(|b| b.is_ascii_hexdigit()), "bad hash for {name}");
            files.insert(name.to_string(), sum.to_ascii_lowercase());
        }
        Ok(Manifest { version: version.to_string(), files })
    }

    /// `bytes` are the file `name` this release ships.
    fn check(&self, name: &str, bytes: &[u8]) -> Result<()> {
        let want = self.files.get(name).ok_or_else(|| anyhow!("the release manifest doesn't list {name}"))?;
        let got = data_encoding::HEXLOWER.encode(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref());
        ensure!(*want == got, "{name} doesn't match the release manifest; not installing");
        Ok(())
    }
}

fn hex(s: &str) -> Result<Vec<u8>> {
    data_encoding::HEXLOWER.decode(s.as_bytes()).map_err(|_| anyhow!("bad hex"))
}

/// The body at `url`, over HTTPS only (redirects too, as GitHub's release downloads take one), at most `max` bytes.
async fn fetch(url: &str, max: usize) -> Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let mut curl = tokio::process::Command::new("curl")
        .args(["-q", "-fsSL", "--proto", "=https", "--proto-redir", "=https", "--max-redirs", "5"])
        .args(["--max-time", "120", url])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let stdout = curl.stdout.take().expect("piped");
    let mut body = Vec::new();
    tokio::time::timeout(Duration::from_secs(130), stdout.take(max as u64 + 1).read_to_end(&mut body)).await??;
    ensure!(body.len() <= max, "fetching {url}: stopped past {max} bytes");
    let out = tokio::time::timeout(Duration::from_secs(10), curl.wait_with_output()).await??;
    if !out.status.success() {
        bail!("fetching {url}: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(body)
}

/// The newest release's version, from where GitHub's "latest release" link redirects.
pub async fn latest_version() -> Result<String> {
    let repo = crate::REPO;
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let out = tokio::process::Command::new("curl")
        .args([
            "-fsSLI",
            "--max-time",
            "50",
            "-o",
            null,
            "-w",
            "%{url_effective}",
            &format!("https://github.com/{repo}/releases/latest"),
        ])
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(Duration::from_secs(60), out).await??;
    let url = String::from_utf8_lossy(&out.stdout).to_string();
    match url.rsplit_once("/tag/v") {
        Some((_, v)) if out.status.success() && !v.is_empty() => Ok(v.trim().to_string()),
        _ => bail!("no release found at {url}"),
    }
}

/// `a` is a later version than `b` (`major.minor.patch`).
fn newer(a: &str, b: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> { v.split('.').map(|p| p.parse().unwrap_or(0)).collect() };
    parse(a) > parse(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    fn signer() -> Ed25519KeyPair {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new()).unwrap();
        Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap()
    }

    fn manifest(script: &[u8]) -> String {
        let sum = data_encoding::HEXLOWER.encode(ring::digest::digest(&ring::digest::SHA256, script).as_ref());
        format!("{MANIFEST_HEADER}\nversion 1.2.3\n{sum}  install.sh\n{}  reason-network-x.tar.gz\n", "0".repeat(64))
    }

    fn sign(key: &Ed25519KeyPair, text: &str) -> Vec<u8> {
        data_encoding::BASE64.encode(key.sign(text.as_bytes()).as_ref()).into_bytes()
    }

    #[test]
    fn a_signed_manifest_vouches_for_the_files_it_lists() {
        let key = signer();
        let text = manifest(b"echo hi");
        let m = Manifest::verify(text.as_bytes(), &sign(&key, &text), key.public_key().as_ref()).unwrap();
        assert_eq!(m.version, "1.2.3");
        assert!(m.check("install.sh", b"echo hi").is_ok());
        assert!(m.check("install.sh", b"curl evil | sh").unwrap_err().to_string().contains("doesn't match"));
        assert!(m.check("install.ps1", b"").unwrap_err().to_string().contains("doesn't list"));
    }

    #[test]
    fn nothing_in_a_manifest_counts_unless_the_release_key_signed_it() {
        let (key, other) = (signer(), signer());
        let text = manifest(b"echo hi");
        let public = key.public_key().as_ref().to_vec();
        let err = |text: &str, sig: &[u8]| Manifest::verify(text.as_bytes(), sig, &public).unwrap_err().to_string();
        assert!(err(&text, &sign(&other, &text)).contains("doesn't verify"), "another key");
        let tampered = text.replace("version 1.2.3", "version 9.9.9");
        assert!(err(&tampered, &sign(&key, &text)).contains("doesn't verify"), "edited after signing");
        assert!(err(&text, b"not base64!").contains("isn't base64"));
        let other_doc = format!("something else\n{text}");
        assert!(err(&other_doc, &sign(&key, &other_doc)).contains("not a release manifest"));
    }

    #[test]
    fn the_pinned_key_is_the_one_the_release_workflow_checks() {
        let pem = include_str!("../../release-signing.pub");
        let der = data_encoding::BASE64
            .decode(pem.lines().filter(|l| !l.starts_with("-----")).collect::<String>().as_bytes())
            .unwrap();
        // An Ed25519 SubjectPublicKeyInfo ends with the 32-byte key.
        assert_eq!(der[der.len() - 32..], hex(RELEASE_KEY).unwrap()[..]);
    }

    #[test]
    fn versions_compare_by_number() {
        assert!(super::newer("0.10.0", "0.9.3"));
        assert!(super::newer("1.0.0", "0.99.99"));
        assert!(!super::newer("0.6.3", "0.6.3"));
        assert!(!super::newer("0.6.2", "0.6.10"));
    }
}
