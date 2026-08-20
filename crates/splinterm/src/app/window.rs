//! Graphical window lifecycle orchestration.

use std::{collections::HashMap, env, path::PathBuf};

use anyhow::{Context, Result, bail};
use splinterm::{
    PerfTraceCorrelation, WindowOptions, WindowUpdate,
    automation::{Connection, MAX_RENDERER_IMAGE_RESIDENT_BYTES, SharedImageContentCache},
    config::AppConfig,
    endpoint::{
        ConnectionFactory, ForcedControlTransfer, GraphicalFocusPublication, LaunchSemantics,
    },
    renderer::{self, RendererOptions},
    run_window,
};
use splinterm_core::SplintId;
use splinterm_protocol::{
    ControlMode, Request, Response, ServerFrame,
    perf_trace::{PerfTraceEvent, emit_perf_trace, perf_trace_enabled},
};
use tokio::sync::{mpsc, watch};

use super::{
    pane_bridge::{
        ControllerOutputs, EventAction, WINDOW_COMMAND_QUEUE, WINDOW_UPDATE_QUEUE, attach,
        classify_subscription_event, layout_splint_ids, lease_snapshot_images, lease_update_images,
        load_authority_status, optional_pane_controller, pane_access_scopes,
        pane_claims_initial_control, prepare_live_pane, resolve_image_contents,
        resolve_update_images, resynchronize, run_controller, terminal_grid_limits,
        update_advances_from, validate_attached_snapshot,
    },
    theme_watch::{ThemeUpdateSink, load_startup_theme, watch_theme},
    topology_manager::{initial_window_dojo_identity, run_topology_manager, spawn_topology_smoke},
};

async fn run_graphical_focus_reporter(
    factory: ConnectionFactory,
    mut updates: watch::Receiver<Option<SplintId>>,
) -> Result<()> {
    let mut connection = factory.connect().await?;
    loop {
        let focused_splint_id = *updates.borrow_and_update();
        let response = connection
            .request(Request::PublishGraphicalFocus { focused_splint_id })
            .await?;
        anyhow::ensure!(
            matches!(response, Response::Acknowledged),
            "splinterd rejected graphical focus publication"
        );
        if updates.changed().await.is_err() {
            return Ok(());
        }
    }
}

fn spawn_graphical_focus_reporter(
    factory: ConnectionFactory,
    updates: watch::Receiver<Option<SplintId>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if run_graphical_focus_reporter(factory, updates)
            .await
            .is_err()
        {
            eprintln!("splinterm graphical focus reporter failed");
        }
    })
}

fn endpoint_graphical_focus(
    factory: &ConnectionFactory,
) -> (
    Option<watch::Sender<Option<SplintId>>>,
    Option<tokio::task::JoinHandle<()>>,
) {
    if factory.capabilities().graphical_focus_publication == GraphicalFocusPublication::Enabled {
        let (sender, updates) = watch::channel(None);
        (
            Some(sender),
            Some(spawn_graphical_focus_reporter(factory.clone(), updates)),
        )
    } else {
        (None, None)
    }
}

fn pane_chrome_capture() -> Result<Option<PathBuf>> {
    let Some(path) = env::var_os("SPLINTERM_PANE_CHROME_CAPTURE") else {
        return Ok(None);
    };
    anyhow::ensure!(
        env::var_os("SPLINTERM_ENABLE_DEV_ATTACH").is_some(),
        "SPLINTERM_PANE_CHROME_CAPTURE requires development attach"
    );
    Ok(Some(PathBuf::from(path)))
}

fn finish_topology_manager(
    result: std::result::Result<Result<()>, tokio::task::JoinError>,
) -> Result<()> {
    result
        .context("topology manager task failed")?
        .context("topology manager stopped")
}

fn resolved_window_app_id(app_id: Option<String>) -> String {
    app_id.unwrap_or_else(|| splinterm::config::APP_ID.to_owned())
}

