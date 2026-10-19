//! The initiating side of a live daemon upgrade (Plan 0061 M3.13d).
//!
//! A trusted local client asks the daemon for a dry-run report, or starts the
//! handoff with `PrepareDaemonHandoff { Execute }` and answers the one
//! decision prompt the daemon may send about Windows that cannot take part
//! (operator decision D10). The initiator always uses its own connection: the
//! daemon refuses `Execute` on a Window's anchor connection.

use std::{future::Future, time::Duration};

use anyhow::{Context, Result, bail};
use splinterm_protocol::{
    ClientFrame, DaemonHandoffMode, ErrorCode, HandoffBlockingReason, HandoffReport,
    HandoffUnresponsiveWindow, ProtocolError, Request, Response, ServerFrame,
    validate_handoff_blocking_reasons, validate_handoff_decision,
    validate_handoff_decision_required,
};

use crate::{Connection, DaemonProtocolFailure, InFlightRequest, is_daemon_closed_connection};

/// How long before the daemon's decision deadline the initiator stops waiting
/// for its own answer, so a `Cancel` or decision still arrives in time.
pub const HANDOFF_DECISION_SAFETY_MARGIN: Duration = Duration::from_millis(1_500);

/// Longest the initiator waits for the daemon in any phase that has no
/// decision deadline of its own: preparation before a prompt, and capture and
/// exec after a decision. A daemon that says nothing for this long is treated
/// as failed; the request is cancelled and the connection closed.
const HANDOFF_ANSWER_TIMEOUT: Duration = Duration::from_secs(120);

/// Longest the initiator waits for the final answer after it cancelled.
const HANDOFF_CANCEL_ANSWER_TIMEOUT: Duration = Duration::from_secs(30);

/// One validated decision prompt, owned so a callback can keep it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffDecisionPrompt {
    /// Generation of the daemon that is handing off.
    pub daemon_generation: u64,
    /// How long the daemon waits for the answer, counted from when it wrote
    /// the prompt.
    pub deadline: Duration,
    /// How long the callback may take. The helper enforces it and treats an
    /// unanswered callback as a cancel; it ends before `deadline`.
    pub respond_within: Duration,
    /// Every Window that cannot take part, with the Dojos the daemon names.
    pub windows: Vec<HandoffUnresponsiveWindow>,
}

/// The person's answer to a [`HandoffDecisionPrompt`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandoffDecision {
    /// Continue the upgrade without every listed Window. Those Windows close;
    /// their terminals keep running in the upgraded daemon.
    ProceedWithout,
    /// Cancel the upgrade. Nothing changes.
    Cancel,
}

/// How a `PrepareDaemonHandoff { Execute }` request ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonHandoffOutcome {
    /// The daemon closed the connection without answering, which is how a
    /// committed handoff ends: the old daemon leaves serving for the exec
    /// boundary and never answers `Execute`. A clean end of file cannot
    /// distinguish this from a daemon that crashed at the same moment, so a
    /// caller that needs certainty reconnects and checks the generation.
    HandedOff {
        /// The prompt indices of the Windows the upgrade proceeded without.
        proceeded_without: Vec<u8>,
    },
    /// The handoff cannot run now. Nothing changed.
    Unavailable {
        reasons: Vec<HandoffBlockingReason>,
        message: String,
    },
    /// Another handoff is already running.
    InProgress { message: String },
    /// The daemon refused the request for another reason, such as
    /// `Unauthorized` for a client that is not the installed trusted client,
    /// or `InvalidArgument` for `Execute` on a Window anchor connection.
    Refused(ProtocolError),
    /// The callback chose to cancel and the daemon confirmed. Nothing changed.
    Cancelled { message: String },
    /// The callback did not answer within its bound, so the helper cancelled
    /// and the daemon confirmed. Nothing changed.
    TimedOut { message: String },
}

/// One frame an executing handoff request can receive, after correlation.
enum ExecuteFrame {
    /// The correlated final answer; a success response carries nothing the
    /// initiator uses, since `Execute` never succeeds with one.
    Final(std::result::Result<(), ProtocolError>),
    Prompt(HandoffDecisionPrompt),
    Closed,
}

