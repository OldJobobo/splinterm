use std::{
    env,
    future::Future,
    io::{self, IsTerminal, Write},
    path::PathBuf,
    sync::mpsc as std_mpsc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use splinterm::{
    SessionPickerDecision, SessionPickerUi, WindowOptions,
    config::{AppConfig, ResolvedTheme},
    endpoint::ConnectionFactory,
    renderer::{self, RendererOptions},
    run_window,
    session_picker::{SessionEntry, collect_sessions},
};
use splinterm_core::{DojoId, LairId, SplintId};
use splinterm_protocol::{Request, Response};

use super::{
    session_catalog::{
        create_request, launch_parameters, recent_dojo_ids, remember_dojo, select_dojo_from,
        session_picker_item,
    },
    window::{run_live_multipane_window, run_live_window},
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

fn configure_picker_renderer(config: &AppConfig, theme: ResolvedTheme) -> Result<()> {
    renderer::configure(RendererOptions {
        font: config.font.clone(),
        font_size: config.font_size,
        font_sizing_policy: config.font_sizing_policy,
        physical_dpi: 96.0,
        padding: config.padding,
        background_alpha: theme.background_alpha,
    })
}

fn choose_recent_session(
    config: &AppConfig,
    entries: &[SessionEntry],
) -> Result<Option<SessionPickerDecision>> {
    let theme = load_startup_theme(config);
    configure_picker_renderer(config, theme)?;
    let items = entries.iter().map(session_picker_item).collect();
    let (decision, receiver) = std_mpsc::channel();
    let mut picker = SessionPickerUi::new(items, decision);
    let snapshot = picker.snapshot();
    run_window(WindowOptions {
        snapshot: Some(snapshot),
        session_picker: Some(picker),
        initial_columns: config.initial_columns,
        initial_rows: config.initial_rows,
        cursor_style: config.cursor_style,
        cursor_blink: false,
        theme,
        keymap: config.keymap.clone(),
        prefix_timeout_ms: config.prefix_timeout_ms,
        ..WindowOptions::default()
    })?;
    Ok(receiver.try_recv().ok())
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
        "selected session no longer has a fully running pane layout"
    );
    Ok(dojo)
}

pub(in crate::app) async fn run_sessions(
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
    drop(connection);
    let entries = collect_sessions(&lairs, &recent_dojo_ids(&factory))
        .into_iter()
        .filter(SessionEntry::reopenable)
        .collect::<Vec<_>>();
    let picker_entries = entries.clone();
    let picker_config = config.clone();
    let decision =
        tokio::task::spawn_blocking(move || choose_recent_session(&picker_config, &picker_entries))
            .await
            .context("session picker task failed")??;
    match decision {
        None => Ok(()),
        Some(SessionPickerDecision::New) => {
            if let Some(diagnostics) = splinterm::diagnostics::global() {
                diagnostics.begin_window(None, None);
            }
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let cwd = if factory.is_local() {
                Some(env::current_dir().context("failed to read current directory")?)
            } else {
                None
            };
            launch(
                Some(format!("terminal-{stamp}-{}", std::process::id())),
                cwd,
                None,
                true,
                Vec::new(),
                config,
                factory,
            )
            .await
        }
        Some(SessionPickerDecision::Open(index)) => {
            if let Some(diagnostics) = splinterm::diagnostics::global() {
                diagnostics.begin_window(None, None);
            }
            let selected = entries
                .get(index)
                .context("session picker returned an invalid selection")?;
            let dojo = select_reopenable_dojo(&factory, selected.lair_id, selected.dojo_id).await?;
            remember_dojo(&factory, dojo.id);
            run_live_multipane_window(config, dojo, factory).await
        }
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
        .context("no recent running session; open the session picker with `splinterm sessions`")?;
    let dojo = select_dojo_from(&lairs, (selected.lair_id, selected.dojo_id))?;
    drop(connection);
    remember_dojo(&factory, dojo.id);
    run_live_multipane_window(config, dojo, factory).await
}

fn fresh_dojo_name(now: SystemTime, process_id: u32) -> String {
    let stamp = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    format!("terminal-{stamp}-{process_id}")
}

async fn keep_owner_until_complete<Owner, WindowFuture>(
    owner: Owner,
    window: WindowFuture,
) -> WindowFuture::Output
where
    WindowFuture: Future,
{
    let result = window.await;
    drop(owner);
    result
}

pub(in crate::app) async fn xdg_launch(
    cwd: PathBuf,
    command: Vec<String>,
    config: AppConfig,
    factory: ConnectionFactory,
) -> Result<()> {
    if command.is_empty() {
        return launch(None, Some(cwd), None, true, command, config, factory).await;
    }
    if !factory.is_local() {
        bail!("transient XDG launch requires the local trusted client");
    }
    let mut owner = factory
        .connect()
        .await
        .context("splinterd is unavailable; start splinterd.service or run splinterd")?;
    let expected = owner.topology_revision().await?;
    let name = fresh_dojo_name(SystemTime::now(), std::process::id());
    let Response::LairCreated { lair, .. } = owner
        .request(Request::CreateTransientLair {
            expected_topology_revision: expected,
            name,
            launch: launch_parameters(cwd, command, &config),
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
    keep_owner_until_complete(owner, run_live_multipane_window(config, dojo, factory)).await
}

pub(in crate::app) async fn launch(
    name: Option<String>,
    cwd: Option<PathBuf>,
    splint_id: Option<SplintId>,
    _create_new: bool,
    command: Vec<String>,
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
    if let Some(splint_id) = splint_id {
        if !command.is_empty() {
            bail!("cannot execute a new command while attaching an existing Splint");
        }
        let dojo = dojo_containing(&lairs, splint_id)
            .context("selected Splint is not present in a daemon Dojo")?;
        remember_dojo(&factory, dojo.id);
        drop(connection);
        return run_live_window(config, splint_id, factory).await;
    }

    let name = name.unwrap_or_else(|| fresh_dojo_name(SystemTime::now(), std::process::id()));
    let expected = connection.topology_revision().await?;
    let Response::LairCreated { lair: dojo, .. } = connection
        .request(create_request(
            &factory, expected, name, cwd, command, &config,
        )?)
        .await?
    else {
        bail!("splinterd did not create the requested terminal");
    };
    let dojo = dojo
        .dojos
        .first()
        .context("new Lair did not contain a Dojo")?;
    if !matches!(&dojo.root, splinterm_core::LayoutNode::Leaf(_)) {
        bail!("new dojo did not contain exactly one Splint");
    }
    let dojo = dojo.clone();
    remember_dojo(&factory, dojo.id);
    drop(connection);
    run_live_multipane_window(config, dojo, factory).await
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use super::*;

    struct DropMarker(Arc<AtomicBool>);

    impl Drop for DropMarker {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn owner_is_kept_until_window_future_completes() {
        for succeeds in [true, false] {
            let dropped = Arc::new(AtomicBool::new(false));
            let (send, receive) = tokio::sync::oneshot::channel::<Result<(), &'static str>>();
            let task = tokio::spawn(keep_owner_until_complete(
                DropMarker(Arc::clone(&dropped)),
                async move { receive.await.unwrap() },
            ));
            tokio::task::yield_now().await;
            assert!(!dropped.load(Ordering::SeqCst));
            let expected = if succeeds {
                Ok(())
            } else {
                Err("window failed")
            };
            send.send(expected).unwrap();
            let actual = task.await.unwrap();
            assert_eq!(actual.is_ok(), succeeds);
            assert!(dropped.load(Ordering::SeqCst));
        }
    }

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
    fn fresh_dojo_names_include_time_and_process_identity() {
        assert_eq!(
            fresh_dojo_name(
                SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_234),
                56
            ),
            "terminal-1234-56"
        );
    }
}
