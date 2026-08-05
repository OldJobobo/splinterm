use super::{
    CliEnvelopeV2, CliErrorCodeV2, Connection, Context, DojoId, LairId, PingEnvelopeV2,
    ReadResyncReasonV2, Request, Response, Result, SplintId, TerminalContinuationV2,
    TerminalReadProvenanceV2, bail, bounded_public_message, decode_terminal_cursor, protocol_error,
    public_error_code, read_resync_envelope, require_expected_incarnation,
    scrollback_page_envelope, search_page_envelope, terminal_snapshot_envelope,
    write_json_document, write_machine_connection_failure, write_machine_read_failure,
};

pub(super) enum MachineHistory {
    Scrollback {
        cursor: Option<String>,
        max_rows: usize,
    },
    Search {
        query: String,
        case_sensitive: bool,
        cursor: Option<String>,
        max_results: usize,
    },
}

impl MachineHistory {
    const fn operation(&self) -> &'static str {
        match self {
            Self::Scrollback { .. } => "scrollback_page",
            Self::Search { .. } => "search_scrollback",
        }
    }

    fn cursor(&self) -> Option<&str> {
        match self {
            Self::Scrollback { cursor, .. } | Self::Search { cursor, .. } => cursor.as_deref(),
        }
    }
}

struct MachineHistoryContext {
    provenance: TerminalReadProvenanceV2,
    before_row_id: Option<u64>,
    daemon_cursor: Option<String>,
}

fn history_cursor_context(
    command: &MachineHistory,
    encoded: &str,
    lair_id: LairId,
    dojo_id: DojoId,
    splint_id: SplintId,
    incarnation: u64,
) -> Result<MachineHistoryContext> {
    let cursor = decode_terminal_cursor(encoded).context("invalid continuation cursor")?;
    let (cursor_splint, cursor_incarnation, revision, generation, before, daemon) = match cursor {
        TerminalContinuationV2::Scrollback {
            splint_id,
            incarnation,
            terminal_revision,
            history_generation,
            before_row_id,
        } if matches!(command, MachineHistory::Scrollback { .. }) => (
            splint_id,
            incarnation,
            terminal_revision,
            history_generation,
            Some(before_row_id),
            None,
        ),
        TerminalContinuationV2::Search {
            splint_id,
            incarnation,
            terminal_revision,
            history_generation,
            daemon_cursor,
        } if matches!(command, MachineHistory::Search { .. }) => (
            splint_id,
            incarnation,
            terminal_revision,
            history_generation,
            None,
            Some(daemon_cursor),
        ),
        _ => bail!("continuation cursor does not match the requested operation"),
    };
    if cursor_splint != splint_id || cursor_incarnation != incarnation {
        bail!("continuation cursor does not match the selected live Splint");
    }
    Ok(MachineHistoryContext {
        provenance: TerminalReadProvenanceV2 {
            lair_id,
            dojo_id,
            splint_id,
            incarnation,
            terminal_revision: revision,
            history_generation: generation,
        },
        before_row_id: before,
        daemon_cursor: daemon,
    })
}

pub(super) fn live_terminal_location(
    topology: &splinterm_protocol::TopologySnapshot,
    splint_id: SplintId,
) -> Result<(LairId, DojoId, u64)> {
    topology
        .validate()
        .map_err(|error| anyhow::anyhow!(error.message))?;
    let (lair_id, dojo_id) = topology
        .topology
        .lairs()
        .find_map(|lair| {
            lair.dojos
                .iter()
                .find(|dojo| dojo.root.find_splint(splint_id).is_some())
                .map(|dojo| (lair.id, dojo.id))
        })
        .context("requested Splint was not found")?;
    let incarnation = topology
        .runtimes
        .iter()
        .find(|runtime| runtime.splint_id == splint_id)
        .context("validated topology omitted Splint runtime")?
        .live_incarnation
        .context("selected Splint does not have a live process")?;
    Ok((lair_id, dojo_id, incarnation))
}

