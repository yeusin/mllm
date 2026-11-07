use anyhow::{bail, Context, Result};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

const REPO: &str = "yeusin/mllm";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone)]
pub struct UpdateOptions {
    pub check_only: bool,
    pub version: String,
    pub force: bool,
    pub dry_run: bool,
    pub target_path: Option<PathBuf>,
}

impl Default for UpdateOptions {
    fn default() -> Self {
        Self {
            check_only: false,
            version: "latest".to_string(),
            force: false,
            dry_run: false,
            target_path: None,
        }
    }
}

/// Detect host platform target triple matching release assets
pub fn detect_target_triple() -> Result<&'static str> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    match (os, arch) {
        ("linux", "x86_64") => {
            if cfg!(target_env = "musl") {
                Ok("x86_64-unknown-linux-musl")
            } else {
                Ok("x86_64-unknown-linux-gnu")
            }
        }
        ("linux", "aarch64") => {
            if cfg!(target_env = "musl") {
                Ok("aarch64-unknown-linux-musl")
            } else {
                Ok("aarch64-unknown-linux-gnu")
            }
        }
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        _ => bail!(
            "Unsupported platform: OS='{}', ARCH='{}'. Officially supported: Linux and macOS (x86_64, aarch64).",
            os,
            arch
        ),
    }
}

/// Normalize version string by stripping leading 'v' or whitespace
pub fn normalize_version(version: &str) -> String {
    let v = version.trim();
    if let Some(stripped) = v.strip_prefix('v') {
        stripped.to_string()
    } else {
        v.to_string()
    }
}

/// Validate that a version string contains only safe characters
pub fn validate_version_string(version: &str) -> Result<()> {
    let trimmed = version.trim();
    if trimmed.is_empty() {
        bail!("Version string cannot be empty");
    }
    if !trimmed
        .chars()
        .all(|c| c.is_alphanumeric() || c == '.' || c == '-' || c == '+' || c == 'v' || c == '_')
    {
        bail!("Version string '{}' contains invalid characters", version);
    }
    Ok(())
}

/// Parse basic semver numeric components (major, minor, patch) for downgrade detection
fn parse_semver_tuple(version: &str) -> Result<(u64, u64, u64)> {
    let norm = normalize_version(version);
    let mut parts = norm.split('.');
    let major = parts
        .next()
        .context("Missing major version")?
        .parse::<u64>()
        .context("Invalid major version")?;
    let minor = parts.next().unwrap_or("0").parse::<u64>().unwrap_or(0);
    let patch = parts.next().unwrap_or("0").parse::<u64>().unwrap_or(0);
    Ok((major, minor, patch))
}

/// Resolves the target executable location, following symlinks if existing
pub fn resolve_target_executable(target_override: Option<&Path>) -> Result<PathBuf> {
    if let Some(target) = target_override {
        if let Ok(meta) = fs::symlink_metadata(target) {
            if meta.is_dir() {
                bail!(
                    "Specified target '{}' is a directory; expected an executable file path",
                    target.display()
                );
            }
            if meta.is_symlink() {
                // If symlink exists, resolve to its link target path so we write to the target instead of clobbering the symlink pointer
                if let Ok(link_dest) = fs::read_link(target) {
                    let resolved = if link_dest.is_absolute() {
                        link_dest
                    } else if let Some(parent) = target.parent() {
                        parent.join(link_dest)
                    } else {
                        link_dest
                    };
                    return Ok(resolved);
                }
            }
        }
        if target.exists() {
            let canonical = fs::canonicalize(target)
                .with_context(|| format!("Failed to canonicalize target path '{}'", target.display()))?;
            if canonical.is_dir() {
                bail!(
                    "Specified target '{}' resolves to a directory; expected an executable file path",
                    target.display()
                );
            }
            Ok(canonical)
        } else {
            Ok(target.to_path_buf())
        }
    } else {
        let exe = std::env::current_exe().context("Failed to determine current executable path")?;
        match fs::canonicalize(&exe) {
            Ok(canonical) => {
                if canonical.is_dir() {
                    bail!(
                        "Current executable path '{}' resolves to a directory",
                        canonical.display()
                    );
                }
                Ok(canonical)
            }
            Err(_) => Ok(exe),
        }
    }
}