pub(super) async fn run_live_multipane_window(
    config: AppConfig,
    dojo_model: splinterm_core::Dojo,
    factory: ConnectionFactory,
) -> Result<()> {
    run_live_multipane_window_inner(config, dojo_model, factory, None, None, true).await
}

pub(super) async fn run_live_multipane_window_with_app_id(
    config: AppConfig,
    dojo_model: splinterm_core::Dojo,
    factory: ConnectionFactory,
    app_id: Option<String>,
    initial_tab_strip_visible: bool,
) -> Result<()> {
    run_live_multipane_window_inner(
        config,
        dojo_model,
        factory,
        None,
        app_id,
        initial_tab_strip_visible,
    )
    .await
}

pub(super) async fn run_owned_live_multipane_window_with_app_id(
    config: AppConfig,
    dojo_model: splinterm_core::Dojo,
    factory: ConnectionFactory,
    owner: Connection,
    app_id: Option<String>,
    initial_tab_strip_visible: bool,
) -> Result<()> {
    run_live_multipane_window_inner(
        config,
        dojo_model,
        factory,
        Some(owner),
        app_id,
        initial_tab_strip_visible,
    )
    .await
}

#[allow(
    clippy::too_many_lines,
    reason = "multi-pane startup keeps renderer, topology, theme, and input authorities in one lifecycle"
)]
async fn run_live_multipane_window_inner(
    config: AppConfig,
    dojo_model: splinterm_core::Dojo,
    factory: ConnectionFactory,
    initial_transient_owner: Option<Connection>,
    app_id: Option<String>,
    initial_tab_strip_visible: bool,
) -> Result<()> {
    if let Some(diagnostics) = splinterm::diagnostics::global() {
        diagnostics.ensure_window(Some(dojo_model.id), Some(dojo_model.default_focus));
    }
    let initial_identity = initial_window_dojo_identity(&factory, dojo_model.id).await?;
    let theme = load_startup_theme(&config);
    renderer::configure(RendererOptions {
        font: config.font.clone(),
        font_size: config.font_size,
        font_sizing_policy: config.font_sizing_policy,
        physical_dpi: 96.0,
        padding: config.padding,
        background_alpha: theme.background_alpha,
    })?;
    let mut ids = Vec::new();
    layout_splint_ids(&dojo_model.root, &mut ids);
    let image_cache =
        SharedImageContentCache::with_maximum_bytes(MAX_RENDERER_IMAGE_RESIDENT_BYTES)?;
    let mut prepared = Vec::with_capacity(ids.len());
    for splint_id in ids {
        prepared.push(
            prepare_live_pane(
                &factory,
                &config,
                splint_id,
                image_cache.clone(),
                pane_claims_initial_control(splint_id, dojo_model.default_focus),
            )
            .await?,
        );
    }
    let (topology_commands, topology_command_receiver) = mpsc::channel(8);
    let (topology_update_sender, topology_updates) = mpsc::channel(4);
    let (graphical_focus, _graphical_focus_reporter) = endpoint_graphical_focus(&factory);
    let forced_control_transfer =
        factory.capabilities().forced_control_transfer == ForcedControlTransfer::Enabled;
    let optimistic_remote_splits =
        factory.capabilities().launch_semantics == LaunchSemantics::RemoteInteractive;
    let theme_task = tokio::spawn(watch_theme(
        config.theme_source(),
        config.background_alpha,
        config.background_blur,
        theme,
        ThemeUpdateSink::Topology(topology_update_sender.clone()),
    ));
    let mut panes = Vec::with_capacity(prepared.len());
    let mut tasks = HashMap::with_capacity(prepared.len());
    for pane in prepared {
        let splint_id = pane.options.snapshot.splint_id;
        panes.push(pane.options);
        tasks.insert(splint_id, pane.task);
    }
    let topology_smoke = spawn_topology_smoke(
        topology_commands.clone(),
        dojo_model.id,
        dojo_model.default_focus,
    )?;
    let window_config = config.clone();
    let root = dojo_model.root;
    let (manager_root, active_splint) = (root.clone(), dojo_model.default_focus);
    let topology_manager = tokio::spawn(run_topology_manager(
        factory,
        config,
        image_cache,
        initial_identity.clone(),
        manager_root,
        topology_command_receiver,
        topology_update_sender,
        tasks,
        initial_transient_owner,
    ));
    let result = tokio::task::spawn_blocking(move || {
        run_window(WindowOptions {
            capture: pane_chrome_capture()?,
            panes,
            layout: Some(root),
            active_splint: Some(active_splint),
            topology_updates: Some(topology_updates),
            topology_commands: Some(topology_commands),
            graphical_focus,
            forced_control_transfer,
            optimistic_remote_splits,
            initial_dojo: Some(initial_identity),
            initial_tab_strip_visible,
            initial_columns: window_config.initial_columns,
            initial_rows: window_config.initial_rows,
            cursor_style: window_config.cursor_style,
            cursor_blink: window_config.cursor_blink,
            title: window_config.title,
            app_id: resolved_window_app_id(app_id),
            theme,
            pane_divider_style: window_config.pane_divider_style,
            frame_title_mode: window_config.frame_title_mode,
            keymap: window_config.keymap,
            prefix_timeout_ms: window_config.prefix_timeout_ms,
            ..WindowOptions::default()
        })
    })
    .await
    .context("Wayland window task failed")?;
    theme_task.abort();
    if let Some(smoke) = topology_smoke {
        smoke.await.context("topology smoke task failed")??;
    }
    finish_topology_manager(topology_manager.await)?;
    result
}

