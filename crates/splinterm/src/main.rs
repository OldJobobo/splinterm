use std::{
    env,
    io::{self, ErrorKind, Read, Write},
    path::PathBuf,
    sync::mpsc as std_mpsc,
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use splinterm::{
    AuthorityStatus, TrustedConsentUi, WindowCommand, WindowOptions, WindowUpdate, run_window,
};
use splinterm_core::SplintId;
use splinterm_protocol::{
    AccessGrant, AccessScope, ActiveScreen, CellAttributes, ClientFrame, ColorSource,
    ConsentPrompt, ConsentReply, ErrorCode, MAX_CONSENT_FRAME_BYTES, MAX_FRAME_BYTES,
    PROTOCOL_VERSION, Request, Response, ServerFrame, SubscriptionEvent, TerminalCell,
    TerminalInputModes, TerminalRow, TerminalSnapshot, TerminalUpdate, encode_frame,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    sync::mpsc,
};

const WINDOW_UPDATE_QUEUE: usize = 4;
const WINDOW_COMMAND_QUEUE: usize = 64;

#[derive(Debug, Parser)]
#[command(version, about = "Splinterm terminal client")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Render ordered snapshots of the daemon-owned live terminal.
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
    /// Private daemon-launched trusted consent surface.
    #[command(hide = true)]
    Consent,
}