/// Verify that the downloaded archive matches the SHA-256 hash in the checksum asset
pub fn verify_archive_checksum(archive_path: &Path, checksum_path: &Path) -> Result<String> {
    if !checksum_path.exists() {
        bail!(
            "Missing SHA-256 checksum asset at '{}'. Aborting to ensure payload integrity.",
            checksum_path.display()
        );
    }

    let expected_content = fs::read_to_string(checksum_path)
        .with_context(|| format!("Failed to read checksum asset at '{}'", checksum_path.display()))?;
    let expected_hash = expected_content
        .split_whitespace()
        .next()
        .context("Checksum asset was empty or improperly formatted")?;

    let actual_hash = compute_file_sha256(archive_path)?;
    if !actual_hash.eq_ignore_ascii_case(expected_hash) {
        bail!(
            "SHA-256 checksum verification failed!\nExpected: {}\nActual:   {}",
            expected_hash,
            actual_hash
        );
    }

    Ok(actual_hash)
}

/// Verify that the extracted payload is a regular file (not a directory or symlink)
pub fn verify_extracted_payload(payload_path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(payload_path)
        .with_context(|| format!("Extracted archive did not contain expected binary '{}'", payload_path.display()))?;
    if !meta.is_file() {
        bail!("Extracted payload '{}' is not a regular file (symlink or directory detected)", payload_path.display());
    }
    Ok(())
}

/// Query GitHub API or releases URL to determine latest release tag
pub fn fetch_latest_release_tag(repo: &str) -> Result<String> {
    // 1. Try GitHub Releases API
    let api_url = format!("https://api.github.com/repos/{}/releases/latest", repo);
    let output = Command::new("curl")
        .args([
            "-fsSL",
            "-H",
            "User-Agent: mllm-updater",
            "-H",
            "Accept: application/vnd.github.v3+json",
            &api_url,
        ])
        .output();

    if let Ok(out) = output {
        if out.status.success() {
            if let Ok(body) = String::from_utf8(out.stdout) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
                    if let Some(tag) = v.get("tag_name").and_then(|t| t.as_str()) {
                        return Ok(tag.to_string());
                    }
                }
            }
        }
    }

    // 2. Fallback: Query redirect location from release/latest
    let redirect_url = format!("https://github.com/{}/releases/latest", repo);
    let output = Command::new("curl")
        .args(["-fsSLI", "-o", "/dev/null", "-w", "%{url_effective}", &redirect_url])
        .output();

    if let Ok(out) = output {
        if out.status.success() {
            if let Ok(effective_url) = String::from_utf8(out.stdout) {
                let trimmed = effective_url.trim();
                if let Some(idx) = trimmed.rfind("/tag/") {
                    let tag = &trimmed[idx + 5..];
                    if !tag.is_empty() {
                        return Ok(tag.to_string());
                    }
                }
            }
        }
    }

    bail!("Failed to query latest release tag from GitHub for repository '{}'", repo)
}

/// Compute SHA-256 hash using system tools (sha256sum, shasum, openssl)
pub fn compute_file_sha256(path: &Path) -> Result<String> {
    if let Ok(out) = Command::new("sha256sum").arg(path).output() {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            if let Some(hash) = s.split_whitespace().next() {
                return Ok(hash.to_lowercase());
            }
        }
    }

    if let Ok(out) = Command::new("shasum").args(["-a", "256"]).arg(path).output() {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            if let Some(hash) = s.split_whitespace().next() {
                return Ok(hash.to_lowercase());
            }
        }
    }

    if let Ok(out) = Command::new("openssl").args(["dgst", "-sha256"]).arg(path).output() {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            if let Some(hash) = s.split_whitespace().last() {
                return Ok(hash.to_lowercase());
            }
        }
    }

    bail!("No SHA-256 utility (sha256sum, shasum, openssl) found on system")
}