async fn machine_history_context(
    connection: &mut Connection,
    command: &MachineHistory,
    splint_id: SplintId,
    deadline: std::time::Duration,
    started: std::time::Instant,
) -> Result<MachineHistoryContext> {
    let response = connection
        .request_with_deadline(
            Request::InspectTopology,
            deadline.saturating_sub(started.elapsed()),
        )
        .await?;
    let Response::Topology { snapshot: topology } = response else {
        bail!("splinterd returned an unexpected topology response");
    };
    let (lair_id, dojo_id, incarnation) = live_terminal_location(&topology, splint_id)?;
    if let Some(encoded) = command.cursor() {
        return history_cursor_context(command, encoded, lair_id, dojo_id, splint_id, incarnation);
    }
    let response = connection
        .request_with_deadline(
            Request::Attach {
                splint_id,
                incarnation: Some(incarnation),
                scrollback_rows: 0,
            },
            deadline.saturating_sub(started.elapsed()),
        )
        .await?;
    let Response::Attached {
        subscription_id,
        snapshot,
        ..
    } = response
    else {
        bail!("splinterd returned an unexpected attach response");
    };
    snapshot
        .validate()
        .map_err(|error| anyhow::anyhow!(error.message))?;
    if subscription_id == 0
        || snapshot.splint_id != splint_id
        || snapshot.incarnation != incarnation
    {
        bail!("splinterd returned inconsistent terminal identity");
    }
    let before_row_id = snapshot
        .newest_available_scrollback_row_id
        .unwrap_or(1)
        .checked_add(1)
        .context("scrollback row identity exhausted")?;
    let detached = connection
        .request_with_deadline(
            Request::Detach { subscription_id },
            deadline.saturating_sub(started.elapsed()),
        )
        .await?;
    if !matches!(detached, Response::Acknowledged) {
        bail!("splinterd did not detach the history bootstrap subscription");
    }
    Ok(MachineHistoryContext {
        provenance: TerminalReadProvenanceV2 {
            lair_id,
            dojo_id,
            splint_id,
            incarnation,
            terminal_revision: snapshot.revision,
            history_generation: snapshot.history_generation,
        },
        before_row_id: Some(before_row_id),
        daemon_cursor: None,
    })
}

#[allow(
    clippy::too_many_lines,
    reason = "the closed scrollback/search response matrix remains contiguous for protocol review"
)]
async fn machine_history_envelope(
    connection: &mut Connection,
    command: &MachineHistory,
    splint_id: SplintId,
    deadline: std::time::Duration,
    started: std::time::Instant,
) -> Result<CliEnvelopeV2> {
    let context =
        machine_history_context(connection, command, splint_id, deadline, started).await?;
    let request = match command {
        MachineHistory::Scrollback { max_rows, .. } => Request::ScrollbackPage {
            splint_id,
            incarnation: context.provenance.incarnation,
            terminal_revision: context.provenance.terminal_revision,
            history_generation: context.provenance.history_generation,
            before_row_id: context
                .before_row_id
                .context("scrollback cursor omitted row identity")?,
            max_rows: *max_rows,
        },
        MachineHistory::Search {
            query,
            case_sensitive,
            max_results,
            ..
        } => Request::SearchScrollback {
            splint_id,
            incarnation: context.provenance.incarnation,
            terminal_revision: context.provenance.terminal_revision,
            history_generation: context.provenance.history_generation,
            query: query.clone(),
            case_sensitive: *case_sensitive,
            cursor: context.daemon_cursor.clone(),
            max_results: *max_results,
        },
    };
    let response = connection
        .request_with_deadline(request, deadline.saturating_sub(started.elapsed()))
        .await?;
    match response {
        Response::ScrollbackPage { page, .. }
            if matches!(command, MachineHistory::Scrollback { .. }) =>
        {
            if page.splint_id != splint_id
                || page.incarnation != context.provenance.incarnation
                || page.terminal_revision != context.provenance.terminal_revision
                || page.history_generation != context.provenance.history_generation
            {
                bail!("splinterd returned inconsistent scrollback provenance");
            }
            scrollback_page_envelope(
                context.provenance.lair_id,
                context.provenance.dojo_id,
                &page,
            )
        }
        Response::SearchResults { page, .. }
            if matches!(command, MachineHistory::Search { .. }) =>
        {
            if page.splint_id != splint_id
                || page.incarnation != context.provenance.incarnation
                || page.terminal_revision != context.provenance.terminal_revision
                || page.history_generation != context.provenance.history_generation
            {
                bail!("splinterd returned inconsistent search provenance");
            }
            search_page_envelope(
                context.provenance.lair_id,
                context.provenance.dojo_id,
                &page,
            )
        }
        Response::ScrollbackResyncRequired {
            current_revision,
            history_generation,
            ..
        } if matches!(command, MachineHistory::Scrollback { .. }) => read_resync_envelope(
            command.operation(),
            TerminalReadProvenanceV2 {
                terminal_revision: current_revision,
                history_generation,
                ..context.provenance
            },
            if history_generation == context.provenance.history_generation {
                ReadResyncReasonV2::StaleRevision
            } else {
                ReadResyncReasonV2::HistoryReplaced
            },
        ),
        Response::SearchResyncRequired {
            current_revision,
            history_generation,
            ..
        } if matches!(command, MachineHistory::Search { .. }) => read_resync_envelope(
            command.operation(),
            TerminalReadProvenanceV2 {
                terminal_revision: current_revision,
                history_generation,
                ..context.provenance
            },
            if history_generation == context.provenance.history_generation {
                ReadResyncReasonV2::StaleRevision
            } else {
                ReadResyncReasonV2::HistoryReplaced
            },
        ),
        _ => bail!("splinterd returned an unexpected history response"),
    }
}

