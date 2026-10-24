use std::{
    env,
    io::{self, IsTerminal, Write},
    path::PathBuf,
    sync::mpsc as std_mpsc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use splinterm::{
    SessionPickerDecision, SessionPickerItem, SessionPickerUi, WindowOptions, WindowSurface,
    config::{AppConfig, ResolvedTheme},
    endpoint::ConnectionFactory,
    remote_launch::hand_off_to_remote_chooser,
    renderer::{self, RendererOptions},
    run_window,
    session_picker::{SessionEntry, collect_sessions},
};
use splinterm_core::{DojoId, LairId, SplintId};
use splinterm_protocol::{Request, Response};

use super::{
    remote_chooser::remote_dojos_row,
    session_catalog::{
        GraphicalLairLifetime, graphical_create_request, launch_parameters, recent_dojo_ids,
        remember_dojo, select_dojo_from, session_picker_item,
    },
    window::{
        WhileParked, parked_close, parked_deadline, resolved_window_app_id,
        run_live_multipane_window, run_live_multipane_window_with_app_id, run_live_window,
        run_owned_live_multipane_window_with_app_id, run_window_phase, while_parked,
    },
};

use super::theme_watch::load_startup_theme;

fn collect_choices(
    node: &splinterm_core::LayoutNode,
    lair: &str,
    dojo: &str,
    choices: &mut Vec<(SplintId, String)>,
) {
    match node {
        splinterm_core::LayoutNode::Leaf(splint) => choices.push((
            splint.id,
            format!("{lair} / {dojo} / {} ({:?})", splint.title, splint.state),
        )),
        splinterm_core::LayoutNode::Branch { first, second, .. } => {
            collect_choices(first, lair, dojo, choices);
            collect_choices(second, lair, dojo, choices);
        }
    }
}

fn parse_session_choice(
    choices: &[(SplintId, String)],
    allow_new: bool,
    answer: &str,
) -> Result<Option<SplintId>> {
    let answer = answer.trim();
    if allow_new && answer.eq_ignore_ascii_case("new") {
        return Ok(None);
    }
    let selected: SplintId = answer.parse().context("selection is not a Splint UUID")?;
    choices
        .iter()
        .any(|(id, _)| *id == selected)
        .then_some(Some(selected))
        .context("selected Splint is not present in the current Lair")
}

fn choose_session(lairs: &[splinterm_core::Lair], allow_new: bool) -> Result<Option<SplintId>> {
    if !io::stdin().is_terminal() {
        let guidance = if allow_new {
            "pass --splint-id <UUID> to attach or --new to create"
        } else {
            "pass a Splint UUID explicitly"
        };
        bail!("session selection requires an interactive terminal; {guidance}");
    }
    let mut choices = Vec::new();
    for lair in lairs {
        for dojo in &lair.dojos {
            collect_choices(&dojo.root, &lair.name, &dojo.name, &mut choices);
        }
    }
    eprintln!("Saved Splints:");
    for (id, label) in &choices {
        eprintln!("  {id}  {label}");
    }
    if allow_new {
        eprintln!("  new  create a new Dojo");
    }
    eprint!(
        "Enter an exact Splint UUID{}: ",
        if allow_new { " or 'new'" } else { "" }
    );
    io::stderr()
        .flush()
        .context("failed to display session chooser")?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .context("failed to read session selection")?;
    parse_session_choice(&choices, allow_new, &answer)
}

fn dojo_containing(
    lairs: &[splinterm_core::Lair],
    splint_id: SplintId,
) -> Option<splinterm_core::Dojo> {
    lairs
        .iter()
        .flat_map(|dojo| &dojo.dojos)
        .find(|dojo| dojo.root.find_splint(splint_id).is_some())
        .cloned()
}

pub(in crate::app) async fn select_dojo(
    factory: &ConnectionFactory,
    selection: Option<(LairId, DojoId)>,
) -> Result<splinterm_core::Dojo> {
    let mut connection = factory.connect().await?;
    let Response::Lairs { lairs, .. } = connection.request(Request::ListLairs).await? else {
        bail!("splinterd did not return its session list");
    };
    if let Some(selection) = selection {
        select_dojo_from(&lairs, selection)
    } else {
        let splint_id = choose_session(&lairs, false)?.context("no Splint was selected")?;
        dojo_containing(&lairs, splint_id)
            .context("selected Splint is not present in a daemon Dojo")
    }
}

pub(in crate::app) fn configure_picker_renderer(
    config: &AppConfig,
    theme: ResolvedTheme,
) -> Result<()> {
    renderer::configure(RendererOptions {
        font: config.font.clone(),
        font_ligatures: config.font_ligatures,
        font_features: config.font_features.clone(),
        font_authority: config.font_authority,
        font_size: config.font_size,
        font_sizing_policy: config.font_sizing_policy,
        physical_dpi: 96.0,
        padding: config.padding,
        background_alpha: theme.background_alpha,
    })
}

/// Shows Recent Dojos, with `remote_row` appended after the Dojo rows: on the
/// staged Window `surface` when given (returning it parked), or else in a new
/// Window of its own.
fn choose_recent_session(
    config: &AppConfig,
    entries: &[SessionEntry],
    remote_row: Option<SessionPickerItem>,
    app_id: Option<String>,
    surface: Option<WindowSurface>,
) -> Result<(Option<SessionPickerDecision>, Option<WindowSurface>)> {
    let mut items = entries.iter().map(session_picker_item).collect::<Vec<_>>();
    items.extend(remote_row);
    let (decision, receiver) = std_mpsc::channel();
    let mut picker = SessionPickerUi::new(items, decision);
    if entries.is_empty() {
        // A lone remote row must not displace New as the default choice.
        picker.select_new_row();
    }
    if surface.is_some() {
        return run_staged_picker(config, picker, &receiver, app_id, surface);
    }
    Ok((
        run_standalone_picker(config, picker, &receiver, app_id)?,
        None,
    ))
}

/// Runs one standalone picker Window and returns its decision, if any.
pub(in crate::app) fn run_standalone_picker(
    config: &AppConfig,
    picker: SessionPickerUi,
    receiver: &std_mpsc::Receiver<SessionPickerDecision>,
    app_id: Option<String>,
) -> Result<Option<SessionPickerDecision>> {
    run_window(standalone_picker_options(config, picker, app_id)?)?;
    Ok(receiver.try_recv().ok())
}

/// Runs a standalone picker as one phase of a staged Window, on `surface` or
/// on a new Window, and returns its decision (none after Escape) and the
/// parked Window. No Window comes back when it was closed.
pub(in crate::app) fn run_staged_picker(
    config: &AppConfig,
    picker: SessionPickerUi,
    receiver: &std_mpsc::Receiver<SessionPickerDecision>,
    app_id: Option<String>,
    surface: Option<WindowSurface>,
) -> Result<(Option<SessionPickerDecision>, Option<WindowSurface>)> {
    let options = WindowOptions {
        keep_surface: true,
        // Phases before the terminal never bind the input method.
        input_method: false,
        ..standalone_picker_options(config, picker, app_id)?
    };
    let surface = run_window_phase(surface, options)?;
    Ok((receiver.try_recv().ok(), surface))
}

fn standalone_picker_options(
    config: &AppConfig,
    mut picker: SessionPickerUi,
    app_id: Option<String>,
) -> Result<WindowOptions> {
    let theme = load_startup_theme(config);
    configure_picker_renderer(config, theme)?;
    let snapshot = picker.snapshot();
    Ok(WindowOptions {
        snapshot: Some(snapshot),
        session_picker: Some(picker),
        app_id: resolved_window_app_id(app_id),
        initial_columns: config.initial_columns,
        initial_rows: config.initial_rows,
        cursor_style: config.cursor_style,
        cursor_blink: false,
        theme,
        ui_font_size: config.ui_font_size,
        keymap: config.keymap.clone(),
        prefix_timeout_ms: config.prefix_timeout_ms,
        ..WindowOptions::default()
    })
}

async fn select_reopenable_dojo(
    factory: &ConnectionFactory,
    lair_id: LairId,
    dojo_id: DojoId,
) -> Result<splinterm_core::Dojo> {
    let mut connection = factory.connect().await?;
    let Response::Lairs { lairs, .. } = connection.request(Request::ListLairs).await? else {
        bail!("splinterd did not return its session list");
    };
    let dojo = select_dojo_from(&lairs, (lair_id, dojo_id))?;
    let reopenable = collect_sessions(&lairs, &[])
        .into_iter()
        .find(|entry| entry.dojo_id == dojo_id)
        .is_some_and(|entry| entry.reopenable());
    anyhow::ensure!(
        reopenable,
        "selected Dojo no longer has a fully running pane layout"
    );
    Ok(dojo)
}

pub(in crate::app) async fn run_dojos(config: AppConfig, factory: ConnectionFactory) -> Result<()> {
    run_dojos_from(None, None, config, factory, None).await
}

/// Opens the Recent Dojos picker; **New** creates its terminal in `cwd`, or in
/// the local current directory when absent, and every Window of the flow keeps
/// the launcher-supplied `app_id`. With a staged Window `surface` (a remote
/// connection status that kept its Window), the picker and then the terminal
/// show in that same Window.
pub(in crate::app) async fn run_dojos_from(
    cwd: Option<PathBuf>,
    app_id: Option<String>,
    config: AppConfig,
    factory: ConnectionFactory,
    surface: Option<WindowSurface>,
) -> Result<()> {
    let staged = surface.is_some();
    let deadline = parked_deadline(&factory);
    let listed = while_parked(parked_close(surface.as_ref()), deadline, async {
        let mut connection = factory
            .connect()
            .await
            .context("splinterd is unavailable; start splinterd.service or run splinterd")?;
        let Response::Lairs { lairs, .. } = connection.request(Request::ListLairs).await? else {
            bail!("splinterd did not return its session list");
        };
        Ok(lairs)
    })
    .await?;
    let WhileParked::Done(lairs) = listed else {
        return Ok(());
    };
    let entries = collect_sessions(&lairs, &recent_dojo_ids(&factory))
        .into_iter()
        .filter(SessionEntry::reopenable)
        .collect::<Vec<_>>();
    // Remote profiles are local configuration, so only a local picker offers them.
    let remote_row = factory.is_local().then(remote_dojos_row).flatten();
    let remote_row_index = remote_row.is_some().then_some(entries.len());
    let picker_entries = entries.clone();
    let picker_config = config.clone();
    let picker_app_id = app_id.clone();
    let (decision, surface) = tokio::task::spawn_blocking(move || {
        choose_recent_session(
            &picker_config,
            &picker_entries,
            remote_row,
            picker_app_id,
            surface,
        )
    })
    .await
    .context("Dojo picker task failed")??;
    if staged && surface.is_none() {
        // The compositor closed the Window: nothing is left to continue in.
        return Ok(());
    }
    match decision {
        // Escape. A parked Window unmaps when it is dropped here.
        None => Ok(()),
        Some(SessionPickerDecision::Open(index)) if Some(index) == remote_row_index => {
            hand_off_to_remote_chooser()
        }
        Some(SessionPickerDecision::New) => {
            begin_terminal_window(staged);
            let cwd = match cwd {
                None if factory.is_local() => {
                    Some(env::current_dir().context("failed to read current directory")?)
                }
                cwd => cwd,
            };
            launch_with_app_id(
                None,
                cwd,
                None,
                true,
                Vec::new(),
                config,
                factory,
                app_id,
                surface,
            )
            .await
        }
        Some(SessionPickerDecision::Open(index)) => {
            begin_terminal_window(staged);
            let selected = entries
                .get(index)
                .context("Dojo picker returned an invalid selection")?;
            let selected = while_parked(
                parked_close(surface.as_ref()),
                deadline,
                select_reopenable_dojo(&factory, selected.lair_id, selected.dojo_id),
            )
            .await?;
            let WhileParked::Done(dojo) = selected else {
                return Ok(());
            };
            remember_dojo(&factory, dojo.id);
            run_live_multipane_window_with_app_id(config, dojo, factory, app_id, true, surface)
                .await
        }
    }
}

/// A picker in a Window of its own recorded that Window's exit, so the
/// terminal that follows is a new diagnostic Window. A staged Window is still
/// the same Window.
fn begin_terminal_window(staged: bool) {
    if !staged && let Some(diagnostics) = splinterm::diagnostics::global() {
        diagnostics.begin_window(None, None);
    }
}

pub(in crate::app) async fn reopen_recent(
    config: AppConfig,
    factory: ConnectionFactory,
) -> Result<()> {
    let mut connection = factory
        .connect()
        .await
        .context("splinterd is unavailable; start splinterd.service or run splinterd")?;
    let Response::Lairs { lairs, .. } = connection.request(Request::ListLairs).await? else {
        bail!("splinterd did not return its session list");
    };
    let recent = recent_dojo_ids(&factory);
    let entries = collect_sessions(&lairs, &recent);
    let selected = recent
        .iter()
        .find_map(|dojo_id| {
            entries
                .iter()
                .find(|entry| entry.dojo_id == *dojo_id && entry.reopenable())
        })
        .context("no recent running Dojo; open the Dojo picker with `splinterm dojos`")?;
    let dojo = select_dojo_from(&lairs, (selected.lair_id, selected.dojo_id))?;
    drop(connection);
    remember_dojo(&factory, dojo.id);
    run_live_multipane_window(config, dojo, factory).await
}

fn fresh_lair_name(now: SystemTime, process_id: u32) -> String {
    let stamp = now
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    splinterm_core::generated_lair_name(stamp, process_id)
}

pub(in crate::app) async fn xdg_launch(
    cwd: PathBuf,
    app_id: Option<String>,
    command: Vec<String>,
    config: AppConfig,
    factory: ConnectionFactory,
) -> Result<()> {
    if command.is_empty() {
        return launch_with_app_id(
            None,
            Some(cwd),
            None,
            true,
            command,
            config,
            factory,
            app_id,
            None,
        )
        .await;
    }
    if !factory.is_local() {
        bail!("transient XDG launch requires the local trusted client");
    }
    let mut owner = factory
        .connect()
        .await
        .context("splinterd is unavailable; start splinterd.service or run splinterd")?;
    let expected = owner.topology_revision().await?;
    let name = fresh_lair_name(SystemTime::now(), std::process::id());
    let Response::LairCreated { lair, .. } = owner
        .request(Request::CreateTransientLair {
            expected_topology_revision: expected,
            name,
            launch: launch_parameters(cwd, command, &config),
            generated_name: true,
        })
        .await?
    else {
        bail!("splinterd did not create the transient terminal");
    };
    let dojo = lair
        .dojos
        .first()
        .context("new transient Lair did not contain a Dojo")?
        .clone();
    if !matches!(&dojo.root, splinterm_core::LayoutNode::Leaf(_)) {
        bail!("new transient Dojo did not contain exactly one Splint");
    }
    run_owned_live_multipane_window_with_app_id(config, dojo, factory, owner, app_id, false, None)
        .await
}

pub(in crate::app) async fn launch(
    name: Option<String>,
    cwd: Option<PathBuf>,
    splint_id: Option<SplintId>,
    create_new: bool,
    command: Vec<String>,
    config: AppConfig,
    factory: ConnectionFactory,
) -> Result<()> {
    launch_with_app_id(
        name, cwd, splint_id, create_new, command, config, factory, None, None,
    )
    .await
}

/// Creates a new terminal on `factory`'s endpoint and opens it on the parked
/// staged Window `surface`, as **New** in that endpoint's Dojo picker does.
pub(in crate::app) async fn launch_new_terminal_on(
    config: AppConfig,
    factory: ConnectionFactory,
    surface: WindowSurface,
) -> Result<()> {
    let cwd = if factory.is_local() {
        Some(env::current_dir().context("failed to read current directory")?)
    } else {
        None
    };
    launch_with_app_id(
        None,
        cwd,
        None,
        true,
        Vec::new(),
        config,
        factory,
        None,
        Some(surface),
    )
    .await
}

#[allow(
    clippy::too_many_arguments,
    reason = "the private XDG app ID remains explicit and client-local at the launch boundary"
)]
async fn launch_with_app_id(
    name: Option<String>,
    cwd: Option<PathBuf>,
    splint_id: Option<SplintId>,
    _create_new: bool,
    command: Vec<String>,
    config: AppConfig,
    factory: ConnectionFactory,
    app_id: Option<String>,
    surface: Option<WindowSurface>,
) -> Result<()> {
    let deadline = parked_deadline(&factory);
    let listed = while_parked(parked_close(surface.as_ref()), deadline, async {
        let mut connection = factory
            .connect()
            .await
            .context("splinterd is unavailable; start splinterd.service or run splinterd")?;
        let Response::Lairs { lairs, .. } = connection.request(Request::ListLairs).await? else {
            bail!("splinterd did not return its session list");
        };
        Ok((connection, lairs))
    })
    .await?;
    let WhileParked::Done((mut connection, lairs)) = listed else {
        return Ok(());
    };
    if let Some(splint_id) = splint_id {
        // Attaching one Splint is never a staged launch; any parked Window
        // unmaps here.
        drop(surface);
        if !command.is_empty() {
            bail!("cannot execute a new command while attaching an existing Splint");
        }
        let dojo = dojo_containing(&lairs, splint_id)
            .context("selected Splint is not present in a daemon Dojo")?;
        remember_dojo(&factory, dojo.id);
        drop(connection);
        return run_live_window(config, splint_id, factory).await;
    }

    let explicit_name = name.is_some();
    let name = name.unwrap_or_else(|| fresh_lair_name(SystemTime::now(), std::process::id()));
    let created = while_parked(parked_close(surface.as_ref()), deadline, async {
        let expected = connection.topology_revision().await?;
        let (request, lifetime) = graphical_create_request(
            &factory,
            expected,
            name,
            explicit_name,
            cwd,
            command,
            &config,
        )?;
        let Response::LairCreated { lair, .. } = connection.request(request).await? else {
            bail!("splinterd did not create the requested terminal");
        };
        Ok((lair, lifetime))
    })
    .await?;
    let WhileParked::Done((dojo, lifetime)) = created else {
        return Ok(());
    };
    let dojo = dojo
        .dojos
        .first()
        .context("new Lair did not contain a Dojo")?;
    if !matches!(&dojo.root, splinterm_core::LayoutNode::Leaf(_)) {
        bail!("new dojo did not contain exactly one Splint");
    }
    let dojo = dojo.clone();
    match lifetime {
        GraphicalLairLifetime::Persistent => {
            remember_dojo(&factory, dojo.id);
            drop(connection);
            run_live_multipane_window_with_app_id(config, dojo, factory, app_id, true, surface)
                .await
        }
        GraphicalLairLifetime::ClientBound => {
            run_owned_live_multipane_window_with_app_id(
                config, dojo, factory, connection, app_id, true, surface,
            )
            .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dojo_selection_uses_only_its_local_hint() {
        let mut first = splinterm_core::Lair::new("first", PathBuf::from("/tmp"));
        let first_dojo = first.id;
        let first_dojo_id = first.dojos[0].id;
        let first_hint = first.dojos[0].default_focus;
        let second = splinterm_core::Lair::new("second", PathBuf::from("/tmp"));
        let second_dojo_id = second.dojos[0].id;
        let second_hint = second.dojos[0].default_focus;

        let selected = select_dojo_from(
            &[first.clone(), second.clone()],
            (first_dojo, first_dojo_id),
        )
        .unwrap();
        assert_eq!(selected.default_focus, first_hint);
        assert_eq!(selected.root, first.dojos[0].root);
        assert_ne!(first_hint, second_hint);
        assert!(select_dojo_from(&[first.clone(), second], (first_dojo, second_dojo_id)).is_err());

        first.dojos[0].default_focus = SplintId::new();
        assert!(select_dojo_from(&[first], (first_dojo, first_dojo_id)).is_err());
    }

    #[test]
    fn explicit_dojo_choice_requires_an_exact_saved_splint_id() {
        let saved = SplintId::new();
        let choices = vec![(saved, "saved".to_owned())];
        assert_eq!(parse_session_choice(&choices, true, "new\n").unwrap(), None);
        assert_eq!(
            parse_session_choice(&choices, true, &saved.to_string()).unwrap(),
            Some(saved)
        );
        assert!(parse_session_choice(&choices, false, "new").is_err());
    }

    #[test]
    fn fresh_lair_names_include_time_and_process_identity() {
        assert_eq!(
            fresh_lair_name(
                SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_234),
                56
            ),
            "terminal-1234000000000-56"
        );
    }
}