/// Atomically replace target executable with new binary
pub fn atomic_replace(new_binary: &Path, target_binary: &Path) -> Result<()> {
    let target_dir = target_binary
        .parent()
        .context("Target binary has no parent directory")?;

    if !target_dir.exists() {
        fs::create_dir_all(target_dir)
            .with_context(|| format!("Failed to create target directory '{}'", target_dir.display()))?;
    }

    // Create unique temporary file name in the same directory to guarantee atomic rename does NOT cross a filesystem boundary
    let unique_id = uuid::Uuid::new_v4();
    let temp_dest = target_dir.join(format!(".mllm-update-{}.tmp", unique_id));

    // Copy new binary into temp destination
    if let Err(e) = fs::copy(new_binary, &temp_dest) {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            bail!(
                "Permission denied writing to '{}'. Please rerun with elevated privileges: 'sudo mllm update' or install to a user-writable path such as '~/.local/bin/mllm'.",
                target_dir.display()
            );
        } else {
            return Err(e).with_context(|| format!("Failed to stage update binary at '{}'", temp_dest.display()));
        }
    }

    // Set executable permissions (0755)
    #[cfg(unix)]
    {
        let perms = fs::Permissions::from_mode(0o755);
        if let Err(e) = fs::set_permissions(&temp_dest, perms) {
            let _ = fs::remove_file(&temp_dest);
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                bail!(
                    "Permission denied setting executable permissions on '{}'. Please rerun with elevated privileges: 'sudo mllm update'.",
                    temp_dest.display()
                );
            } else {
                bail!("Failed to set executable permissions on staged binary: {}", e);
            }
        }
    }

    // Guarantee data persistence before rename to prevent truncated or zero-length binary on crash/power-loss
    {
        let staged_file = fs::File::open(&temp_dest)
            .with_context(|| format!("Failed to open staged binary '{}' for synchronization", temp_dest.display()))?;
        staged_file
            .sync_all()
            .with_context(|| format!("Failed to sync staged binary '{}' to persistent storage", temp_dest.display()))?;
    }

    // Atomic rename over the target executable
    if let Err(e) = fs::rename(&temp_dest, target_binary) {
        let _ = fs::remove_file(&temp_dest);
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            bail!(
                "Permission denied replacing '{}'. Please rerun with elevated privileges: 'sudo mllm update'.",
                target_binary.display()
            );
        } else {
            return Err(e).with_context(|| format!("Failed to atomically replace '{}'", target_binary.display()));
        }
    }

    // Best-effort directory metadata synchronization on Unix
    #[cfg(unix)]
    {
        if let Ok(dir_file) = fs::File::open(target_dir) {
            let _ = dir_file.sync_all();
        }
    }

    Ok(())
}

