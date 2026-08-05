use super::{
    CliEventV2, Connection, Context, HashMap, HistoryTransition, Request, Response, Result,
    ResyncReasonV2, ServerFrame, SplintId, SubscribeCommand, SubscriptionEvent, bail,
    require_incarnation, response_protocol_error, write_json_document,
};

async fn next_private_event(connection: &mut Connection) -> Result<(u64, u64, SubscriptionEvent)> {
    match connection.next_server_frame().await? {
        ServerFrame::Event {
            subscription_id,
            sequence,
            event,
        } => Ok((subscription_id, sequence, event)),
        ServerFrame::Error { error, .. } => Err(response_protocol_error(error)),
        _ => bail!("splinterd sent an unexpected subscription frame"),
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "terminal sequence, revision, history, and termination state stay adjacent"
)]
async fn run_terminal_subscription(
    connection: &mut Connection,
    splint_id: SplintId,
    expected_incarnation: Option<u64>,
    setup_deadline: std::time::Duration,
) -> Result<()> {
    let runtime = connection
        .request_with_deadline(Request::InspectSplint { splint_id }, setup_deadline)
        .await?;
    let Response::Splint { runtime, .. } = runtime else {
        bail!("splinterd did not return the selected Splint identity");
    };
    let incarnation = runtime
        .live_incarnation
        .context("selected Splint does not have a live process")?;
    require_incarnation(incarnation, expected_incarnation)?;
    let response = connection
        .request_with_deadline(
            Request::Attach {
                splint_id,
                incarnation: Some(incarnation),
                scrollback_rows: 0,
            },
            setup_deadline,
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
    if snapshot.splint_id != splint_id || snapshot.incarnation != incarnation {
        bail!("splinterd returned inconsistent terminal subscription identity");
    }
    write_json_document(&CliEventV2::terminal_snapshot(1, 1, &snapshot, false)?)?;
    let mut public_sequence = 1_u64;
    let mut private_sequence = 1_u64;
    let mut revision = snapshot.revision;
    let mut history_generation = snapshot.history_generation;
    let mut columns = snapshot.columns;
    let mut rows = snapshot.rows;
    loop {
        let (event_subscription, sequence, event) = tokio::select! {
            result = next_private_event(connection) => result?,
            _ = tokio::signal::ctrl_c() => return Ok(()),
        };
        public_sequence = public_sequence
            .checked_add(1)
            .context("public sequence exhausted")?;
        if event_subscription != subscription_id {
            bail!("splinterd sent an event for the wrong subscription");
        }
        if sequence != private_sequence {
            write_json_document(&CliEventV2::terminal_resync(
                1,
                public_sequence,
                splint_id,
                incarnation,
                revision,
                Some(history_generation),
                ResyncReasonV2::RevisionGap,
            )?)?;
            return Ok(());
        }
        private_sequence = private_sequence
            .checked_add(1)
            .context("private sequence exhausted")?;
        match event {
            SubscriptionEvent::Snapshot { snapshot } => {
                if snapshot.splint_id != splint_id || snapshot.incarnation != incarnation {
                    bail!("terminal subscription snapshot identity changed");
                }
                revision = snapshot.revision;
                history_generation = snapshot.history_generation;
                columns = snapshot.columns;
                rows = snapshot.rows;
                write_json_document(&CliEventV2::terminal_snapshot(
                    1,
                    public_sequence,
                    &snapshot,
                    false,
                )?)?;
            }
            SubscriptionEvent::Update { update } => {
                update
                    .validate_against(revision, history_generation, columns, rows)
                    .map_err(|error| anyhow::anyhow!(error.message))?;
                revision = update.revision;
                columns = update.columns.unwrap_or(columns);
                rows = update.row_count.unwrap_or(rows);
                if let Some(scrollback) = &update.scrollback {
                    history_generation = scrollback.history_generation;
                    if !matches!(scrollback.transition, HistoryTransition::Append { .. }) {
                        write_json_document(&CliEventV2::terminal_resync(
                            1,
                            public_sequence,
                            splint_id,
                            incarnation,
                            revision,
                            Some(history_generation),
                            ResyncReasonV2::HistoryReplaced,
                        )?)?;
                        return Ok(());
                    }
                }
                write_json_document(&CliEventV2::terminal_update(
                    1,
                    public_sequence,
                    splint_id,
                    incarnation,
                    revision,
                    history_generation,
                )?)?;
            }
            SubscriptionEvent::ResyncRequired { current_revision } => {
                write_json_document(&CliEventV2::terminal_resync(
                    1,
                    public_sequence,
                    splint_id,
                    incarnation,
                    current_revision,
                    Some(history_generation),
                    ResyncReasonV2::SubscriberStalled,
                )?)?;
                return Ok(());
            }
            SubscriptionEvent::AccessRevoked { grant_id } => {
                write_json_document(&CliEventV2::access_revoked(
                    1,
                    public_sequence,
                    splint_id,
                    incarnation,
                    grant_id,
                )?)?;
                return Ok(());
            }
            SubscriptionEvent::Exited { code, signal } => {
                write_json_document(&CliEventV2::exited(
                    1,
                    public_sequence,
                    splint_id,
                    incarnation,
                    code,
                    signal,
                )?)?;
                return Ok(());
            }
            _ => bail!("splinterd sent a non-terminal event on a terminal subscription"),
        }
    }
}

async fn run_topology_subscription(
    connection: &mut Connection,
    setup_deadline: std::time::Duration,
) -> Result<()> {
    let response = connection
        .request_with_deadline(Request::SubscribeTopology, setup_deadline)
        .await?;
    let Response::TopologySubscribed {
        subscription_id,
        snapshot,
    } = response
    else {
        bail!("splinterd returned an unexpected topology subscription response");
    };
    write_json_document(&CliEventV2::topology_snapshot(1, 1, &snapshot)?)?;
    let mut public_sequence = 1_u64;
    let mut private_sequence = 1_u64;
    let mut revision = snapshot.revision;
    loop {
        let (event_subscription, sequence, event) = tokio::select! {
            result = next_private_event(connection) => result?,
            _ = tokio::signal::ctrl_c() => return Ok(()),
        };
        public_sequence = public_sequence
            .checked_add(1)
            .context("public sequence exhausted")?;
        if event_subscription != subscription_id {
            bail!("splinterd sent an event for the wrong subscription");
        }
        let event_revision = match &event {
            SubscriptionEvent::TopologyChanged { change } => change.revision,
            SubscriptionEvent::TopologyResyncRequired { current_revision } => *current_revision,
            _ => bail!("splinterd sent a non-topology event on a topology subscription"),
        };
        if sequence != private_sequence {
            write_json_document(&CliEventV2::topology_resync(
                1,
                public_sequence,
                event_revision,
                ResyncReasonV2::RevisionGap,
            )?)?;
            return Ok(());
        }
        private_sequence = private_sequence
            .checked_add(1)
            .context("private sequence exhausted")?;
        match event {
            SubscriptionEvent::TopologyChanged { change } => {
                if change.revision <= revision {
                    bail!("topology subscription revision did not advance");
                }
                change
                    .validate()
                    .map_err(|error| anyhow::anyhow!(error.message))?;
                revision = change.revision;
                write_json_document(&CliEventV2::topology_changed(
                    1,
                    public_sequence,
                    change.kind,
                    &change.snapshot,
                )?)?;
            }
            SubscriptionEvent::TopologyResyncRequired { current_revision } => {
                write_json_document(&CliEventV2::topology_resync(
                    1,
                    public_sequence,
                    current_revision,
                    ResyncReasonV2::SubscriberStalled,
                )?)?;
                return Ok(());
            }
            _ => unreachable!("topology event checked above"),
        }
    }
}

async fn run_control_subscription(
    connection: &mut Connection,
    splint_id: SplintId,
    expected_incarnation: Option<u64>,
    setup_deadline: std::time::Duration,
) -> Result<()> {
    let runtime = connection
        .request_with_deadline(Request::InspectSplint { splint_id }, setup_deadline)
        .await?;
    let Response::Splint { runtime, .. } = runtime else {
        bail!("splinterd did not return the selected Splint identity");
    };
    let incarnation = runtime
        .live_incarnation
        .context("selected Splint does not have a live process")?;
    require_incarnation(incarnation, expected_incarnation)?;
    let response = connection
        .request_with_deadline(
            Request::SubscribeControl {
                splint_id,
                incarnation,
            },
            setup_deadline,
        )
        .await?;
    let Response::ControlSubscribed {
        subscription_id,
        status,
    } = response
    else {
        bail!("splinterd returned an unexpected control subscription response");
    };
    write_json_document(&CliEventV2::control_snapshot(1, 1, status)?)?;
    let mut public_sequence = 1_u64;
    let mut private_sequence = 1_u64;
    let mut transfer_ids = HashMap::<u64, u64>::new();
    let mut next_transfer_id = 1_u64;
    loop {
        let (event_subscription, sequence, event) = tokio::select! {
            result = next_private_event(connection) => result?,
            _ = tokio::signal::ctrl_c() => return Ok(()),
        };
        public_sequence = public_sequence
            .checked_add(1)
            .context("public sequence exhausted")?;
        if event_subscription != subscription_id {
            bail!("splinterd sent an event for the wrong subscription");
        }
        if sequence != private_sequence {
            write_json_document(&CliEventV2::control_resync(
                1,
                public_sequence,
                splint_id,
                incarnation,
                ResyncReasonV2::RevisionGap,
            )?)?;
            return Ok(());
        }
        private_sequence = private_sequence
            .checked_add(1)
            .context("private sequence exhausted")?;
        let record = match event {
            SubscriptionEvent::ControlStatusChanged { status } => {
                CliEventV2::control_status_changed(1, public_sequence, status)?
            }
            SubscriptionEvent::ControlTransferRequested { transfer_id } => {
                if transfer_ids.len() >= 64 || transfer_ids.contains_key(&transfer_id) {
                    bail!("control transfer map is full or contains a duplicate private ID");
                }
                let public_transfer_id = next_transfer_id;
                next_transfer_id = next_transfer_id
                    .checked_add(1)
                    .context("public transfer ID space exhausted")?;
                transfer_ids.insert(transfer_id, public_transfer_id);
                CliEventV2::control_transfer_requested(
                    1,
                    public_sequence,
                    splint_id,
                    incarnation,
                    public_transfer_id,
                )?
            }
            SubscriptionEvent::ControlTransferResolved {
                transfer_id,
                outcome,
                ..
            } => {
                let public_transfer_id = transfer_ids
                    .remove(&transfer_id)
                    .context("control transfer resolution has no public request mapping")?;
                CliEventV2::control_transfer_resolved(
                    1,
                    public_sequence,
                    splint_id,
                    incarnation,
                    public_transfer_id,
                    outcome,
                )?
            }
            _ => bail!("splinterd sent a non-control event on a control subscription"),
        };
        write_json_document(&record)?;
    }
}

pub(in crate::app) async fn run_machine_subscription(
    stream: SubscribeCommand,
    schema_major: u16,
    timeout_ms: u64,
) -> Result<()> {
    if schema_major != 2 {
        bail!("unsupported schema major {schema_major}");
    }
    let setup_deadline = std::time::Duration::from_millis(timeout_ms);
    let mut connection = tokio::time::timeout(setup_deadline, Connection::connect_automation())
        .await
        .context("subscription connection deadline elapsed")??;
    match stream {
        SubscribeCommand::Terminal {
            splint_id,
            expected_incarnation,
        } => {
            run_terminal_subscription(
                &mut connection,
                splint_id,
                expected_incarnation,
                setup_deadline,
            )
            .await
        }
        SubscribeCommand::Topology => {
            run_topology_subscription(&mut connection, setup_deadline).await
        }
        SubscribeCommand::Control {
            splint_id,
            expected_incarnation,
        } => {
            run_control_subscription(
                &mut connection,
                splint_id,
                expected_incarnation,
                setup_deadline,
            )
            .await
        }
    }
}
