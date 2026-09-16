use crate::{
    adapters::{p2panda::decode_update, sqlite},
    config,
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::Path,
};
const FILES: [&str; 3] = [config::DB_FILE, "author.ed25519", "content-master.key"];
const MANIFEST: &str = "backup.json";
pub const INCOMPLETE: &str = ".recovery-incomplete";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format: String,
    version: u32,
    files: BTreeMap<String, String>,
}

pub fn backup(source: &Path, destination: &Path) -> Result<()> {
    config::load_keys(source)?;
    if source.join(INCOMPLETE).exists() {
        bail!("source recovery is incomplete");
    }
    create_destination(destination)?;
    for name in &FILES[1..] {
        copy_private(&source.join(name), &destination.join(name))?;
    }
    let db = destination.join(config::DB_FILE);
    create_private(&db)?.sync_all()?;
    sqlite::backup_database(&config::database_path(source), &db)?;
    let mut files = BTreeMap::new();
    for name in FILES {
        files.insert(name.into(), digest(&destination.join(name))?);
    }
    let manifest = Manifest {
        format: "community-stack-device-backup".into(),
        version: 1,
        files,
    };
    let mut file = create_private(&destination.join(MANIFEST))?;
    file.write_all(&serde_json::to_vec_pretty(&manifest)?)?;
    file.sync_all()?;
    verify_history(destination)?;
    finish_destination(destination)
}
pub fn restore(source: &Path, destination: &Path) -> Result<()> {
    verify(source)?;
    create_destination(destination)?;
    for name in FILES {
        copy_private(&source.join(name), &destination.join(name))?;
    }
    let manifest = read_manifest(source)?;
    verify_files(destination, &manifest)?;
    config::load_keys(destination)?;
    sqlite::verify_backup_database(&config::database_path(destination))?;
    verify_history(destination)?;
    finish_destination(destination)
}
pub fn verify(source: &Path) -> Result<()> {
    if source.join(INCOMPLETE).exists() {
        bail!("backup is incomplete");
    }
    let manifest = read_manifest(source)?;
    verify_files(source, &manifest)?;
    config::load_keys(source)?;
    sqlite::verify_backup_database(&config::database_path(source))?;
    verify_history(source)
}
fn read_manifest(source: &Path) -> Result<Manifest> {
    let path = source.join(MANIFEST);
    require_regular(&path)?;
    let mut bytes = Vec::new();
    File::open(path)?.take(16_385).read_to_end(&mut bytes)?;
    if bytes.len() > 16_384 {
        bail!("backup manifest exceeds limit");
    }
    let manifest: Manifest = serde_json::from_slice(&bytes).context("invalid backup manifest")?;
    if manifest.format != "community-stack-device-backup"
        || manifest.version != 1
        || manifest.files.len() != FILES.len()
        || FILES.iter().any(|name| !manifest.files.contains_key(*name))
    {
        bail!("unsupported backup manifest");
    }
    Ok(manifest)
}
fn verify_files(source: &Path, manifest: &Manifest) -> Result<()> {
    for name in FILES {
        if digest(&source.join(name))? != manifest.files[name] {
            bail!("backup checksum mismatch for {name}");
        }
    }
    Ok(())
}
fn create_destination(destination: &Path) -> Result<()> {
    fs::DirBuilder::new()
        .mode(0o700)
        .create(destination)
        .context("destination must not exist and its parent must exist")?;
    create_private(&destination.join(INCOMPLETE))?.sync_all()?;
    File::open(destination)?.sync_all()?;
    sync_parent(destination)
}
fn finish_destination(destination: &Path) -> Result<()> {
    File::open(destination)?.sync_all()?;
    fs::remove_file(destination.join(INCOMPLETE))?;
    File::open(destination)?.sync_all()?;
    sync_parent(destination)
}
fn sync_parent(path: &Path) -> Result<()> {
    File::open(
        path.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )?
    .sync_all()?;
    Ok(())
}
fn require_regular(path: &Path) -> Result<()> {
    if !fs::symlink_metadata(path)?.file_type().is_file() {
        bail!("backup entry must be a regular file: {}", path.display());
    }
    Ok(())
}
fn create_private(path: &Path) -> Result<File> {
    Ok(OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?)
}
fn copy_private(source: &Path, destination: &Path) -> Result<()> {
    require_regular(source)?;
    let mut input = File::open(source)?;
    let mut output = create_private(destination)?;
    std::io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    Ok(())
}
fn digest(path: &Path) -> Result<String> {
    require_regular(path)?;
    let mut input = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0_u8; 65_536];
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}
fn verify_history(source: &Path) -> Result<()> {
    let (signing, master) = config::load_keys(source)?;
    sqlite::visit_backup_updates(
        &config::database_path(source),
        |key, hash, header_bytes, body, update| {
            let (header, payload) = decode_update(header_bytes, body, &master)?;
            if header.hash().as_bytes().as_slice() != hash
                || header.verifying_key != signing.verifying_key()
                || header.extensions.app_id != key.app_id
                || header.extensions.community_id != key.community_id
                || header.extensions.document_id != key.document_id
                || payload.loro_update != update
            {
                bail!(
                    "backup canonical history, device identity, and applied updates do not agree"
                );
            }
            Ok(())
        },
    )
}
