use std::{
    fs::OpenOptions,
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use p2panda_core::SigningKey;

pub const DB_FILE: &str = "community.sqlite3";
pub const SOCKET_FILE: &str = "community.sock";
const SIGNING_KEY_FILE: &str = "author.ed25519";
const MASTER_KEY_FILE: &str = "content-master.key";

pub fn initialize_data_dir(data_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(data_dir)?;
    std::fs::set_permissions(data_dir, std::fs::Permissions::from_mode(0o700))?;
    ensure_secret(&data_dir.join(SIGNING_KEY_FILE), 32)?;
    ensure_secret(&data_dir.join(MASTER_KEY_FILE), 32)?;
    Ok(())
}

pub fn load_keys(data_dir: &Path) -> Result<(SigningKey, [u8; 32])> {
    let signing = read_secret(&data_dir.join(SIGNING_KEY_FILE), 32)?;
    let master = read_secret(&data_dir.join(MASTER_KEY_FILE), 32)?;
    Ok((
        SigningKey::try_from(signing.as_slice())?,
        master
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid master key length"))?,
    ))
}

pub fn generate_token() -> Result<([u8; 32], String)> {
    let mut token = [0_u8; 32];
    getrandom::fill(&mut token)
        .map_err(|error| anyhow::anyhow!("OS random source failed: {error}"))?;
    Ok((*blake3::hash(&token).as_bytes(), hex::encode(token)))
}

pub fn database_path(data_dir: &Path) -> PathBuf {
    data_dir.join(DB_FILE)
}
pub fn socket_path(data_dir: &Path) -> PathBuf {
    data_dir.join(SOCKET_FILE)
}

fn ensure_secret(path: &Path, length: usize) -> Result<()> {
    if path.exists() {
        let _ = read_secret(path, length)?;
        return Ok(());
    }
    let mut bytes = vec![0_u8; length];
    getrandom::fill(&mut bytes)
        .map_err(|error| anyhow::anyhow!("OS random source failed: {error}"))?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut file = options
        .open(path)
        .with_context(|| format!("creating {}", path.display()))?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

fn read_secret(path: &Path, expected_length: usize) -> Result<Vec<u8>> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("reading metadata for {}", path.display()))?;
    if metadata.file_type().is_symlink() {
        bail!("secret path {} must not be a symlink", path.display());
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        bail!(
            "secret path {} is accessible by group or other users",
            path.display()
        );
    }
    let mut bytes = Vec::new();
    OpenOptions::new()
        .read(true)
        .open(path)?
        .take((expected_length + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() != expected_length {
        bail!("secret path {} has invalid length", path.display());
    }
    Ok(bytes)
}
