//! Graphical window lifecycle orchestration.

use std::{collections::HashMap, env, path::PathBuf};

use anyhow::{Context, Result, bail};
use splinterm::{
    PerfTraceCorrelation, WindowOptions, WindowUpdate,
    automation::{Connection, MAX_RENDERER_IMAGE_RESIDENT_BYTES, SharedImageContentCache},
    config::AppConfig,
    renderer::{self, RendererOptions},
    run_window,
};
use splinterm_core::SplintId;
use splinterm_protocol::{
    AccessScope, ControlMode, Request, Response, ServerFrame,
    perf_trace::{PerfTraceEvent, emit_perf_trace, perf_trace_enabled},
};
use tokio::sync::{mpsc, watch};

use super::{
    pane_bridge::{
        ControllerOutputs, EventAction, WINDOW_COMMAND_QUEUE, WINDOW_UPDATE_QUEUE, attach,
        classify_subscription_event, layout_splint_ids, lease_snapshot_images, lease_update_images,
        load_authority_status, pane_claims_initial_control, prepare_live_pane,
        resolve_image_contents, resolve_update_images, resynchronize, run_controller,
        update_advances_from, validate_attached_snapshot,
    },
    theme_watch::{ThemeUpdateSink, load_startup_theme, watch_theme},
    topology_manager::{initial_window_dojo_identity, run_topology_manager, spawn_topology_smoke},
};

async fn run_graphical_focus_reporter(
    mut updates: watch::Receiver<Option<SplintId>>,
) -> Result<()> {
    let mut connection = Connection::connect().await?;
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
    updates: watch::Receiver<Option<SplintId>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(error) = run_graphical_focus_reporter(updates).await {
            eprintln!("splinterm graphical focus reporter: {error:#}");
        }
    })
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

pub(super) async fn run_live_multipane_window(
    config: AppConfig,
    dojo_model: splinterm_core::Dojo,
) -> Result<()> {
    let initial_identity = initial_window_dojo_identity(dojo_model.id).await?;
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
    let (graphical_focus, graphical_focus_updates) = watch::channel(None);
    let _graphical_focus_reporter = spawn_graphical_focus_reporter(graphical_focus_updates);
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
    let manager_root = root.clone();
    let active_splint = dojo_model.default_focus;
    let topology_manager = tokio::spawn(run_topology_manager(
        config,
        image_cache,
        initial_identity.clone(),
        manager_root,
        topology_command_receiver,
        topology_update_sender,
        tasks,
    ));
    let result = tokio::task::spawn_blocking(move || {
        run_window(WindowOptions {
            capture: pane_chrome_capture()?,
            panes,
            layout: Some(root),
            active_splint: Some(active_splint),
            topology_updates: Some(topology_updates),
            topology_commands: Some(topology_commands),
            graphical_focus: Some(graphical_focus),
            initial_dojo: Some(initial_identity),
            initial_columns: window_config.initial_columns,
            initial_rows: window_config.initial_rows,
            cursor_style: window_config.cursor_style,
            cursor_blink: window_config.cursor_blink,
            title: window_config.title,
            theme,
            pane_divider_style: window_config.pane_divider_style,
            frame_title_mode: window_config.frame_title_mode,
            ..WindowOptions::default()
        })
    })
    .await
    .context("Wayland window task failed")?;
    theme_task.abort();
    if let Some(smoke) = topology_smoke {
        smoke.await.context("topology smoke task failed")??;
    }
    topology_manager
        .await
        .context("topology manager task failed")??;
    result
}

#[allow(
    clippy::too_many_lines,
    reason = "subscription resync, controller ownership, and window task shutdown are one lifecycle"
)]
pub(super) async fn run_live_window(config: AppConfig, splint_id: SplintId) -> Result<()> {
    let theme = load_startup_theme(&config);
    renderer::configure(RendererOptions {
        font: config.font.clone(),
        font_size: config.font_size,
        font_sizing_policy: config.font_sizing_policy,
        physical_dpi: 96.0,
        padding: config.padding,
        background_alpha: theme.background_alpha,
    })?;
    let mut connection = Connection::connect().await?;
    let incarnation = connection.live_incarnation(splint_id).await?;
    let requested_scopes = vec![
        AccessScope::Observe,
        AccessScope::Scrollback,
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
    let image_cache =
        SharedImageContentCache::with_maximum_bytes(MAX_RENDERER_IMAGE_RESIDENT_BYTES)?;
    resolve_image_contents(&mut connection, &attachment.snapshot, &image_cache).await?;
    let initial_image_sources = lease_snapshot_images(&image_cache, &attachment.snapshot)?;
    let mut control = Connection::connect().await?;
    let control_incarnation = control.live_incarnation(splint_id).await?;
    if control_incarnation != incarnation {
        bail!("control connection observed a different process incarnation");
    }
    let controller_id = control
        .acquire_control(
            splint_id,
            incarnation,
            vec![ControlMode::Input, ControlMode::Resize],
        )
        .await?;
    println!("Controller lease {controller_id} granted for live Splint");
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
        Some(controller_id),
        splint_id,
        incarnation,
        config.resize_delay_ms,
        controller_cancellation,
    ));
    let mut last_revision = attachment.snapshot.revision;
    let initial_snapshot = attachment.snapshot;
    let window_config = config.clone();
    let (graphical_focus, graphical_focus_updates) = watch::channel(None);
    let _graphical_focus_reporter = spawn_graphical_focus_reporter(graphical_focus_updates);
    let mut window = tokio::task::spawn_blocking(move || {
        run_window(WindowOptions {
            snapshot: Some(initial_snapshot),
            image_sources: initial_image_sources,
            updates: Some(receiver),
            commands: Some(command_sender),
            authority,
            graphical_focus: Some(graphical_focus),
            initial_columns: window_config.initial_columns,
            initial_rows: window_config.initial_rows,
            cursor_style: window_config.cursor_style,
            cursor_blink: window_config.cursor_blink,
            title: window_config.title,
            theme,
            pane_divider_style: window_config.pane_divider_style,
            frame_title_mode: window_config.frame_title_mode,
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
                resolve_image_contents(&mut connection, &attachment.snapshot, &image_cache).await?;
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
                            resolve_image_contents(&mut connection, &snapshot, &image_cache).await?;
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
                            resolve_image_contents(&mut connection, &attachment.snapshot, &image_cache).await?;
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
                            resolve_image_contents(&mut connection, &attachment.snapshot, &image_cache).await?;
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
