use std::{
    io::ErrorKind,
    os::unix::net::UnixListener as StdUnixListener,
    os::{fd::AsRawFd, unix::fs::PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, bail};
#[cfg(target_os = "linux")]
use rustix::fs::{ResolveFlags, openat2};
use rustix::{
    fs::{FileType, Mode, OFlags, fstat, statat, unlinkat},
    process::geteuid,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::Semaphore,
    time::{Duration, timeout},
};
use tracing::{info, warn};

use crate::application::CommunityCore;

const API_VERSION: u16 = 1;
const MAX_REQUEST_BYTES: usize = 1_048_576;
const MAX_RESPONSE_BYTES: usize = 4_194_304;
const MAX_REQUESTS_PER_CONNECTION: usize = 256;
const IO_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Deserialize)]
struct ApiRequest {
    v: u16,
    id: String,
    #[serde(default)]
    token: Option<String>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct ApiResponse {
    v: u16,
    id: String,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ApiErrorBody>,
}

#[derive(Debug, Serialize)]
struct ApiErrorBody {
    code: String,
    message: String,
    retryable: bool,
}

impl ApiResponse {
    fn success(id: String, result: Value) -> Self {
        Self {
            v: API_VERSION,
            id,
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    fn failure(
        id: String,
        code: impl Into<String>,
        message: impl Into<String>,
        retryable: bool,
    ) -> Self {
        Self {
            v: API_VERSION,
            id,
            ok: false,
            result: None,
            error: Some(ApiErrorBody {
                code: code.into(),
                message: message.into(),
                retryable,
            }),
        }
    }
}

pub async fn serve(socket_path: &Path, core: CommunityCore) -> Result<()> {
    let binding = prepare_socket(socket_path)?;
    let listener = binding.bind()?;
    info!(path = %socket_path.display(), "local API ready");
    let connections = Arc::new(Semaphore::new(128));

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                let Ok(permit) = Arc::clone(&connections).try_acquire_owned() else {
                    warn!("local API connection limit reached");
                    drop(stream);
                    continue;
                };
                let core = core.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    if let Err(error) = handle_connection(stream, core).await {
                        warn!(error = %error, "local API connection closed with an error");
                    }
                });
            }
            signal = tokio::signal::ctrl_c() => {
                signal?;
                info!("shutdown requested");
                break;
            }
        }
    }
    drop(listener);
    binding.remove_socket()?;
    Ok(())
}

#[derive(Debug)]
struct SecureSocketBinding {
    parent: rustix::fd::OwnedFd,
    name: String,
    bind_path: PathBuf,
}

impl SecureSocketBinding {
    fn bind(&self) -> Result<UnixListener> {
        // Unix pathname sockets have no bindat(2). openat2 fixes and validates
        // every existing intermediate component before this bind; the final
        // parent is private and owned by the service user, so an unprivileged
        // peer cannot replace it between these operations.
        let listener = StdUnixListener::bind(&self.bind_path)
            .with_context(|| format!("binding local API socket entry {}", self.name))?;
        listener.set_nonblocking(true)?;
        std::fs::set_permissions(&self.bind_path, std::fs::Permissions::from_mode(0o600))?;
        Ok(UnixListener::from_std(listener)?)
    }