/// The callback's budget: the safety margin before the daemon's deadline,
/// but never less than half of it, so a short deadline still leaves time to
/// answer before the daemon gives up.
fn decision_budget(deadline: Duration) -> Duration {
    deadline
        .saturating_sub(HANDOFF_DECISION_SAFETY_MARGIN)
        .max(deadline / 2)
}

impl Connection {
    /// Asks the daemon whether it could hand off now, changing nothing.
    ///
    /// Trusted local graphical UI only: the daemon answers `Unauthorized`
    /// unless this process runs the installed client beside the running
    /// daemon.
    pub async fn daemon_handoff_report(&mut self) -> Result<HandoffReport> {
        match self
            .request(Request::PrepareDaemonHandoff {
                mode: DaemonHandoffMode::DryRun,
            })
            .await?
        {
            Response::HandoffReport { report } => {
                report
                    .validate()
                    .map_err(|error| anyhow::anyhow!("splinterd sent {}", error.message))?;
                Ok(report)
            }
            _ => bail!("splinterd returned an unexpected response to a handoff dry run"),
        }
    }

    /// Starts a live daemon handoff and drives it to its end.
    ///
    /// The request is sent on this connection, which must not be a Window's
    /// anchor (the daemon refuses that). When Windows cannot take part, the
    /// daemon sends one decision prompt; `decide` receives it and must answer
    /// within [`HandoffDecisionPrompt::respond_within`]. The helper keeps
    /// reading the connection meanwhile, so a daemon that ends the attempt or
    /// fails drops the callback. `ProceedWithout` answers with exactly the
    /// prompted indices; `Cancel`, or no answer in time, sends `Cancel` and
    /// waits for the daemon to confirm.
    ///
    /// The connection cannot be reused afterwards. Dropping the returned
    /// future while the request is in flight sends one best-effort `Cancel`
    /// and closes the connection, which also cancels the attempt.
    ///
    /// # Errors
    /// Transport and framing failures, a malformed, misdirected or repeated
    /// prompt, an unexpected successful response, the connection closing
    /// while a prompt or cancellation is pending, and a daemon silent past an
    /// internal bound.
    pub async fn execute_daemon_handoff<D, F>(&mut self, decide: D) -> Result<DaemonHandoffOutcome>
    where
        D: FnOnce(HandoffDecisionPrompt) -> F,
        F: Future<Output = HandoffDecision>,
    {
        if !self.trusted_ui {
            bail!("a daemon handoff requires the trusted local client connection");
        }
        let request_id = self.reserve_request_id()?;
        let mut in_flight = InFlightRequest {
            connection: self,
            request_id,
            armed: true,
        };
        let result = in_flight.connection.drive_handoff(request_id, decide).await;
        in_flight.disarm();
        // A late frame (such as the daemon's answer to a `Cancel` that
        // crossed its timeout) must never be read as another request's.
        in_flight.connection.mark_unusable();
        result
    }

    async fn drive_handoff<D, F>(
        &mut self,
        request_id: u64,
        decide: D,
    ) -> Result<DaemonHandoffOutcome>
    where
        D: FnOnce(HandoffDecisionPrompt) -> F,
        F: Future<Output = HandoffDecision>,
    {
        self.write_client_frame(&ClientFrame::Request {
            request_id,
            diagnostic_correlation: self.diagnostic_correlation,
            request: Request::PrepareDaemonHandoff {
                mode: DaemonHandoffMode::Execute,
            },
        })
        .await?;
        let prompt = match self
            .bounded_execute_frame(request_id, true, HANDOFF_ANSWER_TIMEOUT)
            .await?
        {
            ExecuteFrame::Final(result) => return final_outcome(result),
            ExecuteFrame::Closed => {
                return Ok(DaemonHandoffOutcome::HandedOff {
                    proceeded_without: Vec::new(),
                });
            }
            ExecuteFrame::Prompt(prompt) => prompt,
        };
        let indices = prompt
            .windows
            .iter()
            .map(|window| window.index)
            .collect::<Vec<_>>();
        let respond_within = prompt.respond_within;
        // Events are queued inside `next_execute_frame`, so its branch only
        // completes with the end of the attempt.
        let decision = tokio::select! {
            biased;
            frame = self.next_execute_frame(request_id, false) => match frame? {
                // The daemon ended the attempt first, such as on its own
                // deadline; the callback is dropped.
                ExecuteFrame::Final(result) => return final_outcome(result),
                ExecuteFrame::Closed => bail!(
                    "splinterd closed the connection while the upgrade waited for a decision"
                ),
                ExecuteFrame::Prompt(_) => unreachable!("prompts are refused while deciding"),
            },
            decision = decide(prompt) => Some(decision),
            () = tokio::time::sleep(respond_within) => None,
        };
        match decision {
            Some(HandoffDecision::ProceedWithout) => {
                self.proceed_without(request_id, indices).await
            }
            Some(HandoffDecision::Cancel) => {
                let message = self.cancel_and_confirm(request_id).await?;
                Ok(DaemonHandoffOutcome::Cancelled { message })
            }
            None => {
                let message = self.cancel_and_confirm(request_id).await?;
                Ok(DaemonHandoffOutcome::TimedOut { message })
            }
        }
    }