pub(super) async fn run_machine_history(
    command: MachineHistory,
    splint_id: SplintId,
    schema_major: u16,
    timeout_ms: u64,
) -> Result<()> {
    let operation = command.operation();
    if schema_major != 2 {
        write_machine_read_failure(
            operation,
            CliErrorCodeV2::UnsupportedSchema,
            format!("unsupported schema major {schema_major}"),
            false,
        )?;
        bail!("unsupported schema major {schema_major}");
    }
    let deadline = std::time::Duration::from_millis(timeout_ms);
    let started = std::time::Instant::now();
    let mut connection =
        match tokio::time::timeout(deadline, Connection::connect_automation()).await {
            Ok(Ok(connection)) => connection,
            Ok(Err(error)) => {
                write_machine_connection_failure(operation, &error)?;
                return Err(error);
            }
            Err(_) => {
                write_machine_read_failure(
                    operation,
                    CliErrorCodeV2::Timeout,
                    "connection deadline elapsed",
                    true,
                )?;
                bail!("splinterd connection timed out");
            }
        };
    match machine_history_envelope(&mut connection, &command, splint_id, deadline, started).await {
        Ok(envelope) => write_json_document(&envelope),
        Err(error) => {
            let (code, retryable) = if error.to_string().contains("timed out") {
                (CliErrorCodeV2::Timeout, true)
            } else if let Some(protocol) = protocol_error(&error) {
                public_error_code(protocol.code)
            } else if error.to_string().contains("continuation cursor") {
                (CliErrorCodeV2::InvalidArgument, false)
            } else if error.to_string().contains("not found") {
                (CliErrorCodeV2::NotFound, false)
            } else {
                (CliErrorCodeV2::Internal, false)
            };
            write_machine_read_failure(operation, code, bounded_public_message(&error), retryable)?;
            Err(error)
        }
    }
}

async fn machine_snapshot_envelope(
    connection: &mut Connection,
    splint_id: SplintId,
    expected_incarnation: Option<u64>,
    deadline: std::time::Duration,
    started: std::time::Instant,
) -> Result<CliEnvelopeV2> {
    let topology = connection
        .request_with_deadline(
            Request::InspectTopology,
            deadline.saturating_sub(started.elapsed()),
        )
        .await?;
    let Response::Topology { snapshot: topology } = topology else {
        bail!("splinterd returned an unexpected topology response");
    };
    topology
        .validate()
        .map_err(|error| anyhow::anyhow!(error.message))?;
    require_expected_incarnation(&topology, splint_id, expected_incarnation)?;
    let mut identity = None;
    for lair in topology.topology.lairs() {
        for dojo in &lair.dojos {
            if dojo.root.find_splint(splint_id).is_some() {
                let runtime = topology
                    .runtimes
                    .iter()
                    .find(|runtime| runtime.splint_id == splint_id)
                    .context("validated topology omitted Splint runtime")?;
                identity = Some((
                    lair.id,
                    dojo.id,
                    runtime
                        .live_incarnation
                        .context("selected Splint does not have a live process")?,
                ));
            }
        }
    }
    let (lair_id, dojo_id, incarnation) = identity.context("requested Splint was not found")?;
    let attached = connection
        .request_with_deadline(
            Request::Attach {
                splint_id,
                incarnation: Some(incarnation),
                scrollback_rows: 0,
            },
            deadline.saturating_sub(started.elapsed()),
        )
        .await?;
    let Response::Attached {
        subscription_id,
        snapshot,
        ..
    } = attached
    else {
        bail!("splinterd returned an unexpected attach response");
    };
    if subscription_id == 0
        || snapshot.splint_id != splint_id
        || snapshot.incarnation != incarnation
    {
        bail!("splinterd returned inconsistent terminal identity");
    }
    let detached = connection
        .request_with_deadline(
            Request::Detach { subscription_id },
            deadline.saturating_sub(started.elapsed()),
        )
        .await?;
    if !matches!(detached, Response::Acknowledged) {
        bail!("splinterd did not detach the one-shot terminal subscription");
    }
    terminal_snapshot_envelope(lair_id, dojo_id, &snapshot)
}