    fn remove_socket(&self) -> Result<()> {
        match unlinkat(&self.parent, &self.name, rustix::fs::AtFlags::empty()) {
            Ok(()) => Ok(()),
            Err(error) if error == rustix::io::Errno::NOENT => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

#[cfg(target_os = "linux")]
fn prepare_socket(path: &Path) -> Result<SecureSocketBinding> {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty() && *value != "." && *value != "..")
        .context("socket path must have a UTF-8 final component")?;
    if name.contains('/') {
        bail!("socket path final component must not contain a separator");
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent_fd = openat2(
        rustix::fs::CWD,
        parent,
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::NO_SYMLINKS,
    )
    .with_context(|| format!("securely opening socket parent {}", parent.display()))?;
    validate_socket_parent(&parent_fd, parent)?;

    match statat(&parent_fd, name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW) {
        Ok(metadata) if FileType::from_raw_mode(metadata.st_mode).is_socket() => {
            unlinkat(&parent_fd, name, rustix::fs::AtFlags::empty())?;
        }
        Ok(metadata) => {
            bail!(
                "refusing to replace non-socket path {} ({} is present)",
                path.display(),
                match FileType::from_raw_mode(metadata.st_mode) {
                    FileType::Symlink => "symlink",
                    _ => "another filesystem object",
                }
            );
        }
        Err(error) if error == rustix::io::Errno::NOENT => {}
        Err(error) => return Err(error.into()),
    }

    if path.as_os_str().len() > 107 {
        bail!("socket path is too long for Unix pathname binding");
    }
    let bind_path = PathBuf::from(format!("/proc/self/fd/{}/{}", parent_fd.as_raw_fd(), name));
    Ok(SecureSocketBinding {
        parent: parent_fd,
        name: name.to_owned(),
        bind_path,
    })
}

#[cfg(not(target_os = "linux"))]
fn prepare_socket(_path: &Path) -> Result<SecureSocketBinding> {
    bail!("secure socket binding is unavailable on this platform")
}

fn validate_socket_parent(parent: &rustix::fd::OwnedFd, display_path: &Path) -> Result<()> {
    let metadata = fstat(parent)?;
    if !FileType::from_raw_mode(metadata.st_mode).is_dir() {
        bail!(
            "socket parent {} must be a directory",
            display_path.display()
        );
    }
    if metadata.st_uid != geteuid().as_raw() {
        bail!(
            "socket parent {} is not owned by the effective user",
            display_path.display()
        );
    }
    if metadata.st_mode & 0o077 != 0 || metadata.st_mode & 0o700 != 0o700 {
        bail!(
            "socket parent {} must be private (mode 0700)",
            display_path.display()
        );
    }
    Ok(())
}

async fn handle_connection(mut stream: UnixStream, core: CommunityCore) -> Result<()> {
    for _ in 0..MAX_REQUESTS_PER_CONNECTION {
        let read_length = timeout(IO_TIMEOUT, stream.read_u32())
            .await
            .context("local API connection timed out waiting for a frame")?;
        let length = match read_length {
            Ok(length) => length as usize,
            Err(error) if error.kind() == ErrorKind::UnexpectedEof => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        if length == 0 || length > MAX_REQUEST_BYTES {
            bail!("invalid request frame length {length}");
        }
        let mut bytes = vec![0; length];
        timeout(IO_TIMEOUT, stream.read_exact(&mut bytes))
            .await
            .context("local API connection timed out reading a frame")??;
        let response = match serde_json::from_slice::<ApiRequest>(&bytes) {
            Ok(request) => dispatch(&core, request).await,
            Err(error) => {
                ApiResponse::failure(String::new(), "INVALID_JSON", error.to_string(), false)
            }
        };
        let encoded = serde_json::to_vec(&response)?;
        if encoded.len() > MAX_RESPONSE_BYTES {
            bail!("response exceeds configured frame limit");
        }
        timeout(IO_TIMEOUT, async {
            stream.write_u32(u32::try_from(encoded.len())?).await?;
            stream.write_all(&encoded).await?;
            stream.flush().await?;
            Result::<()>::Ok(())
        })
        .await
        .context("local API connection timed out writing a response")??;
    }
    bail!("local API connection exceeded its request limit")
}

async fn dispatch(core: &CommunityCore, request: ApiRequest) -> ApiResponse {
    let id = request.id;
    if request.v != API_VERSION {
        return ApiResponse::failure(
            id,
            "UNSUPPORTED_VERSION",
            format!("API version {} is not supported", request.v),
            false,
        );
    }
    if id.is_empty() || id.len() > 128 {
        return ApiResponse::failure(
            id,
            "INVALID_REQUEST_ID",
            "request id must contain 1..=128 bytes",
            false,
        );
    }

    let result: Result<Value> = if request.method == "health" {
        core.call_public(&request.method).await.map(|mut value| {
            value["api_version"] = Value::from(API_VERSION);
            value
        })
    } else {
        match request.token.as_deref() {
            None => Err(anyhow::anyhow!("authentication token is required")),
            Some(token) => match core.authenticate(token).await {
                Ok(Some(principal)) => {
                    core.call_authenticated(&principal, &request.method, request.params)
                        .await
                }
                Ok(None) => Err(anyhow::anyhow!("authentication failed")),
                Err(error) => Err(error),
            },
        }
    };

    match result {
        Ok(value) => ApiResponse::success(id, value),
        Err(error) => {
            let message = error.to_string();
            let (code, retryable) = classify_error(&message);
            ApiResponse::failure(id, code, message, retryable)
        }
    }
}

fn classify_error(message: &str) -> (&'static str, bool) {
    if message.contains("authentication") {
        ("UNAUTHENTICATED", false)
    } else if message.contains("not allowed") || message.contains("not granted") {
        ("FORBIDDEN", false)
    } else if message.contains("log head changed")
        || message.contains("state changed before commit")
        || message.contains("current revision changed before commit")
        || message.contains("database is locked")
    {
        ("CONFLICT", true)
    } else if message.contains("cardinality_conflict") {
        ("CONFLICT", false)
    } else if message.contains("idempotency key was already used") {
        ("IDEMPOTENCY_CONFLICT", false)
    } else if message.contains("unknown public method")
        || message.contains("method is unknown or not allowed")
    {
        ("METHOD_NOT_FOUND", false)
    } else {
        ("INVALID_REQUEST", false)
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use tempfile::TempDir;

    use super::{SecureSocketBinding, prepare_socket};

    #[test]
    fn rejects_non_private_socket_parent_before_bind() {
        let temp = TempDir::new().unwrap();
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        let error = prepare_socket(&temp.path().join("community.sock")).unwrap_err();
        assert!(error.to_string().contains("private"));
    }

    #[test]
    fn accepts_owned_private_socket_parent() {
        let temp = TempDir::new().unwrap();
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        prepare_socket(&temp.path().join("community.sock")).unwrap();
    }

    #[test]
    fn rejects_nested_socket_parent_symlink() {
        let temp = TempDir::new().unwrap();
        let target = temp.path().join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o700)).unwrap();
        let nested = temp.path().join("nested");
        std::os::unix::fs::symlink(&target, &nested).unwrap();
        let error = prepare_socket(&nested.join("community.sock")).unwrap_err();
        assert!(error.to_string().contains("securely opening socket parent"));
    }

    #[tokio::test]
    async fn descriptor_bind_does_not_follow_replaced_parent() {
        let temp = TempDir::new().unwrap();
        let stable = temp.path().join("stable");
        let redirect = temp.path().join("redirect");
        let nested = temp.path().join("nested");
        let moved = temp.path().join("moved");
        let socket_name = "community.sock";
        for path in [&stable, &redirect] {
            std::fs::create_dir(path).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        std::fs::rename(&stable, &nested).unwrap();

        let binding: SecureSocketBinding = prepare_socket(&nested.join(socket_name)).unwrap();
        std::fs::rename(&nested, &moved).unwrap();
        std::os::unix::fs::symlink(&redirect, &nested).unwrap();

        let listener = binding.bind().unwrap();
        assert!(moved.join(socket_name).exists());
        assert!(!redirect.join(socket_name).exists());
        drop(listener);
        binding.remove_socket().unwrap();
        assert!(!moved.join(socket_name).exists());
    }
}