#[allow(
    clippy::too_many_lines,
    reason = "subscription resync, controller ownership, and window task shutdown are one lifecycle"
)]
pub(super) async fn run_live_window(
    config: AppConfig,
    splint_id: SplintId,
    factory: ConnectionFactory,
) -> Result<()> {
    if let Some(diagnostics) = splinterm::diagnostics::global() {
        diagnostics.ensure_window(None, Some(splint_id));
    }
    let theme = load_startup_theme(&config);
    renderer::configure(RendererOptions {
        font: config.font.clone(),
        font_size: config.font_size,
        font_sizing_policy: config.font_sizing_policy,
        physical_dpi: 96.0,
        padding: config.padding,
        background_alpha: theme.background_alpha,
    })?;
    let mut connection = factory.connect().await?;
    let terminal_grid_limits = terminal_grid_limits(connection.limits());
    let incarnation = connection.live_incarnation(splint_id).await?;
    let requested_scopes = pane_access_scopes();
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
    let image_cache =
        SharedImageContentCache::with_maximum_bytes(MAX_RENDERER_IMAGE_RESIDENT_BYTES)?;
    let image_transport = factory.capabilities().image_transport;
    resolve_image_contents(
        image_transport,
        &mut connection,
        &attachment.snapshot,
        &image_cache,
    )
    .await?;
    let initial_image_sources = lease_snapshot_images(&image_cache, &attachment.snapshot)?;
    let mut control = factory.connect().await?;
    let control_incarnation = control.live_incarnation(splint_id).await?;
    if control_incarnation != incarnation {
        bail!("control connection observed a different process incarnation");
    }
    let controller_id = optional_pane_controller(
        control
            .acquire_control(
                splint_id,
                incarnation,
                vec![ControlMode::Input, ControlMode::Resize],
            )
            .await,
    )?;
    if let Some(controller_id) = controller_id {
        println!("Controller lease {controller_id} granted for live Splint");
    }
    let (updates, receiver) = mpsc::channel(WINDOW_UPDATE_QUEUE);
    let _theme_watcher = tokio::spawn(watch_theme(
        config.theme_source(),
        config.background_alpha,
        config.background_blur,
        theme,
        ThemeUpdateSink::Panes(vec![updates.clone()]),
    ));
    let (command_sender, commands) = mpsc::channel(WINDOW_COMMAND_QUEUE);
    let (resync_sender, mut resyncs) = mpsc::channel(1);
    let controller_cancellation = tokio_util::sync::CancellationToken::new();
    let mut controller = tokio::spawn(run_controller(
        control,
        commands,
        ControllerOutputs {
            updates: updates.clone(),
            resyncs: resync_sender,
        },
        controller_id,
        splint_id,
        incarnation,
        factory.capabilities().forced_control_transfer,
        config.resize_delay_ms,
        controller_cancellation,
    ));
    let mut last_revision = attachment.snapshot.revision;
    let initial_snapshot = attachment.snapshot;
    let window_config = config.clone();
    let (graphical_focus, _graphical_focus_reporter) = endpoint_graphical_focus(&factory);
    let forced_control_transfer =
        factory.capabilities().forced_control_transfer == ForcedControlTransfer::Enabled;
    let mut window = tokio::task::spawn_blocking(move || {
        run_window(WindowOptions {
            snapshot: Some(initial_snapshot),
            image_sources: initial_image_sources,
            updates: Some(receiver),
            commands: Some(command_sender),
            authority,
            controlled: controller_id.is_some(),
            graphical_focus,
            forced_control_transfer,
            terminal_grid_limits,
            initial_columns: window_config.initial_columns,
            initial_rows: window_config.initial_rows,
            cursor_style: window_config.cursor_style,
            cursor_blink: window_config.cursor_blink,
            title: window_config.title,
            theme,
            pane_divider_style: window_config.pane_divider_style,
            frame_title_mode: window_config.frame_title_mode,
            keymap: window_config.keymap,
            prefix_timeout_ms: window_config.prefix_timeout_ms,
            ..WindowOptions::default()
        })
    });
    let mut last_sequence = 0_u64;

    loop {
        tokio::select! {
            biased;
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
            Some(()) = resyncs.recv() => {
                if std::env::var_os("SPLINTERM_SCROLL_TRACE").is_some() {
                    eprintln!("scroll-trace resync=controller_page");
                }
                attachment = resynchronize(
                    &mut connection,
                    attachment.subscription_id,
                    splint_id,
                    incarnation,
                ).await?;
                resolve_image_contents(
                    image_transport, &mut connection, &attachment.snapshot, &image_cache,
                ).await?;
                let image_sources = lease_snapshot_images(&image_cache, &attachment.snapshot)?;
                if updates
                    .send(WindowUpdate::Snapshot {
                        snapshot: attachment.snapshot.clone(),
                        image_sources,
                        authoritative: true,
                    })
                    .await
                    .is_err()
                {
                    let window_result = window.await.context("Wayland window task failed")?;
                    controller.await.context("window controller task failed")??;
                    return window_result;
                }
                last_revision = attachment.snapshot.revision;
                last_sequence = 0;
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
                            validate_attached_snapshot(&snapshot, splint_id, incarnation)?;
                            last_revision = snapshot.revision;
                            resolve_image_contents(
                                image_transport, &mut connection, &snapshot, &image_cache,
                            ).await?;
                            let image_sources = lease_snapshot_images(&image_cache, &snapshot)?;
                            if updates.send(WindowUpdate::Snapshot {
                                snapshot,
                                image_sources,
                                authoritative: false,
                            }).await.is_err() {
                                let window_result = window.await.context("Wayland window task failed")?;
                                controller.await.context("window controller task failed")??;
                                return window_result;
                            }
                            last_sequence = sequence;
                        }
                        EventAction::Update { sequence, update }
                            if update_advances_from(&update, last_revision) => {
                            if perf_trace_enabled() {
                                emit_perf_trace(
                                    "splinterm",
                                    "client_receive",
                                    PerfTraceEvent {
                                        splint_id: Some(splint_id),
                                        incarnation: Some(incarnation),
                                        base_revision: Some(update.base_revision),
                                        revision: Some(update.revision),
                                        subscription_id: Some(attachment.subscription_id),
                                        transaction_sequence: Some(sequence),
                                        rows: Some(
                                            u64::try_from(update.rows.len()).unwrap_or(u64::MAX),
                                        ),
                                        count: Some(1),
                                        ..PerfTraceEvent::default()
                                    },
                                );
                            }
                            last_revision = update.revision;
                            resolve_update_images(
                                image_transport,
                                &mut connection,
                                &update,
                                splint_id,
                                incarnation,
                                &image_cache,
                            ).await?;
                            let image_sources = lease_update_images(&image_cache, &update)?;
                            let base_revision = update.base_revision;
                            let revision = update.revision;
                            if updates
                                .send(WindowUpdate::Update {
                                    update,
                                    image_sources,
                                    trace: perf_trace_enabled().then_some(PerfTraceCorrelation {
                                        base_revision,
                                        revision,
                                        subscription_id: attachment.subscription_id,
                                        transaction_sequence: sequence,
                                    }),
                                })
                                .await
                                .is_err()
                            {
                                let window_result =
                                    window.await.context("Wayland window task failed")?;
                                controller.await.context("window controller task failed")??;
                                return window_result;
                            }
                            last_sequence = sequence;
                        }
                        EventAction::Update { update, .. } => {
                            if std::env::var_os("SPLINTERM_SCROLL_TRACE").is_some() {
                                eprintln!(
                                    "scroll-trace resync=revision last={} base={} final={}",
                                    last_revision, update.base_revision, update.revision
                                );
                            }
                            attachment = resynchronize(
                                &mut connection,
                                attachment.subscription_id,
                                splint_id,
                                incarnation,
                            ).await?;
                            resolve_image_contents(
                                image_transport, &mut connection, &attachment.snapshot, &image_cache,
                            ).await?;
                            let image_sources = lease_snapshot_images(&image_cache, &attachment.snapshot)?;
                            if updates
                                .send(WindowUpdate::Snapshot {
                                    snapshot: attachment.snapshot.clone(),
                                    image_sources,
                                    authoritative: true,
                                })
                                .await
                                .is_err()
                            {
                                let window_result = window.await.context("Wayland window task failed")?;
                                controller.await.context("window controller task failed")??;
                                return window_result;
                            }
                            last_revision = attachment.snapshot.revision;
                            last_sequence = 0;
                        }
                        EventAction::Resynchronize => {
                            if std::env::var_os("SPLINTERM_SCROLL_TRACE").is_some() {
                                eprintln!(
                                    "scroll-trace resync=subscription_sequence last_sequence={last_sequence} received_sequence={sequence}"
                                );
                            }
                            attachment = resynchronize(
                                &mut connection,
                                attachment.subscription_id,
                                splint_id,
                                incarnation,
                            ).await?;
                            resolve_image_contents(
                                image_transport, &mut connection, &attachment.snapshot, &image_cache,
                            ).await?;
                            let image_sources = lease_snapshot_images(&image_cache, &attachment.snapshot)?;
                            if updates
                                .send(WindowUpdate::Snapshot {
                                    snapshot: attachment.snapshot.clone(),
                                    image_sources,
                                    authoritative: true,
                                })
                                .await
                                .is_err()
                            {
                                let window_result = window.await.context("Wayland window task failed")?;
                                controller.await.context("window controller task failed")??;
                                return window_result;
                            }
                            last_revision = attachment.snapshot.revision;
                            last_sequence = 0;
                        }
                        EventAction::Exited | EventAction::Shutdown => {
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

#[cfg(test)]
mod tests {
    use super::resolved_window_app_id;

    #[test]
    fn xdg_window_identity_defaults_and_preserves_exact_override() {
        assert_eq!(resolved_window_app_id(None), splinterm::config::APP_ID);
        assert_eq!(
            resolved_window_app_id(Some("org.omarchy.screensaver".to_owned())),
            "org.omarchy.screensaver"
        );
    }
}
