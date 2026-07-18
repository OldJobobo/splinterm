use std::{env, io::ErrorKind, path::PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use splinterm::{WindowOptions, run_window};
use splinterm_core::SplintId;
use splinterm_protocol::{
    ClientFrame, ErrorCode, MAX_FRAME_BYTES, PROTOCOL_VERSION, Request, Response, ServerFrame,
    TerminalSnapshot, encode_frame,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
};

#[derive(Debug, Parser)]
#[command(version, about = "Splinterm terminal client")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Open the native renderer window without attaching terminal state yet.
    Window,
    Ping,
    List,
    New {
        name: String,
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// Show the current live terminal snapshot (development mode only).
    Snapshot,
    /// Send literal UTF-8 text to the live shell (development mode only).
    Send {
        text: String,
    },
    /// Resize the live terminal (development mode only).
    Resize {
        columns: u16,
        rows: u16,
    },
    /// Terminate the live shell (development mode only).
    Terminate,
}

#[tokio::main]
async fn main() -> Result<()> {
    let command = Cli::parse().command;
    if matches!(command, Command::Window) {
        run_window(WindowOptions::default())?;
        return Ok(());
    }

    let mut connection = Connection::connect().await?;
    match command {
        Command::Window => unreachable!("window command returned before daemon connection"),
        Command::Ping => print_response(connection.request(Request::Ping).await?),
        Command::List => print_response(connection.request(Request::ListDojos).await?),
        Command::New { name, cwd } => print_response(
            connection
                .request(Request::CreateDojo {
                    name,
                    cwd: cwd
                        .unwrap_or(env::current_dir().context("failed to read current directory")?),
                })
                .await?,
        ),
        Command::Snapshot => {
            let (splint_id, incarnation) = connection.live_identity().await?;
            print_response(
                connection
                    .request(Request::Attach {
                        splint_id,
                        incarnation,
                        scrollback_rows: 16,
                    })
                    .await?,
            )
        }
        Command::Send { text } => {
            let (splint_id, incarnation) = connection.live_identity().await?;
            print_response(
                connection
                    .request(Request::Input {
                        splint_id,
                        incarnation,
                        bytes: text.into_bytes(),
                    })
                    .await?,
            )
        }
        Command::Resize { columns, rows } => {
            let (splint_id, incarnation) = connection.live_identity().await?;
            print_response(
                connection
                    .request(Request::Resize {
                        splint_id,
                        incarnation,
                        columns,
                        rows,
                        pixel_width: 0,
                        pixel_height: 0,
                    })
                    .await?,
            )
        }
        Command::Terminate => {
            let (splint_id, incarnation) = connection.live_identity().await?;
            print_response(
                connection
                    .request(Request::Terminate {
                        splint_id,
                        incarnation,
                    })
                    .await?,
            )
        }
    }
}

struct Connection {
    stream: UnixStream,
    next_request: u64,
}

impl Connection {
    async fn connect() -> Result<Self> {
        let socket = socket_path()?;
        let mut stream = UnixStream::connect(&socket)
            .await
            .with_context(|| format!("cannot connect to splinterd at {}", socket.display()))?;
        write_frame(
            &mut stream,
            &ClientFrame::Hello {
                minimum_version: PROTOCOL_VERSION,
                maximum_version: PROTOCOL_VERSION,
            },
        )
        .await?;
        match read_frame(&mut stream).await? {
            ServerFrame::Hello { version, .. } if version == PROTOCOL_VERSION => {}
            ServerFrame::Error { error, .. } => bail!("splinterd: {}", error.message),
            _ => bail!("splinterd sent an invalid handshake"),
        }
        Ok(Self {
            stream,
            next_request: 1,
        })
    }

    async fn request(&mut self, request: Request) -> Result<Response> {
        let request_id = self.next_request;
        self.next_request += 1;
        write_frame(
            &mut self.stream,
            &ClientFrame::Request {
                request_id,
                request,
            },
        )
        .await?;
        loop {
            match read_frame(&mut self.stream).await? {
                ServerFrame::Response {
                    request_id: response_id,
                    result,
                } if response_id == request_id => return Ok(result),
                ServerFrame::Error {
                    request_id: Some(response_id),
                    error,
                } if response_id == request_id => {
                    if error.code == ErrorCode::DevelopmentFeatureDisabled {
                        bail!(
                            "splinterd: {} (restart with SPLINTERM_ENABLE_DEV_ATTACH=1)",
                            error.message
                        );
                    }
                    bail!(
                        "splinterd [{}]: {}",
                        format!("{:?}", error.code).to_lowercase(),
                        error.message
                    )
                }
                ServerFrame::Event { .. } => {}
                _ => bail!("splinterd sent a response with the wrong request id"),
            }
        }
    }

    async fn live_identity(&mut self) -> Result<(SplintId, u64)> {
        match self.request(Request::InspectLiveSplint).await? {
            Response::LiveSplint {
                splint_id,
                incarnation,
            } => Ok((splint_id, incarnation)),
            _ => bail!("splinterd did not return a live Splint identity"),
        }
    }
}

async fn write_frame(stream: &mut UnixStream, frame: &ClientFrame) -> Result<()> {
    stream.write_all(&encode_frame(frame)?).await?;
    Ok(())
}

async fn read_frame(stream: &mut UnixStream) -> Result<ServerFrame> {
    let mut length = [0_u8; 4];
    stream.read_exact(&mut length).await?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        bail!("splinterd sent an oversized frame");
    }
    let mut body = vec![0_u8; length];
    stream.read_exact(&mut body).await.map_err(|error| {
        if error.kind() == ErrorKind::UnexpectedEof {
            anyhow::anyhow!("splinterd sent a truncated frame")
        } else {
            error.into()
        }
    })?;
    serde_json::from_slice(&body).context("splinterd sent invalid JSON")
}

#[allow(
    clippy::unnecessary_wraps,
    reason = "response rendering retains a fallible CLI boundary for future output modes"
)]
fn print_response(response: Response) -> Result<()> {
    match response {
        Response::Pong => println!("splinterd is awake"),
        Response::Dojos { dojos } if dojos.is_empty() => println!("No dojos in the lair."),
        Response::Dojos { dojos } => {
            for dojo in dojos {
                let splints: usize = dojo
                    .windows
                    .iter()
                    .map(|window| window.root.splint_count())
                    .sum();
                println!(
                    "{}  {} window(s)  {splints} splint(s)",
                    dojo.name,
                    dojo.windows.len()
                );
            }
        }
        Response::DojoCreated { dojo } => println!("Created dojo '{}'.", dojo.name),
        Response::Attached { snapshot, .. } => print_snapshot(&snapshot),
        Response::Acknowledged => println!("Acknowledged."),
        Response::Terminated { code, signal } => {
            println!("Shell terminated (code={code:?}, signal={signal:?}).");
        }
        Response::LiveSplint {
            splint_id,
            incarnation,
        } => println!("Live Splint {splint_id:?}, incarnation {incarnation}"),
    }
    Ok(())
}

fn print_snapshot(snapshot: &TerminalSnapshot) {
    println!(
        "Splint {:?} · incarnation {} · revision {} · {}x{}",
        snapshot.splint_id,
        snapshot.incarnation,
        snapshot.revision,
        snapshot.columns,
        snapshot.rows
    );
    for row in &snapshot.visible_rows {
        let line: String = row
            .cells
            .iter()
            .map(|cell| {
                if cell.content.is_empty() {
                    " "
                } else {
                    &cell.content
                }
            })
            .collect();
        println!("{}", line.trim_end());
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