#[tokio::main]
async fn main() -> Result<()> {
    let command = Cli::parse().command;
    if matches!(command, Command::Window) {
        return run_live_window().await;
    }
    if matches!(command, Command::Consent) {
        return tokio::task::spawn_blocking(run_consent_client)
            .await
            .context("trusted consent task failed")?;
    }

    let mut connection = Connection::connect().await?;
    match command {
        Command::Window | Command::Consent => {
            unreachable!("graphical command returned before daemon connection")
        }
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
            let controller_id = connection.acquire_control(splint_id, incarnation).await?;
            let response = connection
                .request(Request::Input {
                    controller_id,
                    splint_id,
                    incarnation,
                    bytes: text.into_bytes(),
                })
                .await?;
            connection.release_control(controller_id).await?;
            print_response(response)
        }
        Command::Resize { columns, rows } => {
            let (splint_id, incarnation) = connection.live_identity().await?;
            let controller_id = connection.acquire_control(splint_id, incarnation).await?;
            let response = connection
                .request(Request::Resize {
                    controller_id,
                    splint_id,
                    incarnation,
                    columns,
                    rows,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .await?;
            connection.release_control(controller_id).await?;
            print_response(response)
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

fn read_private_frame<T: serde::de::DeserializeOwned>(reader: &mut impl Read) -> Result<T> {
    let mut length = [0_u8; 4];
    reader.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_CONSENT_FRAME_BYTES {
        bail!("invalid private consent frame length");
    }
    let mut body = vec![0_u8; length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body).context("invalid private consent frame")
}

fn write_private_frame<T: serde::Serialize>(writer: &mut impl Write, value: &T) -> Result<()> {
    let body = serde_json::to_vec(value).context("encode private consent frame")?;
    if body.is_empty() || body.len() > MAX_CONSENT_FRAME_BYTES {
        bail!("private consent frame exceeds limit");
    }
    writer.write_all(&u32::try_from(body.len())?.to_be_bytes())?;
    writer.write_all(&body)?;
    writer.flush()?;
    Ok(())
}

fn consent_snapshot(prompt: &ConsentPrompt) -> TerminalSnapshot {
    let mut lines = vec![
        "TRUSTED SPLINTERM ACCESS REQUEST".to_owned(),
        String::new(),
        format!("Requester: {}", prompt.requester),
        format!(
            "Process: PID {} · UID {}",
            prompt.requester_pid, prompt.requester_uid
        ),
        format!(
            "Splint: {:?} · incarnation {}",
            prompt.splint_id, prompt.incarnation
        ),
        String::new(),
        "Requested one-time capabilities:".to_owned(),
    ];
    lines.extend(
        prompt
            .scopes
            .iter()
            .map(|scope| format!("  • {}", scope.label())),
    );
    lines.extend([
        String::new(),
        "This grant expires automatically and is not persisted.".to_owned(),
        "D / Escape: DENY          G / Enter: GRANT ONCE".to_owned(),
        "Click the red left or green right action area below.".to_owned(),
    ]);
    let columns = lines
        .iter()
        .map(String::len)
        .max()
        .unwrap_or(1)
        .clamp(64, 120);
    let rows = lines.len().max(18);
    let blank_attributes = CellAttributes {
        bold: false,
        dim: false,
        italic: false,
        underline: false,
        strikethrough: false,
        blink: false,
        conceal: false,
        reverse: false,
        foreground_source: ColorSource::Default,
        foreground: 0,
        background_source: ColorSource::Default,
        background: 0,
    };
    let mut visible_rows: Vec<_> = lines
        .into_iter()
        .map(|line| TerminalRow {
            linebreak: false,
            cells: line
                .chars()
                .take(columns)
                .map(|character| TerminalCell {
                    content: character.to_string(),
                    spacer_remaining: None,
                    attributes: blank_attributes,
                })
                .collect(),
        })
        .collect();
    visible_rows.resize_with(rows, || TerminalRow {
        linebreak: false,
        cells: Vec::new(),
    });
    TerminalSnapshot {
        splint_id: prompt.splint_id,
        incarnation: prompt.incarnation,
        revision: 1,
        columns,
        rows,
        cursor_column: -1,
        cursor_row: -1,
        cursor_deferred_wrap: false,
        active_screen: ActiveScreen::Normal,
        input_modes: TerminalInputModes {
            application_cursor: false,
            application_keypad: false,
            focus_reporting: false,
            bracketed_paste: false,
            cursor_visible: false,
            cursor_blink: false,
            mouse_tracking: splinterm_protocol::MouseTracking::None,
            sgr_mouse: false,
        },
        palette: vec![0; 256],
        default_colors: [0x00f4_f0e8, 0x0014_1820, 0x00e0_a030],
        title: "Trusted access request".to_owned(),
        visible_rows,
        scrollback_rows: Vec::new(),
        available_scrollback_rows: 0,
        omitted_oldest_scrollback_rows: 0,
        exited_code: None,
        exited_signal: None,
    }
}

fn run_consent_client() -> Result<()> {
    let prompt: ConsentPrompt = read_private_frame(&mut io::stdin().lock())?;
    if prompt.capability.len() != splinterm_protocol::CONSENT_CAPABILITY_BYTES
        || prompt.scopes.is_empty()
        || prompt.scopes.len() > splinterm_protocol::MAX_ACCESS_SCOPES
        || prompt.requester.chars().count() > 1024
    {
        bail!("invalid trusted consent prompt");
    }
    let (decision, receiver) = std_mpsc::channel();
    run_window(WindowOptions {
        snapshot: Some(consent_snapshot(&prompt)),
        trusted_consent: Some(TrustedConsentUi { decision }),
        ..WindowOptions::default()
    })?;
    let granted = receiver.try_recv().unwrap_or(false);
    write_private_frame(
        &mut io::stdout().lock(),
        &ConsentReply {
            capability: prompt.capability,
            granted,
        },
    )
}

struct Attachment {
    subscription_id: u64,
    snapshot: TerminalSnapshot,
}

#[allow(
    clippy::large_enum_variant,
    reason = "subscription events already own bounded protocol snapshots"
)]
#[derive(Debug, PartialEq)]
enum EventAction {
    Ignore,
    Snapshot {
        sequence: u64,
        snapshot: TerminalSnapshot,
    },
    Update {
        sequence: u64,
        update: TerminalUpdate,
    },
    Resynchronize,
    Shutdown,
}

fn classify_subscription_event(
    expected_subscription: u64,
    last_sequence: u64,
    subscription_id: u64,
    sequence: u64,
    event: SubscriptionEvent,
) -> EventAction {
    if subscription_id != expected_subscription {
        return EventAction::Ignore;
    }
    if last_sequence.checked_add(1) != Some(sequence) {
        return EventAction::Resynchronize;
    }
    match event {
        SubscriptionEvent::Snapshot { snapshot } => EventAction::Snapshot { sequence, snapshot },
        SubscriptionEvent::Update { update } => EventAction::Update { sequence, update },
        SubscriptionEvent::ResyncRequired { .. } => EventAction::Resynchronize,
        SubscriptionEvent::AccessRevoked { .. } | SubscriptionEvent::Exited { .. } => {
            EventAction::Shutdown
        }
    }
}

async fn attach(
    connection: &mut Connection,
    splint_id: SplintId,
    incarnation: u64,
) -> Result<Attachment> {
    let Response::Attached {
        subscription_id,
        snapshot,
    } = connection
        .request(Request::Attach {
            splint_id,
            incarnation,
            scrollback_rows: 0,
        })
        .await?
    else {
        bail!("splinterd did not return an attached terminal snapshot");
    };
    Ok(Attachment {
        subscription_id,
        snapshot,
    })
}

async fn resynchronize(
    connection: &mut Connection,
    old_subscription: u64,
    splint_id: SplintId,
    incarnation: u64,
) -> Result<Attachment> {
    let _ = connection
        .request(Request::Detach {
            subscription_id: old_subscription,
        })
        .await?;
    attach(connection, splint_id, incarnation).await
}

fn authority_status(grants: Vec<AccessGrant>, development_bypass: bool) -> AuthorityStatus {
    AuthorityStatus {
        grants: grants
            .into_iter()
            .filter(|grant| grant.grant_id != 0)
            .map(|grant| {
                let scopes = grant
                    .scopes
                    .iter()
                    .map(|scope| scope.label())
                    .collect::<Vec<_>>()
                    .join(", ");
                (grant.grant_id, format!("{}: {scopes}", grant.requester))
            })
            .collect(),
        development_bypass,
    }
}

async fn load_authority_status(
    connection: &mut Connection,
    splint_id: SplintId,
    incarnation: u64,
) -> Result<AuthorityStatus> {
    match connection
        .request(Request::AuthorizationStatus {
            splint_id,
            incarnation,
        })
        .await?
    {
        Response::AuthorizationStatus {
            grants,
            development_bypass,
        } => Ok(authority_status(grants, development_bypass)),
        _ => bail!("splinterd did not return authorization status"),
    }
}

async fn run_controller(
    mut control: Connection,
    mut commands: mpsc::Receiver<WindowCommand>,
    controller_id: u64,
    splint_id: SplintId,
    incarnation: u64,
) -> Result<()> {
    let mut active_controller = Some(controller_id);
    let result = async {
        while let Some(command) = commands.recv().await {
            let request = match command {
                WindowCommand::Input(bytes) => {
                    let Some(controller_id) = active_controller else {
                        continue;
                    };
                    Request::Input {
                        controller_id,
                        splint_id,
                        incarnation,
                        bytes,
                    }
                }
                WindowCommand::Resize {
                    columns,
                    rows,
                    pixel_width,
                    pixel_height,
                } => {
                    let Some(controller_id) = active_controller else {
                        continue;
                    };
                    Request::Resize {
                        controller_id,
                        splint_id,
                        incarnation,
                        columns,
                        rows,
                        pixel_width,
                        pixel_height,
                    }
                }
                WindowCommand::RevokeAccess(grant_id) => Request::RevokeAccess { grant_id },
                WindowCommand::ReleaseControl => {
                    let Some(controller_id) = active_controller.take() else {
                        continue;
                    };
                    Request::ReleaseControl { controller_id }
                }
            };
            if !matches!(control.request(request).await?, Response::Acknowledged) {
                bail!("splinterd did not acknowledge a window control command");
            }
        }
        Ok(())
    }
    .await;
    if let Some(controller_id) = active_controller {
        let _ = control.release_control(controller_id).await;
    }
    result
}

#[allow(
    clippy::too_many_lines,
    reason = "subscription resync, controller ownership, and window task shutdown are one lifecycle"
)]
async fn run_live_window() -> Result<()> {
    let mut connection = Connection::connect().await?;
    let (splint_id, incarnation) = connection.live_identity().await?;
    let requested_scopes = vec![
        AccessScope::Observe,
        AccessScope::Input,
        AccessScope::Resize,
    ];
    if !matches!(
        connection
            .request(Request::RequestAccess {
                splint_id,
                incarnation,
                scopes: requested_scopes,
            })
            .await?,
        Response::AccessGranted { .. }
    ) {
        bail!("splinterd did not grant requested terminal access");
    }
    let authority = load_authority_status(&mut connection, splint_id, incarnation).await?;
    let mut attachment = attach(&mut connection, splint_id, incarnation).await?;
    let mut control = Connection::connect().await?;
    let control_identity = control.live_identity().await?;
    if control_identity != (splint_id, incarnation) {
        bail!("control connection resolved a different live Splint identity");
    }
    let controller_id = control.acquire_control(splint_id, incarnation).await?;
    println!("Controller lease {controller_id} granted for live Splint");
    let (updates, receiver) = mpsc::channel(WINDOW_UPDATE_QUEUE);
    let (command_sender, commands) = mpsc::channel(WINDOW_COMMAND_QUEUE);
    let mut controller = tokio::spawn(run_controller(
        control,
        commands,
        controller_id,
        splint_id,
        incarnation,
    ));
    let mut last_revision = attachment.snapshot.revision;
    let initial_snapshot = attachment.snapshot;
    let mut window = tokio::task::spawn_blocking(move || {
        run_window(WindowOptions {
            snapshot: Some(initial_snapshot),
            updates: Some(receiver),
            commands: Some(command_sender),
            authority,
            ..WindowOptions::default()
        })
    });
    let mut last_sequence = 0_u64;

    loop {
        tokio::select! {
            result = &mut window => {
                // Dropping the window closes its command sender. The controller task then
                // releases the lease explicitly; connection teardown is the fallback.
                let window_result = result.context("Wayland window task failed")?;
                controller
                    .await
                    .context("window controller task failed")??;
                return window_result;
            }
            result = &mut controller => {
                let controller_result = result.context("window controller task failed")?;
                let _ = updates.send(WindowUpdate::Shutdown).await;
                let window_result = window.await.context("Wayland window task failed")?;
                controller_result?;
                return window_result;
            }
            frame = connection.next_server_frame() => {
                match frame? {
                    ServerFrame::Event {
                        subscription_id,
                        sequence,
                        event,
                    } => match classify_subscription_event(
                        attachment.subscription_id,
                        last_sequence,
                        subscription_id,
                        sequence,
                        event,
                    ) {
                        EventAction::Ignore => {}
                        EventAction::Snapshot { sequence, snapshot } => {
                            last_revision = snapshot.revision;
                            updates
                                .send(WindowUpdate::Snapshot(snapshot))
                                .await
                                .context("Wayland window closed its update queue")?;
                            last_sequence = sequence;
                        }
                        EventAction::Update { sequence, update }
                            if update.base_revision == last_revision
                                && update.revision == last_revision.saturating_add(1) => {
                            last_revision = update.revision;
                            updates
                                .send(WindowUpdate::Update(update))
                                .await
                                .context("Wayland window closed its update queue")?;
                            last_sequence = sequence;
                        }
                        EventAction::Update { .. } | EventAction::Resynchronize => {
                            attachment = resynchronize(
                                &mut connection,
                                attachment.subscription_id,
                                splint_id,
                                incarnation,
                            ).await?;
                            updates
                                .send(WindowUpdate::Snapshot(attachment.snapshot.clone()))
                                .await
                                .context("Wayland window closed its update queue")?;
                            last_revision = attachment.snapshot.revision;
                            last_sequence = 0;
                        }
                        EventAction::Shutdown => {
                            let _ = updates.send(WindowUpdate::Shutdown).await;
                            let window_result = window.await.context("Wayland window task failed")?;
                            controller
                                .await
                                .context("window controller task failed")??;
                            return window_result;
                        }
                    },
                    ServerFrame::Error { error, .. } => {
                        bail!("splinterd: {}", error.message);
                    }
                    _ => bail!("splinterd sent an unexpected frame while subscribed"),
                }
            }
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

    async fn next_server_frame(&mut self) -> Result<ServerFrame> {
        read_frame(&mut self.stream).await
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

    async fn acquire_control(&mut self, splint_id: SplintId, incarnation: u64) -> Result<u64> {
        match self
            .request(Request::AcquireControl {
                splint_id,
                incarnation,
            })
            .await?
        {
            Response::ControlGranted { controller_id } if controller_id != 0 => Ok(controller_id),
            _ => bail!("splinterd did not grant a controller lease"),
        }
    }

    async fn release_control(&mut self, controller_id: u64) -> Result<()> {
        if matches!(
            self.request(Request::ReleaseControl { controller_id })
                .await?,
            Response::Acknowledged
        ) {
            Ok(())
        } else {
            bail!("splinterd did not release the controller lease")
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
        Response::AccessGranted { grant } => {
            println!("Access grant {} issued.", grant.grant_id);
        }
        Response::AuthorizationStatus {
            grants,
            development_bypass,
        } => {
            println!(
                "{} active grant(s); development bypass={development_bypass}",
                grants.len()
            );
        }
        Response::ControlGranted { controller_id } => {
            println!("Controller lease {controller_id} granted.");
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use splinterm_protocol::{ActiveScreen, TerminalInputModes};

    fn snapshot(revision: u64) -> TerminalSnapshot {
        TerminalSnapshot {
            splint_id: SplintId::new(),
            incarnation: 1,
            revision,
            columns: 0,
            rows: 0,
            cursor_column: 0,
            cursor_row: 0,
            cursor_deferred_wrap: false,
            active_screen: ActiveScreen::Normal,
            input_modes: TerminalInputModes {
                application_cursor: false,
                application_keypad: false,
                focus_reporting: false,
                bracketed_paste: false,
                cursor_visible: true,
                cursor_blink: true,
                mouse_tracking: splinterm_protocol::MouseTracking::None,
                sgr_mouse: false,
            },
            palette: vec![0; 256],
            default_colors: [0x00eb_ebeb, 0x000e_1216, 0x00eb_ebeb],
            title: String::new(),
            visible_rows: Vec::new(),
            scrollback_rows: Vec::new(),
            available_scrollback_rows: 0,
            omitted_oldest_scrollback_rows: 0,
            exited_code: None,
            exited_signal: None,
        }
    }

    #[test]
    fn subscription_classifier_tracks_order_and_resyncs_gaps() {
        let action = classify_subscription_event(
            9,
            0,
            9,
            1,
            SubscriptionEvent::Snapshot {
                snapshot: snapshot(2),
            },
        );
        assert!(matches!(action, EventAction::Snapshot { sequence: 1, .. }));
        assert_eq!(
            classify_subscription_event(
                9,
                1,
                9,
                3,
                SubscriptionEvent::Snapshot {
                    snapshot: snapshot(3)
                },
            ),
            EventAction::Resynchronize
        );
        assert_eq!(
            classify_subscription_event(
                9,
                1,
                9,
                2,
                SubscriptionEvent::ResyncRequired {
                    current_revision: 4
                },
            ),
            EventAction::Resynchronize
        );
    }

    #[test]
    fn subscription_classifier_ignores_old_subscription_and_stops_on_exit() {
        assert_eq!(
            classify_subscription_event(
                9,
                0,
                8,
                1,
                SubscriptionEvent::Snapshot {
                    snapshot: snapshot(2)
                },
            ),
            EventAction::Ignore
        );
        assert_eq!(
            classify_subscription_event(
                9,
                0,
                9,
                1,
                SubscriptionEvent::Exited {
                    code: Some(0),
                    signal: None
                },
            ),
            EventAction::Shutdown
        );
    }
}