    /// Answers the prompt with exactly its indices and waits for the end.
    async fn proceed_without(
        &mut self,
        request_id: u64,
        indices: Vec<u8>,
    ) -> Result<DaemonHandoffOutcome> {
        validate_handoff_decision(request_id, &indices)
            .map_err(|error| anyhow::anyhow!(error.message))?;
        self.write_client_frame(&ClientFrame::HandoffDecision {
            request_id,
            proceed_without: indices.clone(),
        })
        .await?;
        match self
            .bounded_execute_frame(request_id, false, HANDOFF_ANSWER_TIMEOUT)
            .await?
        {
            ExecuteFrame::Final(result) => final_outcome(result),
            ExecuteFrame::Closed => Ok(DaemonHandoffOutcome::HandedOff {
                proceeded_without: indices,
            }),
            ExecuteFrame::Prompt(_) => unreachable!("prompts are refused after deciding"),
        }
    }

    /// Cancels the executing request and returns the daemon's confirmation.
    async fn cancel_and_confirm(&mut self, request_id: u64) -> Result<String> {
        self.write_client_frame(&ClientFrame::Cancel { request_id })
            .await?;
        match self
            .bounded_execute_frame(request_id, false, HANDOFF_CANCEL_ANSWER_TIMEOUT)
            .await?
        {
            ExecuteFrame::Final(Err(error)) => Ok(error.message),
            ExecuteFrame::Final(Ok(())) => {
                bail!("splinterd answered a cancelled handoff with a response")
            }
            ExecuteFrame::Closed => {
                bail!("splinterd closed the connection before confirming the cancel")
            }
            ExecuteFrame::Prompt(_) => unreachable!("prompts are refused after deciding"),
        }
    }

    async fn bounded_execute_frame(
        &mut self,
        request_id: u64,
        accept_prompt: bool,
        bound: Duration,
    ) -> Result<ExecuteFrame> {
        if let Ok(frame) =
            tokio::time::timeout(bound, self.next_execute_frame(request_id, accept_prompt)).await
        {
            frame
        } else {
            self.cancel_and_close(request_id);
            bail!(
                "splinterd did not answer the handoff within {} seconds",
                bound.as_secs()
            )
        }
    }

    /// Reads until a frame for the executing request arrives. Events are
    /// queued as for any request. Cancel-safe: the only await point is the
    /// transport read, and a frame is consumed only once it is complete.
    async fn next_execute_frame(
        &mut self,
        request_id: u64,
        accept_prompt: bool,
    ) -> Result<ExecuteFrame> {
        loop {
            let frame = match self.read_server_frame_with(accept_prompt).await {
                Ok(frame) => frame,
                Err(error) if is_daemon_closed_connection(&error) => {
                    self.mark_unusable();
                    return Ok(ExecuteFrame::Closed);
                }
                Err(error) => {
                    self.mark_unusable();
                    return Err(error);
                }
            };
            match frame {
                ServerFrame::Response {
                    request_id: response_id,
                    ..
                } if response_id == request_id => return Ok(ExecuteFrame::Final(Ok(()))),
                ServerFrame::Error {
                    request_id: Some(response_id),
                    error,
                } if response_id == request_id => return Ok(ExecuteFrame::Final(Err(error))),
                ServerFrame::Error {
                    request_id: None,
                    error,
                } => {
                    self.mark_unusable();
                    return Err(anyhow::Error::new(DaemonProtocolFailure(error)));
                }
                ServerFrame::HandoffDecisionRequired {
                    request_id: prompt_id,
                    daemon_generation,
                    deadline_ms,
                    windows,
                } => {
                    let valid = prompt_id == request_id
                        && daemon_generation == self.daemon_generation
                        && validate_handoff_decision_required(
                            prompt_id,
                            daemon_generation,
                            deadline_ms,
                            &windows,
                        )
                        .is_ok();
                    if !valid {
                        self.mark_unusable();
                        bail!("splinterd sent an invalid handoff decision prompt");
                    }
                    let deadline = Duration::from_millis(u64::from(deadline_ms));
                    return Ok(ExecuteFrame::Prompt(HandoffDecisionPrompt {
                        daemon_generation,
                        deadline,
                        respond_within: decision_budget(deadline),
                        windows,
                    }));
                }
                event @ ServerFrame::Event { .. } => {
                    if let Err(error) = self.queue_event(event) {
                        self.mark_unusable();
                        return Err(error);
                    }
                }
                _ => {
                    self.mark_unusable();
                    bail!("splinterd sent a response with the wrong request id")
                }
            }
        }
    }
}

