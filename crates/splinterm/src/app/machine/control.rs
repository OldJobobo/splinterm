use super::{
    CliEnvelopeV2, CliErrorCodeV2, Connection, ControlMode, Request, Response, Result, SplintId,
    TerminalReadProvenanceV2, bail, connect_machine, finish_machine_envelope,
    live_terminal_location, require_expected_incarnation, terminal_action_envelope,
    write_machine_read_failure,
};

pub(super) enum MachineControl {
    Input(Vec<u8>),
    Resize { columns: u16, rows: u16 },
}

impl MachineControl {
    const fn operation(&self) -> &'static str {
        match self {
            Self::Input(_) => "input",
            Self::Resize { .. } => "resize",
        }
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "atomic acquire/action/release and cleanup remain adjacent for auditability"
)]
async fn machine_control_envelope(
    connection: &mut Connection,
    command: &MachineControl,
    splint_id: SplintId,
    expected_incarnation: Option<u64>,
    deadline: std::time::Duration,
    started: std::time::Instant,
) -> Result<CliEnvelopeV2> {
    if matches!(command, MachineControl::Input(bytes) if bytes.len() > connection.limits().maximum_input_bytes)
    {
        bail!("input exceeds negotiated resource limit");
    }
    let response = connection
        .request_with_deadline(
            Request::InspectTopology,
            deadline.saturating_sub(started.elapsed()),
        )
        .await?;
    let Response::Topology { snapshot: topology } = response else {
        bail!("splinterd returned an unexpected topology response");
    };
    require_expected_incarnation(&topology, splint_id, expected_incarnation)?;
    let (lair_id, dojo_id, incarnation) = live_terminal_location(&topology, splint_id)?;
    let response = connection
        .request_with_deadline(
            Request::AcquireControl {
                splint_id,
                incarnation,
                modes: vec![match command {
                    MachineControl::Input(_) => ControlMode::Input,
                    MachineControl::Resize { .. } => ControlMode::Resize,
                }],
            },
            deadline.saturating_sub(started.elapsed()),
        )
        .await?;
    let Response::ControlGranted { controller_id, .. } = response else {
        bail!("splinterd did not grant a controller lease");
    };
    if controller_id == 0 {
        bail!("splinterd returned an invalid controller lease");
    }
    let request = match command {
        MachineControl::Input(bytes) => Request::Input {
            controller_id,
            splint_id,
            incarnation,
            bytes: bytes.clone(),
        },
        MachineControl::Resize { columns, rows } => Request::Resize {
            controller_id,
            splint_id,
            incarnation,
            columns: *columns,
            rows: *rows,
            pixel_width: 0,
            pixel_height: 0,
        },
    };
    let action = connection
        .request_with_deadline(request, deadline.saturating_sub(started.elapsed()))
        .await;
    let release = connection
        .request_with_deadline(
            Request::ReleaseControl { controller_id },
            deadline.saturating_sub(started.elapsed()),
        )
        .await;
    let response = match action {
        Ok(response) => response,
        Err(error) => {
            let _ = release;
            return Err(error);
        }
    };
    if !matches!(release?, Response::Acknowledged) {
        bail!("splinterd did not release the controller lease");
    }
    let Response::TerminalActionAcknowledged {
        lair_id: response_lair,
        dojo_id: response_dojo,
        splint_id: response_splint,
        incarnation: response_incarnation,
        terminal_revision,
        history_generation,
    } = response
    else {
        bail!("splinterd returned an unexpected terminal action response");
    };
    if (
        response_lair,
        response_dojo,
        response_splint,
        response_incarnation,
    ) != (lair_id, dojo_id, splint_id, incarnation)
    {
        bail!("splinterd returned inconsistent terminal action identity");
    }
    terminal_action_envelope(
        command.operation(),
        TerminalReadProvenanceV2 {
            lair_id,
            dojo_id,
            splint_id,
            incarnation,
            terminal_revision,
            history_generation,
        },
        match command {
            MachineControl::Input(_) => None,
            MachineControl::Resize { columns, rows } => Some((*columns, *rows)),
        },
    )
}

pub(super) async fn run_machine_control(
    command: MachineControl,
    splint_id: SplintId,
    expected_incarnation: Option<u64>,
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
    let (mut connection, started) = connect_machine(operation, deadline).await?;
    let result = machine_control_envelope(
        &mut connection,
        &command,
        splint_id,
        expected_incarnation,
        deadline,
        started,
    )
    .await;
    finish_machine_envelope(operation, result)
}
