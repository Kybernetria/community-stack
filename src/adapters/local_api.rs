use std::{
    io::ErrorKind,
    os::unix::fs::{FileTypeExt, PermissionsExt},
    path::Path,
    sync::Arc,
};

use anyhow::{Context, Result, bail};
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
    prepare_socket(socket_path)?;
    let listener = UnixListener::bind(socket_path)
        .with_context(|| format!("binding local API at {}", socket_path.display()))?;
    std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))?;
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
    let _ = std::fs::remove_file(socket_path);
    Ok(())
}

fn prepare_socket(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_socket() => std::fs::remove_file(path)?,
        Ok(_) => bail!("refusing to replace non-socket path {}", path.display()),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
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
    } else if message.contains("document revision conflict") {
        ("REVISION_CONFLICT", false)
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