/// Maps the daemon's final answer to an `Execute` that was not cancelled.
fn final_outcome(result: std::result::Result<(), ProtocolError>) -> Result<DaemonHandoffOutcome> {
    let Err(error) = result else {
        bail!("splinterd answered a handoff execute with an unexpected response")
    };
    Ok(match error.code {
        ErrorCode::HandoffUnavailable => {
            validate_handoff_blocking_reasons(&error.handoff_blocking_reasons)
                .ok()
                .context("splinterd sent invalid handoff blocking reasons")?;
            DaemonHandoffOutcome::Unavailable {
                reasons: error.handoff_blocking_reasons,
                message: error.message,
            }
        }
        ErrorCode::HandoffInProgress => DaemonHandoffOutcome::InProgress {
            message: error.message,
        },
        _ => DaemonHandoffOutcome::Refused(error),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use splinterm_core::{DojoId, LairId};
    use splinterm_protocol::{
        ClientRole, HandoffBuild, HandoffBuildProvenance, HandoffVersionRange, HandoffWindowDojo,
        HandoffWindowIssue, PROTOCOL_VERSION, ServerLimits, encode_frame,
    };
    use tokio::{
        io::{AsyncReadExt as _, AsyncWriteExt as _},
        net::UnixStream,
    };

    const GENERATION: u64 = 4;

    async fn read_client_frame(stream: &mut UnixStream) -> Option<ClientFrame> {
        let mut length = [0_u8; 4];
        stream.read_exact(&mut length).await.ok()?;
        let mut body = vec![0_u8; u32::from_be_bytes(length) as usize];
        stream.read_exact(&mut body).await.ok()?;
        Some(serde_json::from_slice(&body).unwrap())
    }

    async fn write_server_frame(stream: &mut UnixStream, frame: &ServerFrame) {
        stream
            .write_all(&encode_frame(frame).unwrap())
            .await
            .unwrap();
    }

    /// Negotiates a trusted-UI connection with a scripted daemon.
    async fn connect(
        script: impl FnOnce(UnixStream) -> tokio::task::JoinHandle<()>,
    ) -> (Connection, tokio::task::JoinHandle<()>) {
        let (client, server) = UnixStream::pair().unwrap();
        let server_task = script(server);
        let connection = Connection::connect_stream(client, ClientRole::TrustedUi)
            .await
            .unwrap();
        (connection, server_task)
    }

    async fn accept_hello(server: &mut UnixStream) {
        assert!(matches!(
            read_client_frame(server).await,
            Some(ClientFrame::Hello {
                role: ClientRole::TrustedUi,
                handoff_window: false,
                ..
            })
        ));
        write_server_frame(
            server,
            &ServerFrame::Hello {
                version: PROTOCOL_VERSION,
                limits: ServerLimits::default(),
                development_terminal_access: false,
                daemon_hostname: None,
                daemon_generation: GENERATION,
            },
        )
        .await;
    }

    /// Reads the client's `Execute` and returns its request id.
    async fn expect_execute(server: &mut UnixStream) -> u64 {
        match read_client_frame(server).await {
            Some(ClientFrame::Request {
                request_id,
                request:
                    Request::PrepareDaemonHandoff {
                        mode: DaemonHandoffMode::Execute,
                    },
                ..
            }) => request_id,
            other => panic!("expected an Execute request, got {other:?}"),
        }
    }

    fn window(index: u8, pid: u32, dojos: &[&str]) -> HandoffUnresponsiveWindow {
        HandoffUnresponsiveWindow {
            index,
            pid,
            reason: HandoffWindowIssue::NoAcknowledgement,
            dojos: dojos
                .iter()
                .map(|name| HandoffWindowDojo {
                    lair_id: LairId::new(),
                    dojo_id: DojoId::new(),
                    name: (*name).to_owned(),
                })
                .collect(),
        }
    }

    fn prompt(request_id: u64, deadline_ms: u32) -> ServerFrame {
        ServerFrame::HandoffDecisionRequired {
            request_id,
            daemon_generation: GENERATION,
            deadline_ms,
            // Noncontiguous indices: the answer must echo them exactly.
            windows: vec![window(3, 101, &["work", "logs"]), window(7, 202, &[])],
        }
    }

    fn unavailable(request_id: u64, message: &str) -> ServerFrame {
        ServerFrame::Error {
            request_id: Some(request_id),
            error: ProtocolError {
                message: message.to_owned(),
                ..ProtocolError::handoff_unavailable(vec![HandoffBlockingReason::Unsupported])
            },
        }
    }

    fn report() -> HandoffReport {
        let range = HandoffVersionRange {
            minimum: 1,
            maximum: 1,
        };
        HandoffReport {
            daemon_generation: GENERATION,
            build: HandoffBuild {
                version: "0.2.0".to_owned(),
                identity: [7; splinterm_protocol::HANDOFF_BUILD_IDENTITY_BYTES],
                provenance: HandoffBuildProvenance::ReleaseCandidate,
            },
            private_protocol: range,
            handoff_protocol: range,
            terminal_checkpoint: range,
            descriptor_manifest: range,
            lairs: 1,
            transient_lairs: 0,
            dojos: 2,
            live_splints: 3,
            rollback_available: true,
            blocking_reasons: vec![HandoffBlockingReason::Unsupported],
        }
    }

    #[tokio::test]
    async fn dry_run_returns_the_validated_report() {
        let (mut connection, server_task) = connect(|mut server| {
            tokio::spawn(async move {
                accept_hello(&mut server).await;
                let Some(ClientFrame::Request {
                    request_id,
                    request:
                        Request::PrepareDaemonHandoff {
                            mode: DaemonHandoffMode::DryRun,
                        },
                    ..
                }) = read_client_frame(&mut server).await
                else {
                    panic!("expected a dry run");
                };
                write_server_frame(
                    &mut server,
                    &ServerFrame::Response {
                        request_id,
                        result: Response::HandoffReport { report: report() },
                    },
                )
                .await;
            })
        })
        .await;
        assert_eq!(connection.daemon_handoff_report().await.unwrap(), report());
        // A local stream records the accepting process from SO_PEERCRED.
        assert_eq!(connection.daemon_pid(), Some(std::process::id()));
        // A dry run leaves the connection usable.
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn execute_refused_as_unsupported_is_unavailable_and_never_prompts() {
        let (mut connection, server_task) = connect(|mut server| {
            tokio::spawn(async move {
                accept_hello(&mut server).await;
                let request_id = expect_execute(&mut server).await;
                write_server_frame(
                    &mut server,
                    &unavailable(
                        request_id,
                        "daemon handoff is unavailable: not in this release",
                    ),
                )
                .await;
            })
        })
        .await;
        let outcome = connection
            .execute_daemon_handoff(|_| async { panic!("no prompt was sent") })
            .await
            .unwrap();
        assert_eq!(
            outcome,
            DaemonHandoffOutcome::Unavailable {
                reasons: vec![HandoffBlockingReason::Unsupported],
                message: "daemon handoff is unavailable: not in this release".to_owned(),
            }
        );
        // The connection is consumed.
        assert!(connection.request(Request::Ping).await.is_err());
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn execute_maps_in_progress_and_other_refusals() {
        for (code, expected_in_progress) in [
            (ErrorCode::HandoffInProgress, true),
            (ErrorCode::InvalidArgument, false),
        ] {
            let (mut connection, server_task) = connect(move |mut server| {
                tokio::spawn(async move {
                    accept_hello(&mut server).await;
                    let request_id = expect_execute(&mut server).await;
                    write_server_frame(
                        &mut server,
                        &ServerFrame::Error {
                            request_id: Some(request_id),
                            error: ProtocolError::new(code, "refused"),
                        },
                    )
                    .await;
                })
            })
            .await;
            let outcome = connection
                .execute_daemon_handoff(|_| async { HandoffDecision::Cancel })
                .await
                .unwrap();
            if expected_in_progress {
                assert_eq!(
                    outcome,
                    DaemonHandoffOutcome::InProgress {
                        message: "refused".to_owned()
                    }
                );
            } else {
                assert_eq!(
                    outcome,
                    DaemonHandoffOutcome::Refused(ProtocolError::new(code, "refused"))
                );
            }
            server_task.await.unwrap();
        }
    }

    #[tokio::test]
    async fn execute_without_a_prompt_is_handed_off_when_the_daemon_closes() {
        let (mut connection, server_task) = connect(|mut server| {
            tokio::spawn(async move {
                accept_hello(&mut server).await;
                expect_execute(&mut server).await;
                // Serve ends the connection for the exec boundary.
            })
        })
        .await;
        let outcome = connection
            .execute_daemon_handoff(|_| async { panic!("no prompt was sent") })
            .await
            .unwrap();
        server_task.await.unwrap();
        assert_eq!(
            outcome,
            DaemonHandoffOutcome::HandedOff {
                proceeded_without: Vec::new()
            }
        );
    }

    #[tokio::test]
    async fn proceed_answers_exactly_the_prompted_indices_then_hands_off() {
        let (mut connection, server_task) = connect(|mut server| {
            tokio::spawn(async move {
                accept_hello(&mut server).await;
                let request_id = expect_execute(&mut server).await;
                write_server_frame(&mut server, &prompt(request_id, 20_000)).await;
                assert_eq!(
                    read_client_frame(&mut server).await,
                    Some(ClientFrame::HandoffDecision {
                        request_id,
                        proceed_without: vec![3, 7],
                    })
                );
                // Committed: the connection closes without an answer.
            })
        })
        .await;
        let outcome = connection
            .execute_daemon_handoff(|prompt| async move {
                assert_eq!(prompt.daemon_generation, GENERATION);
                assert_eq!(prompt.deadline, Duration::from_secs(20));
                assert_eq!(prompt.respond_within, Duration::from_millis(18_500));
                assert_eq!(prompt.windows.len(), 2);
                assert_eq!(prompt.windows[0].dojos[1].name, "logs");
                HandoffDecision::ProceedWithout
            })
            .await;
        server_task.await.unwrap();
        assert_eq!(
            outcome.unwrap(),
            DaemonHandoffOutcome::HandedOff {
                proceeded_without: vec![3, 7]
            }
        );
    }

    #[tokio::test]
    async fn cancel_sends_one_cancel_and_returns_the_confirmation() {
        let (mut connection, server_task) = connect(|mut server| {
            tokio::spawn(async move {
                accept_hello(&mut server).await;
                let request_id = expect_execute(&mut server).await;
                write_server_frame(&mut server, &prompt(request_id, 20_000)).await;
                assert_eq!(
                    read_client_frame(&mut server).await,
                    Some(ClientFrame::Cancel { request_id })
                );
                write_server_frame(
                    &mut server,
                    &unavailable(
                        request_id,
                        "daemon handoff is unavailable: it was cancelled",
                    ),
                )
                .await;
                // Nothing else follows: no decision after the cancel.
                assert_eq!(read_client_frame(&mut server).await, None);
            })
        })
        .await;
        let outcome = connection
            .execute_daemon_handoff(|_| async { HandoffDecision::Cancel })
            .await
            .unwrap();
        drop(connection);
        assert_eq!(
            outcome,
            DaemonHandoffOutcome::Cancelled {
                message: "daemon handoff is unavailable: it was cancelled".to_owned()
            }
        );
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn an_unanswered_callback_cancels_before_the_deadline() {
        let (mut connection, server_task) = connect(|mut server| {
            tokio::spawn(async move {
                accept_hello(&mut server).await;
                let request_id = expect_execute(&mut server).await;
                let sent = tokio::time::Instant::now();
                write_server_frame(&mut server, &prompt(request_id, 400)).await;
                assert_eq!(
                    read_client_frame(&mut server).await,
                    Some(ClientFrame::Cancel { request_id })
                );
                // Half the 400 ms deadline, well before it ends.
                assert!(sent.elapsed() < Duration::from_millis(400));
                write_server_frame(
                    &mut server,
                    &unavailable(
                        request_id,
                        "daemon handoff is unavailable: it was cancelled",
                    ),
                )
                .await;
            })
        })
        .await;
        let outcome = connection
            .execute_daemon_handoff(|prompt| async move {
                assert_eq!(prompt.respond_within, Duration::from_millis(200));
                std::future::pending::<HandoffDecision>().await
            })
            .await
            .unwrap();
        assert!(
            matches!(outcome, DaemonHandoffOutcome::TimedOut { .. }),
            "{outcome:?}"
        );
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn dropping_the_execute_future_mid_flight_cancels_and_closes() {
        let (mut connection, server_task) = connect(|mut server| {
            tokio::spawn(async move {
                accept_hello(&mut server).await;
                let request_id = expect_execute(&mut server).await;
                write_server_frame(&mut server, &prompt(request_id, 20_000)).await;
                // The caller gives up while its callback is still deciding:
                // one Cancel for the request, then the connection closes.
                assert_eq!(
                    read_client_frame(&mut server).await,
                    Some(ClientFrame::Cancel { request_id })
                );
                assert_eq!(read_client_frame(&mut server).await, None);
            })
        })
        .await;
        let abandoned = tokio::time::timeout(
            Duration::from_millis(200),
            connection.execute_daemon_handoff(|_| std::future::pending::<HandoffDecision>()),
        )
        .await;
        assert!(abandoned.is_err(), "the execute future was not abandoned");
        server_task.await.unwrap();
        assert!(connection.request(Request::Ping).await.is_err());
    }

    #[tokio::test]
    async fn a_final_answer_while_deciding_drops_the_callback() {
        let (mut connection, server_task) = connect(|mut server| {
            tokio::spawn(async move {
                accept_hello(&mut server).await;
                let request_id = expect_execute(&mut server).await;
                write_server_frame(&mut server, &prompt(request_id, 20_000)).await;
                write_server_frame(
                    &mut server,
                    &unavailable(
                        request_id,
                        "daemon handoff is unavailable: a step missed its deadline",
                    ),
                )
                .await;
            })
        })
        .await;
        let outcome = connection
            .execute_daemon_handoff(|_| std::future::pending::<HandoffDecision>())
            .await
            .unwrap();
        assert!(
            matches!(outcome, DaemonHandoffOutcome::Unavailable { .. }),
            "{outcome:?}"
        );
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn closing_while_deciding_or_after_cancel_is_an_error() {
        let (mut connection, server_task) = connect(|mut server| {
            tokio::spawn(async move {
                accept_hello(&mut server).await;
                let request_id = expect_execute(&mut server).await;
                write_server_frame(&mut server, &prompt(request_id, 20_000)).await;
            })
        })
        .await;
        let error = connection
            .execute_daemon_handoff(|_| std::future::pending::<HandoffDecision>())
            .await
            .unwrap_err();
        server_task.await.unwrap();
        assert!(
            error.to_string().contains("waited for a decision"),
            "{error:#}"
        );

        let (mut connection, server_task) = connect(|mut server| {
            tokio::spawn(async move {
                accept_hello(&mut server).await;
                let request_id = expect_execute(&mut server).await;
                write_server_frame(&mut server, &prompt(request_id, 20_000)).await;
                assert_eq!(
                    read_client_frame(&mut server).await,
                    Some(ClientFrame::Cancel { request_id })
                );
            })
        })
        .await;
        let deciding = connection.execute_daemon_handoff(|_| async { HandoffDecision::Cancel });
        let closer = async {
            server_task.await.unwrap();
        };
        let (result, ()) = tokio::join!(deciding, closer);
        let error = result.unwrap_err();
        assert!(
            error.to_string().contains("before confirming the cancel"),
            "{error:#}"
        );
    }

    #[tokio::test]
    async fn misdirected_or_malformed_prompts_are_refused() {
        let cases: [fn(u64) -> ServerFrame; 4] = [
            // Another request's prompt.
            |request_id| match prompt(request_id, 20_000) {
                ServerFrame::HandoffDecisionRequired {
                    daemon_generation,
                    deadline_ms,
                    windows,
                    ..
                } => ServerFrame::HandoffDecisionRequired {
                    request_id: request_id + 1,
                    daemon_generation,
                    deadline_ms,
                    windows,
                },
                _ => unreachable!(),
            },
            // Another generation's prompt.
            |request_id| match prompt(request_id, 20_000) {
                ServerFrame::HandoffDecisionRequired {
                    deadline_ms,
                    windows,
                    ..
                } => ServerFrame::HandoffDecisionRequired {
                    request_id,
                    daemon_generation: GENERATION + 1,
                    deadline_ms,
                    windows,
                },
                _ => unreachable!(),
            },
            // A deadline past the protocol bound.
            |request_id| prompt(request_id, 60_001),
            // No Windows.
            |request_id| ServerFrame::HandoffDecisionRequired {
                request_id,
                daemon_generation: GENERATION,
                deadline_ms: 20_000,
                windows: Vec::new(),
            },
        ];
        for case in cases {
            let (mut connection, server_task) = connect(move |mut server| {
                tokio::spawn(async move {
                    accept_hello(&mut server).await;
                    let request_id = expect_execute(&mut server).await;
                    write_server_frame(&mut server, &case(request_id)).await;
                })
            })
            .await;
            let error = connection
                .execute_daemon_handoff(|_| async { panic!("an invalid prompt was accepted") })
                .await
                .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("invalid handoff decision prompt"),
                "{error:#}"
            );
            server_task.await.unwrap();
        }
    }

    #[tokio::test]
    async fn a_second_prompt_is_refused() {
        let (mut connection, server_task) = connect(|mut server| {
            tokio::spawn(async move {
                accept_hello(&mut server).await;
                let request_id = expect_execute(&mut server).await;
                write_server_frame(&mut server, &prompt(request_id, 20_000)).await;
                write_server_frame(&mut server, &prompt(request_id, 20_000)).await;
            })
        })
        .await;
        let error = connection
            .execute_daemon_handoff(|_| std::future::pending::<HandoffDecision>())
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("decision prompt outside a handoff"),
            "{error:#}"
        );
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn a_decision_prompt_outside_execute_is_still_refused() {
        let (mut connection, server_task) = connect(|mut server| {
            tokio::spawn(async move {
                accept_hello(&mut server).await;
                let Some(ClientFrame::Request {
                    request_id,
                    request:
                        Request::PrepareDaemonHandoff {
                            mode: DaemonHandoffMode::DryRun,
                        },
                    ..
                }) = read_client_frame(&mut server).await
                else {
                    panic!("expected a dry run");
                };
                write_server_frame(&mut server, &prompt(request_id, 20_000)).await;
            })
        })
        .await;
        let error = connection.daemon_handoff_report().await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("handoff decision prompt outside a handoff"),
            "{error:#}"
        );
        server_task.await.unwrap();
    }

    #[tokio::test]
    async fn execute_requires_a_trusted_connection() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let server_task = tokio::spawn(async move {
            let _ = read_client_frame(&mut server).await;
            write_server_frame(
                &mut server,
                &ServerFrame::Hello {
                    version: PROTOCOL_VERSION,
                    limits: ServerLimits::default(),
                    development_terminal_access: false,
                    daemon_hostname: None,
                    daemon_generation: GENERATION,
                },
            )
            .await;
            // Nothing is sent: the client refuses before writing a request.
            assert_eq!(read_client_frame(&mut server).await, None);
        });
        let mut connection = Connection::connect_stream(client, ClientRole::Automation)
            .await
            .unwrap();
        let error = connection
            .execute_daemon_handoff(|_| async { HandoffDecision::Cancel })
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("trusted local client"),
            "{error:#}"
        );
        drop(connection);
        server_task.await.unwrap();
    }

    #[test]
    fn the_decision_budget_always_ends_before_the_deadline() {
        assert_eq!(
            decision_budget(Duration::from_secs(20)),
            Duration::from_millis(18_500)
        );
        assert_eq!(
            decision_budget(Duration::from_secs(60)),
            Duration::from_millis(58_500)
        );
        assert_eq!(
            decision_budget(Duration::from_millis(2_000)),
            Duration::from_millis(1_000)
        );
        assert_eq!(
            decision_budget(Duration::from_millis(1)),
            Duration::from_nanos(500_000)
        );
    }
}