pub(super) async fn run_machine_snapshot(
    splint_id: SplintId,
    expected_incarnation: Option<u64>,
    schema_major: u16,
    timeout_ms: u64,
) -> Result<()> {
    const OPERATION: &str = "terminal_snapshot";
    if schema_major != 2 {
        write_machine_read_failure(
            OPERATION,
            CliErrorCodeV2::UnsupportedSchema,
            format!("unsupported schema major {schema_major}"),
            false,
        )?;
        bail!("unsupported schema major {schema_major}");
    }
    let deadline = std::time::Duration::from_millis(timeout_ms);
    let started = std::time::Instant::now();
    let mut connection =
        match tokio::time::timeout(deadline, Connection::connect_automation()).await {
            Ok(Ok(connection)) => connection,
            Ok(Err(error)) => {
                write_machine_connection_failure(OPERATION, &error)?;
                return Err(error);
            }
            Err(_) => {
                write_machine_read_failure(
                    OPERATION,
                    CliErrorCodeV2::Timeout,
                    "connection deadline elapsed",
                    true,
                )?;
                bail!("splinterd connection timed out");
            }
        };
    let result = machine_snapshot_envelope(
        &mut connection,
        splint_id,
        expected_incarnation,
        deadline,
        started,
    )
    .await;
    match result {
        Ok(envelope) => write_json_document(&envelope),
        Err(error) => {
            let (code, retryable) = if error.to_string().contains("timed out") {
                (CliErrorCodeV2::Timeout, true)
            } else if let Some(protocol) = protocol_error(&error) {
                public_error_code(protocol.code)
            } else if error.to_string().contains("not found") {
                (CliErrorCodeV2::NotFound, false)
            } else if error.to_string().contains("does not have a live process")
                || error.to_string().contains("expected incarnation")
            {
                (CliErrorCodeV2::StaleIncarnation, false)
            } else {
                (CliErrorCodeV2::Internal, false)
            };
            write_machine_read_failure(OPERATION, code, bounded_public_message(&error), retryable)?;
            Err(error)
        }
    }
}

pub(super) async fn run_machine_ping(schema_major: u16, timeout_ms: u64) -> Result<()> {
    if schema_major != 2 {
        let envelope = PingEnvelopeV2::failure(
            1,
            CliErrorCodeV2::UnsupportedSchema,
            format!("unsupported schema major {schema_major}"),
            false,
        )?;
        write_json_document(&envelope)?;
        bail!("unsupported schema major {schema_major}");
    }

    let deadline = std::time::Duration::from_millis(timeout_ms);
    let started = std::time::Instant::now();
    let mut connection =
        match tokio::time::timeout(deadline, Connection::connect_automation()).await {
            Ok(Ok(connection)) => connection,
            Ok(Err(error)) => {
                let (code, retryable) = protocol_error(&error)
                    .map_or((CliErrorCodeV2::Internal, true), |protocol| {
                        public_error_code(protocol.code)
                    });
                write_json_document(&PingEnvelopeV2::failure(
                    1,
                    code,
                    bounded_public_message(&error),
                    retryable,
                )?)?;
                return Err(error);
            }
            Err(_) => {
                write_json_document(&PingEnvelopeV2::failure(
                    1,
                    CliErrorCodeV2::Timeout,
                    "connection deadline elapsed",
                    true,
                )?)?;
                bail!("splinterd connection timed out");
            }
        };
    let remaining = deadline.saturating_sub(started.elapsed());
    match connection
        .request_with_deadline(Request::Ping, remaining)
        .await
    {
        Ok(Response::Pong) => write_json_document(&PingEnvelopeV2::success(1)?),
        Ok(_) => {
            write_json_document(&PingEnvelopeV2::failure(
                1,
                CliErrorCodeV2::Internal,
                "splinterd returned an unexpected ping response",
                false,
            )?)?;
            bail!("splinterd returned an unexpected ping response")
        }
        Err(error) => {
            let timed_out = error.to_string().contains("timed out");
            let code = if timed_out {
                CliErrorCodeV2::Timeout
            } else {
                CliErrorCodeV2::Internal
            };
            write_json_document(&PingEnvelopeV2::failure(
                1,
                code,
                bounded_public_message(&error),
                true,
            )?)?;
            Err(error)
        }
    }
}
