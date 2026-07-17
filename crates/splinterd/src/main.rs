use std::{
    env,
    io::ErrorKind,
    os::unix::fs::FileTypeExt,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, bail};
use splinterm_core::Lair;
use splinterm_protocol::{Envelope, PROTOCOL_VERSION, Request, Response};
use tokio::{
    fs,
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    signal,
    sync::RwLock,
};
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let socket = socket_path()?;
    if let Some(parent) = socket.parent() {
        fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    remove_stale_socket(&socket).await?;

    let listener = UnixListener::bind(&socket)
        .with_context(|| format!("failed to bind {}", socket.display()))?;
    let lair = Arc::new(RwLock::new(Lair::new()));
    info!(socket = %socket.display(), "splinterd ready");

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted.context("failed to accept client")?;
                let lair = Arc::clone(&lair);
                tokio::spawn(async move {
                    if let Err(error) = serve_client(stream, lair).await {
                        error!(%error, "client connection failed");
                    }
                });
            }
            result = signal::ctrl_c() => {
                result.context("failed to listen for shutdown signal")?;
                info!("shutting down");
                break;
            }
        }
    }

    fs::remove_file(&socket)
        .await
        .with_context(|| format!("failed to remove {}", socket.display()))?;
    Ok(())
}

async fn serve_client(stream: UnixStream, lair: Arc<RwLock<Lair>>) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    while let Some(line) = lines.next_line().await.context("failed to read request")? {
        let response = match serde_json::from_str::<Envelope<Request>>(&line) {
            Ok(envelope) if envelope.version == PROTOCOL_VERSION => {
                handle_request(envelope.message, &lair).await
            }
            Ok(envelope) => Response::Error {
                message: format!(
                    "protocol version {} is unsupported; expected {PROTOCOL_VERSION}",
                    envelope.version
                ),
            },
            Err(error) => Response::Error {
                message: format!("invalid request: {error}"),
            },
        };

        let mut encoded = serde_json::to_vec(&Envelope::new(response))?;
        encoded.push(b'\n');
        writer
            .write_all(&encoded)
            .await
            .context("failed to reply")?;
    }

    Ok(())
}

async fn handle_request(request: Request, lair: &RwLock<Lair>) -> Response {
    match request {
        Request::Ping => Response::Pong,
        Request::ListDojos => Response::Dojos {
            dojos: lair.read().await.dojos().cloned().collect(),
        },
        Request::CreateDojo { name, cwd } => {
            let mut lair = lair.write().await;
            match lair.create_dojo(name, cwd) {
                Ok(dojo) => Response::DojoCreated { dojo: dojo.clone() },
                Err(error) => Response::Error {
                    message: error.to_string(),
                },
            }
        }
    }
}

async fn remove_stale_socket(path: &Path) -> Result<()> {
    match UnixStream::connect(path).await {
        Ok(_) => bail!("splinterd is already running at {}", path.display()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) if error.kind() == ErrorKind::ConnectionRefused => {
            let metadata = fs::symlink_metadata(path)
                .await
                .with_context(|| format!("failed to inspect {}", path.display()))?;
            if !metadata.file_type().is_socket() {
                bail!(
                    "refusing to remove {} because it is not a Unix socket",
                    path.display()
                );
            }

            fs::remove_file(path)
                .await
                .with_context(|| format!("failed to remove stale socket {}", path.display()))?;
            Ok(())
        }
        Err(error) => Err(error)
            .with_context(|| format!("failed to inspect socket endpoint {}", path.display())),
    }
}

fn socket_path() -> Result<PathBuf> {
    if let Some(path) = env::var_os("SPLINTERM_SOCKET") {
        return Ok(path.into());
    }

    let runtime = env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .context("XDG_RUNTIME_DIR is unset; set SPLINTERM_SOCKET explicitly")?;
    Ok(runtime.join("splinterm/splinterd.sock"))
}

#[cfg(test)]
mod tests {
    use std::{
        os::unix::net::UnixListener as StdUnixListener,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    fn temp_dir() -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        env::temp_dir().join(format!("splinterd-test-{}-{nonce}", std::process::id()))
    }

    #[tokio::test]
    async fn refuses_to_replace_a_regular_file() {
        let dir = temp_dir();
        let path = dir.join("endpoint");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(&path, "keep me").unwrap();

        let result = remove_stale_socket(&path).await;

        assert!(result.is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "keep me");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn removes_a_stale_unix_socket() {
        let dir = temp_dir();
        let path = dir.join("endpoint");
        std::fs::create_dir(&dir).unwrap();
        drop(StdUnixListener::bind(&path).unwrap());

        remove_stale_socket(&path).await.unwrap();

        assert!(!path.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
