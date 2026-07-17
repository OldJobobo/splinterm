use std::{
    env,
    io::ErrorKind,
    os::unix::fs::FileTypeExt,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, bail};
use splinterd::{LiveSplintConfig, LiveSplintRuntime};
use splinterm_core::{Lair, LayoutNode, SplintState};
use splinterm_protocol::{Envelope, PROTOCOL_VERSION, Request, Response};
use splinterm_pty::{LinuxPtyBackend, PtyCommand, default_shell};
use tokio::{
    fs,
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    signal,
    sync::{Mutex, RwLock},
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
    let live_splint = Arc::new(Mutex::new(None::<LiveSplintRuntime>));
    let pty_backend = LinuxPtyBackend::installed()?;
    info!(socket = %socket.display(), "splinterd ready");

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted.context("failed to accept client")?;
                let lair = Arc::clone(&lair);
                let live_splint = Arc::clone(&live_splint);
                let pty_backend = pty_backend.clone();
                tokio::spawn(async move {
                    if let Err(error) = serve_client(stream, lair, live_splint, pty_backend).await {
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

    if let Some(runtime) = live_splint.lock().await.take() {
        if let Err(error) = runtime.shutdown().await {
            error!(%error, "failed to shut down live Splint cleanly");
        }
    }

    fs::remove_file(&socket)
        .await
        .with_context(|| format!("failed to remove {}", socket.display()))?;
    Ok(())
}

async fn serve_client(
    stream: UnixStream,
    lair: Arc<RwLock<Lair>>,
    live_splint: Arc<Mutex<Option<LiveSplintRuntime>>>,
    pty_backend: LinuxPtyBackend,
) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    while let Some(line) = lines.next_line().await.context("failed to read request")? {
        let response = match serde_json::from_str::<Envelope<Request>>(&line) {
            Ok(envelope) if envelope.version == PROTOCOL_VERSION => {
                handle_request(envelope.message, &lair, &live_splint, &pty_backend).await
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

async fn handle_request(
    request: Request,
    lair: &Arc<RwLock<Lair>>,
    live_splint: &Mutex<Option<LiveSplintRuntime>>,
    pty_backend: &LinuxPtyBackend,
) -> Response {
    match request {
        Request::Ping => Response::Pong,
        Request::ListDojos => Response::Dojos {
            dojos: lair.read().await.dojos().cloned().collect(),
        },
        Request::CreateDojo { name, cwd } => {
            let mut live = live_splint.lock().await;
            if live.is_some() {
                return Response::Error {
                    message: "Phase 6 supports exactly one live Splint".into(),
                };
            }

            let created = {
                let mut lair = lair.write().await;
                lair.create_dojo(name, cwd.clone()).cloned()
            };
            let dojo = match created {
                Ok(dojo) => dojo,
                Err(error) => {
                    return Response::Error {
                        message: error.to_string(),
                    };
                }
            };
            let LayoutNode::Leaf(splint) = &dojo.windows[0].root else {
                unreachable!("new dojo starts with one shell Splint")
            };
            let command = PtyCommand::new(default_shell(), cwd).login_shell(true);
            match LiveSplintRuntime::spawn(
                splint.id,
                pty_backend.clone(),
                command,
                LiveSplintConfig::default(),
            )
            .await
            {
                Ok(runtime) => {
                    let handle = runtime.handle();
                    let mut lair_guard = lair.write().await;
                    let updated = lair_guard.set_splint_state(splint.id, SplintState::Running);
                    debug_assert!(updated);
                    let response_dojo = lair_guard
                        .dojos()
                        .find(|candidate| candidate.id == dojo.id)
                        .cloned()
                        .expect("created dojo remains present");
                    drop(lair_guard);
                    let lair_for_exit = Arc::clone(lair);
                    let splint_id = splint.id;
                    tokio::spawn(async move {
                        if let Some(status) = handle.wait_for_exit().await {
                            let code = status
                                .code
                                .or_else(|| status.signal.map(|signal| 128 + signal))
                                .unwrap_or(1);
                            lair_for_exit
                                .write()
                                .await
                                .set_splint_state(splint_id, SplintState::Exited(code));
                        }
                    });
                    *live = Some(runtime);
                    Response::DojoCreated {
                        dojo: response_dojo,
                    }
                }
                Err(error) => {
                    lair.write().await.remove_dojo(dojo.id);
                    Response::Error {
                        message: format!("failed to start shell: {error}"),
                    }
                }
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

    fn test_pty_backend() -> LinuxPtyBackend {
        let test_binary = std::env::current_exe().unwrap();
        let helper = test_binary
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("splinterm-pty-child");
        assert!(helper.is_file());
        LinuxPtyBackend::new(helper)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn create_dojo_starts_exactly_one_live_shell() {
        let lair = Arc::new(RwLock::new(Lair::new()));
        let live = Mutex::new(None);
        let response = handle_request(
            Request::CreateDojo {
                name: "main".into(),
                cwd: PathBuf::from("/tmp"),
            },
            &lair,
            &live,
            &test_pty_backend(),
        )
        .await;
        let Response::DojoCreated { dojo } = response else {
            panic!("expected a live dojo")
        };
        let LayoutNode::Leaf(splint) = &dojo.windows[0].root else {
            unreachable!()
        };
        assert_eq!(splint.state, SplintState::Running);

        let duplicate = handle_request(
            Request::CreateDojo {
                name: "second".into(),
                cwd: PathBuf::from("/tmp"),
            },
            &lair,
            &live,
            &test_pty_backend(),
        )
        .await;
        assert!(matches!(duplicate, Response::Error { .. }));
        live.lock().await.take().unwrap().shutdown().await.unwrap();
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