/// Execute mllm update command lifecycle
pub fn run_update<W: Write>(opts: &UpdateOptions, out: &mut W) -> Result<()> {
    validate_version_string(&opts.version)?;
    let target_bin = resolve_target_executable(opts.target_path.as_deref())?;
    let target_triple = detect_target_triple()?;

    writeln!(out, "Current mllm version: v{}", CURRENT_VERSION)?;
    writeln!(out, "Target executable:    {}", target_bin.display())?;
    writeln!(out, "Platform triple:      {}", target_triple)?;

    let is_latest = opts.version.eq_ignore_ascii_case("latest");
    let target_tag = if is_latest {
        fetch_latest_release_tag(REPO)
            .with_context(|| format!("Failed to query latest release tag from GitHub for repository '{}'", REPO))?
    } else if opts.version.starts_with('v') {
        opts.version.clone()
    } else {
        format!("v{}", opts.version)
    };

    let target_version_norm = normalize_version(&target_tag);
    let current_version_norm = normalize_version(CURRENT_VERSION);

    writeln!(out, "Target release:       {}", target_tag)?;

    // Surface explicit downgrade notice if target version is older than running binary
    if let (Ok(curr_ver), Ok(target_ver)) = (
        parse_semver_tuple(CURRENT_VERSION),
        parse_semver_tuple(&target_version_norm),
    ) {
        if target_ver < curr_ver {
            writeln!(
                out,
                "Notice: Target version {} is older than current version v{} (downgrade).",
                target_tag, CURRENT_VERSION
            )?;
        }
    }

    let is_up_to_date = target_version_norm == current_version_norm;

    if opts.check_only {
        if is_up_to_date {
            writeln!(out, "\nStatus: mllm is up to date (v{}).", CURRENT_VERSION)?;
        } else {
            writeln!(
                out,
                "\nStatus: Update available: v{} -> {}! Run 'mllm update' to install.",
                CURRENT_VERSION, target_tag
            )?;
        }
        return Ok(());
    }

    if is_up_to_date && !opts.force {
        writeln!(
            out,
            "\nmllm is already up to date (v{}). Use --force to reinstall.",
            CURRENT_VERSION
        )?;
        return Ok(());
    }

    if opts.dry_run {
        writeln!(out, "\n[Dry Run] Planned actions:")?;
        writeln!(
            out,
            "1. Download https://github.com/{}/releases/download/{}/mllm-{}.tar.gz",
            REPO, target_tag, target_triple
        )?;
        writeln!(
            out,
            "2. Verify SHA-256 checksum from https://github.com/{}/releases/download/{}/mllm-{}.sha256",
            REPO, target_tag, target_triple
        )?;
        writeln!(out, "3. Extract release binary 'mllm'")?;
        writeln!(out, "4. Atomically replace '{}'", target_bin.display())?;
        writeln!(out, "\n[Dry Run] Simulation complete. No files were modified.")?;
        return Ok(());
    }

    // Temporary staging directory with restricted permissions (0700)
    let temp_dir = std::env::temp_dir().join(format!("mllm_update_{}", uuid::Uuid::new_v4()));
    #[cfg(unix)]
    {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        builder.mode(0o700);
        builder.create(&temp_dir)
            .with_context(|| format!("Failed to create secure temporary directory '{}'", temp_dir.display()))?;
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(&temp_dir)
            .with_context(|| format!("Failed to create temporary directory '{}'", temp_dir.display()))?;
    }

    // RAII guard for cleanup
    struct TempDirGuard(PathBuf);
    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _guard = TempDirGuard(temp_dir.clone());

    let archive_name = format!("mllm-{}.tar.gz", target_triple);
    let archive_path = temp_dir.join(&archive_name);
    let checksum_name = format!("mllm-{}.sha256", target_triple);
    let checksum_path = temp_dir.join(&checksum_name);

    let base_url = format!("https://github.com/{}/releases/download/{}", REPO, target_tag);
    let archive_url = format!("{}/{}", base_url, archive_name);
    let checksum_url = format!("{}/{}", base_url, checksum_name);

    writeln!(out, "\nDownloading {} ...", archive_url)?;
    let dl_status = Command::new("curl")
        .args(["-fsSL", &archive_url, "-o"])
        .arg(&archive_path)
        .status();

    let download_success = match dl_status {
        Ok(s) if s.success() => true,
        _ => {
            // Try wget fallback
            Command::new("wget")
                .args(["-q", &archive_url, "-O"])
                .arg(&archive_path)
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
    };

    if !download_success {
        bail!("Failed to download release archive from '{}'", archive_url);
    }

    // Download checksum file (mandatory fail-closed verification)
    writeln!(out, "Downloading checksum asset {} ...", checksum_url)?;
    let cs_status = Command::new("curl")
        .args(["-fsSL", &checksum_url, "-o"])
        .arg(&checksum_path)
        .status();

    let cs_success = match cs_status {
        Ok(s) if s.success() => true,
        _ => {
            Command::new("wget")
                .args(["-q", &checksum_url, "-O"])
                .arg(&checksum_path)
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
    };

    if !cs_success || !checksum_path.exists() {
        bail!(
            "Failed to download mandatory SHA-256 checksum asset from '{}'. Aborting to ensure payload integrity.",
            checksum_url
        );
    }

    writeln!(out, "Verifying SHA-256 checksum...")?;
    let actual_hash = verify_archive_checksum(&archive_path, &checksum_path)?;
    writeln!(out, "Checksum verified: SHA-256 matches ({})", actual_hash)?;

    writeln!(out, "Extracting binary...")?;
    let tar_status = Command::new("tar")
        .args(["-xzf"])
        .arg(&archive_path)
        .args(["--no-same-owner", "--no-same-permissions"])
        .args(["-C"])
        .arg(&temp_dir)
        .status()
        .context("Failed to execute tar for archive extraction")?;

    if !tar_status.success() {
        bail!("Extraction of '{}' failed", archive_path.display());
    }

    let extracted_bin = temp_dir.join("mllm");
    verify_extracted_payload(&extracted_bin)?;

    // Stage atomic replacement
    writeln!(out, "Installing to '{}'...", target_bin.display())?;
    atomic_replace(&extracted_bin, &target_bin)?;

    writeln!(
        out,
        "\nSuccessfully updated mllm to {} at '{}'!",
        target_tag,
        target_bin.display()
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_target_triple() {
        let triple = detect_target_triple();
        assert!(triple.is_ok());
        let t = triple.unwrap();
        assert!(t.contains("linux") || t.contains("darwin"));
    }

    #[test]
    fn test_normalize_version() {
        assert_eq!(normalize_version("v0.1.0"), "0.1.0");
        assert_eq!(normalize_version("0.1.0"), "0.1.0");
        assert_eq!(normalize_version(" v1.2.3 "), "1.2.3");
    }

    #[test]
    fn test_dry_run_output() {
        let opts = UpdateOptions {
            dry_run: true,
            version: "v9.9.9".to_string(),
            ..Default::default()
        };
        let mut out = Vec::new();
        let res = run_update(&opts, &mut out);
        assert!(res.is_ok());
        let output = String::from_utf8(out).unwrap();
        assert!(output.contains("[Dry Run] Planned actions:"));
        assert!(output.contains("Simulation complete"));
    }

    #[test]
    fn test_check_up_to_date_output() {
        let opts = UpdateOptions {
            check_only: true,
            version: format!("v{}", CURRENT_VERSION),
            ..Default::default()
        };
        let mut out = Vec::new();
        let res = run_update(&opts, &mut out);
        assert!(res.is_ok());
        let output = String::from_utf8(out).unwrap();
        assert!(output.contains("is up to date"));
    }

    #[test]
    fn test_atomic_replace_and_permissions() {
        let temp_dir = std::env::temp_dir().join(format!("test_atomic_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&temp_dir).unwrap();

        let src = temp_dir.join("src_mllm");
        let dest = temp_dir.join("dest_mllm");

        fs::write(&src, b"#!/bin/sh\necho test\n").unwrap();
        fs::write(&dest, b"#!/bin/sh\necho old\n").unwrap();

        atomic_replace(&src, &dest).expect("Atomic replace should succeed in temp dir");

        let content = fs::read_to_string(&dest).unwrap();
        assert_eq!(content, "#!/bin/sh\necho test\n");

        #[cfg(unix)]
        {
            let meta = fs::metadata(&dest).unwrap();
            let mode = meta.permissions().mode();
            assert_eq!(mode & 0o755, 0o755);
        }

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_validate_version_string() {
        assert!(validate_version_string("0.1.0").is_ok());
        assert!(validate_version_string("v1.2.3-beta.1").is_ok());
        assert!(validate_version_string("").is_err());
        assert!(validate_version_string("0.1.0; rm -rf /").is_err());
        assert!(validate_version_string("v1.0.0/attack").is_err());
    }

    #[test]
    fn test_target_directory_rejected_early() {
        let temp_dir = std::env::temp_dir().join(format!("test_target_dir_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&temp_dir).unwrap();

        let res = resolve_target_executable(Some(&temp_dir));
        assert!(res.is_err());
        let err_str = format!("{:#}", res.err().unwrap());
        assert!(err_str.contains("is a directory"));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_parse_semver_tuple_and_downgrade_notice() {
        assert_eq!(parse_semver_tuple("0.1.0").unwrap(), (0, 1, 0));
        assert_eq!(parse_semver_tuple("v2.3.4").unwrap(), (2, 3, 4));

        let opts = UpdateOptions {
            dry_run: true,
            version: "v0.0.1".to_string(),
            ..Default::default()
        };
        let mut out = Vec::new();
        let res = run_update(&opts, &mut out);
        assert!(res.is_ok());
        let output = String::from_utf8(out).unwrap();
        assert!(output.contains("downgrade"));
    }

    #[test]
    fn test_checksum_verification_fail_paths() {
        let temp_dir = std::env::temp_dir().join(format!("test_cs_fail_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&temp_dir).unwrap();

        let archive = temp_dir.join("test.tar.gz");
        fs::write(&archive, b"dummy archive content").unwrap();

        let valid_hash = compute_file_sha256(&archive).unwrap();

        // 1. Valid checksum passes
        let cs_valid = temp_dir.join("valid.sha256");
        fs::write(&cs_valid, format!("{}  test.tar.gz\n", valid_hash)).unwrap();
        let res_valid = verify_archive_checksum(&archive, &cs_valid);
        assert!(res_valid.is_ok());

        // 2. Mismatch fails closed
        let cs_mismatch = temp_dir.join("mismatch.sha256");
        fs::write(&cs_mismatch, "0000000000000000000000000000000000000000000000000000000000000000  test.tar.gz\n").unwrap();
        let res_mismatch = verify_archive_checksum(&archive, &cs_mismatch);
        assert!(res_mismatch.is_err());
        assert!(format!("{:#}", res_mismatch.err().unwrap()).contains("checksum verification failed"));

        // 3. Missing checksum file fails closed
        let cs_missing = temp_dir.join("missing.sha256");
        let res_missing = verify_archive_checksum(&archive, &cs_missing);
        assert!(res_missing.is_err());
        assert!(format!("{:#}", res_missing.err().unwrap()).contains("Missing SHA-256 checksum asset"));

        // 4. Empty checksum file fails closed
        let cs_empty = temp_dir.join("empty.sha256");
        fs::write(&cs_empty, b"   \n").unwrap();
        let res_empty = verify_archive_checksum(&archive, &cs_empty);
        assert!(res_empty.is_err());
        assert!(format!("{:#}", res_empty.err().unwrap()).contains("empty or improperly formatted"));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_extracted_payload_validation_fail_paths() {
        let temp_dir = std::env::temp_dir().join(format!("test_payload_fail_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&temp_dir).unwrap();

        // 1. Regular file passes
        let regular = temp_dir.join("mllm_reg");
        fs::write(&regular, b"binary data").unwrap();
        assert!(verify_extracted_payload(&regular).is_ok());

        // 2. Directory payload rejected
        let dir_payload = temp_dir.join("mllm_dir");
        fs::create_dir(&dir_payload).unwrap();
        let res_dir = verify_extracted_payload(&dir_payload);
        assert!(res_dir.is_err());
        assert!(format!("{:#}", res_dir.err().unwrap()).contains("not a regular file"));

        // 3. Symlink payload rejected
        #[cfg(unix)]
        {
            let symlink_payload = temp_dir.join("mllm_sym");
            std::os::unix::fs::symlink(&regular, &symlink_payload).unwrap();
            let res_sym = verify_extracted_payload(&symlink_payload);
            assert!(res_sym.is_err());
            assert!(format!("{:#}", res_sym.err().unwrap()).contains("not a regular file"));
        }

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_broken_symlink_target_handling() {
        #[cfg(unix)]
        {
            let temp_dir = std::env::temp_dir().join(format!("test_symlink_targ_{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&temp_dir).unwrap();

            let target_dest = temp_dir.join("dest_binary");
            let symlink_ptr = temp_dir.join("mllm_link");

            // Create broken / dangling symlink pointing to nonexistent dest_binary
            std::os::unix::fs::symlink(&target_dest, &symlink_ptr).unwrap();
            assert!(!target_dest.exists());
            assert!(symlink_ptr.is_symlink());

            // resolve_target_executable should resolve to target_dest rather than clobbering symlink_ptr
            let resolved = resolve_target_executable(Some(&symlink_ptr)).unwrap();
            assert_eq!(resolved, target_dest);

            let _ = fs::remove_dir_all(&temp_dir);
        }
    }
}
