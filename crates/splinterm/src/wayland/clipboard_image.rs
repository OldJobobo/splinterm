//! One bounded worker owns image bytes and descriptors through publication.
//! The UI owns only identity, small messages and copied paths. Dropping its
//! request disconnects authorization; workers never join the Wayland thread.

use std::{
    os::fd::OwnedFd,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    task::Waker,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use tokio::sync::mpsc::Sender;

use super::{
    clipboard::{CLIPBOARD_IO_TIMEOUT, read_clipboard_with_deadline},
    file_drop::FileDropTarget,
};
use crate::{
    clipboard_image::{MAX_PNG_BYTES, PNG_MIME, StagedClipboardImage},
    frontend::WindowCommand,
};

static IMAGE_WORKER_ACTIVE: AtomicBool = AtomicBool::new(false);
const AUTHORIZATION_TIMEOUT: Duration = Duration::from_secs(2);

struct ImagePermit;
impl ImagePermit {
    fn acquire() -> Result<Self> {
        IMAGE_WORKER_ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| anyhow::anyhow!("another clipboard image save is still running"))?;
        Ok(Self)
    }
}
impl Drop for ImagePermit {
    fn drop(&mut self) {
        IMAGE_WORKER_ACTIVE.store(false, Ordering::Release);
    }
}

#[derive(Debug)]
pub(super) enum ImageEvent {
    Ready,
    Published {
        path: PathBuf,
        verified: bool,
        expires: Instant,
    },
    Failed(String),
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum ImageDecision {
    Publish,
    Admitted,
}

pub(super) struct ImageRequest {
    cancelled: Arc<AtomicBool>,
    pub(super) target: FileDropTarget,
    pub(super) directory: PathBuf,
    pub(super) commands: Sender<WindowCommand>,
    pub(super) events: Receiver<ImageEvent>,
    pub(super) decisions: Option<SyncSender<ImageDecision>>,
}

impl ImageRequest {
    fn cancel(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.decisions = None;
    }
}
impl Drop for ImageRequest {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

pub(super) fn png_offered(mimes: &[String]) -> bool {
    mimes.iter().any(|mime| mime == PNG_MIME)
}

/// Only a copied, validated path is included; never image bodies or decoder errors.
pub(super) fn retained_notice(path: &std::path::Path) -> String {
    // Escaping also makes format/bidi characters visible without logging controls.
    format!(
        "Clipboard PNG retained; path not admitted: {}",
        path.display().to_string().escape_debug()
    )
}

pub(super) fn spawn_image_read(
    fd: OwnedFd,
    target: FileDropTarget,
    directory: PathBuf,
    commands: Sender<WindowCommand>,
    waker: Waker,
) -> Result<ImageRequest> {
    let permit = ImagePermit::acquire()?;
    let (events_tx, events) = mpsc::sync_channel(2);
    let (decisions, decisions_rx) = mpsc::sync_channel(1);
    let worker_directory = directory.clone();
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = cancelled.clone();
    std::thread::Builder::new()
        .name("clipboard-image".into())
        .spawn(move || {
            let _permit = permit;
            let result = image_worker(
                fd,
                &worker_directory,
                &events_tx,
                &decisions_rx,
                &waker,
                &worker_cancelled,
            );
            if let Err(error) = result {
                // Storage errors are owned strings, not decoded clipboard contents.
                let _ = events_tx.try_send(ImageEvent::Failed(format!("{error:#}")));
                waker.wake_by_ref();
            }
        })
        .context("cannot start clipboard image worker")?;
    Ok(ImageRequest {
        cancelled,
        target,
        directory,
        commands,
        events,
        decisions: Some(decisions),
    })
}

fn image_worker(
    fd: OwnedFd,
    directory: &std::path::Path,
    events: &SyncSender<ImageEvent>,
    decisions: &Receiver<ImageDecision>,
    waker: &Waker,
    cancelled: &AtomicBool,
) -> Result<()> {
    let bytes = read_clipboard_with_deadline(&fd, CLIPBOARD_IO_TIMEOUT, MAX_PNG_BYTES)
        .context("clipboard PNG read failed")?;
    drop(fd);
    anyhow::ensure!(
        !cancelled.load(Ordering::Acquire),
        "clipboard image save cancelled before staging"
    );
    let staged = StagedClipboardImage::stage(directory, &bytes)?;
    drop(bytes);
    events
        .try_send(ImageEvent::Ready)
        .context("save cancelled before publication")?;
    waker.wake_by_ref();
    if decisions.recv_timeout(AUTHORIZATION_TIMEOUT) != Ok(ImageDecision::Publish) {
        anyhow::bail!("clipboard image save cancelled before publication");
    }
    anyhow::ensure!(
        !cancelled.load(Ordering::Acquire),
        "clipboard image save cancelled before publication"
    );
    let published = staged.publish()?;
    let path = published.path().to_owned();
    let verified = published.verify_path().is_ok();
    if events
        .try_send(ImageEvent::Published {
            path: path.clone(),
            verified,
            expires: Instant::now() + AUTHORIZATION_TIMEOUT,
        })
        .is_err()
    {
        eprintln!("splinterm: {}", retained_notice(&path));
        return Ok(());
    }
    waker.wake_by_ref();
    if decisions.recv_timeout(AUTHORIZATION_TIMEOUT) != Ok(ImageDecision::Admitted) {
        // Also covers Window shutdown: no UI remains, so stderr is the last
        // available best-effort notice. The published name is never removed.
        eprintln!("splinterm: {}", retained_notice(&path));
    }
    Ok(())
}

#[derive(Clone, Copy)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent terminal input authority gates"
)]
struct ImageEligibility {
    local: bool,
    configured: bool,
    focused: bool,
    obstructed: bool,
    live: bool,
    controlled: bool,
}

impl ImageEligibility {
    fn check(self, search: &super::SearchUiState, explorer_focused: bool) -> Result<()> {
        anyhow::ensure!(
            self.local,
            "clipboard image paths are local-only; remote transfer is not supported"
        );
        anyhow::ensure!(
            self.configured,
            "configure [clipboard] image-directory before saving images"
        );
        anyhow::ensure!(self.focused, "clipboard image save requires keyboard focus");
        anyhow::ensure!(
            !self.obstructed && search.input.is_none() && !explorer_focused,
            "clipboard image save requires an unobstructed terminal (close search and return focus from Explorer)"
        );
        anyhow::ensure!(
            self.live,
            "return to live output before saving a clipboard image"
        );
        anyhow::ensure!(
            self.controlled,
            "clipboard image save requires pane control"
        );
        Ok(())
    }
}

fn image_target_matches(
    captured: FileDropTarget,
    current: Option<FileDropTarget>,
    directory: &std::path::Path,
    current_directory: Option<&std::path::Path>,
) -> bool {
    current == Some(captured) && current_directory == Some(directory)
}

impl super::App {
    /// All gates are checked before receiving the offer and at both handshakes.
    pub(super) fn clipboard_image_target(&self) -> Result<FileDropTarget> {
        let pane = &self.panes.pane;
        ImageEligibility {
            local: self.clipboard.local_endpoint,
            configured: self.clipboard.image_directory.is_some(),
            focused: self.input.keyboard_focused,
            obstructed: self.modal.input_modal_open()
                || self.modal.trusted_consent.is_some()
                || self.modal.session_picker.is_some()
                || self.modal.session_picker_requested
                || self.input.divider_drag.is_some()
                || self.tab_state.session_switch_pending
                || self.modal.command_palette_reconcile_pending
                || self.modal.session_picker_reconcile_pending,
            live: pane.scrollback_viewport.is_live(),
            controlled: pane.controller_active
                && pane
                    .commands
                    .as_ref()
                    .is_some_and(|commands| !commands.is_closed()),
        }
        .check(&pane.search, self.explorer.focused())?;
        let snapshot = pane
            .snapshot
            .as_ref()
            .context("clipboard image save requires a live pane")?;
        Ok(FileDropTarget {
            topology_revision: self.tab_state.active_identity.topology_revision,
            dojo_id: self.tab_state.active_dojo_id(),
            splint_id: snapshot.splint_id,
            incarnation: snapshot.incarnation,
            input_generation: self.input.input_generation,
        })
    }

    pub(super) fn clipboard_image_notice(&self, text: &str) {
        eprintln!("splinterm: {text}");
        self.surface.window.set_title(format!("Splinterm — {text}"));
    }

    pub(super) fn cancel_clipboard_image(&mut self) {
        if let Some(request) = &mut self.clipboard.image_request {
            request.cancel();
        }
    }

    pub(super) fn begin_clipboard_image(&mut self) {
        let result = (|| -> Result<ImageRequest> {
            anyhow::ensure!(
                self.clipboard.image_request.is_none(),
                "clipboard image save already in progress"
            );
            let target = self.clipboard_image_target()?;
            let offer = self
                .clipboard
                .clipboard_offer
                .as_ref()
                .context("clipboard has no image offer")?;
            anyhow::ensure!(
                offer.with_mime_types(png_offered),
                "clipboard does not offer image/png"
            );
            let fd = offer
                .receive(PNG_MIME.to_owned())
                .context("cannot receive clipboard PNG")?;
            spawn_image_read(
                fd.into(),
                target,
                self.clipboard
                    .image_directory
                    .clone()
                    .expect("validated image directory"),
                self.panes
                    .pane
                    .commands
                    .clone()
                    .expect("validated commands"),
                self.platform.update_waker.clone(),
            )
        })();
        match result {
            Ok(request) => {
                self.clipboard.image_request = Some(request);
                self.clipboard_image_notice("Saving clipboard PNG…");
            }
            Err(error) => {
                self.clipboard_image_notice(&format!("Clipboard image save rejected: {error}"));
            }
        }
    }

    fn image_request_is_current(&self, request: &ImageRequest) -> bool {
        !request.cancelled.load(Ordering::Acquire)
            && request.decisions.is_some()
            && image_target_matches(
                request.target,
                self.clipboard_image_target().ok(),
                &request.directory,
                self.clipboard.image_directory.as_deref(),
            )
            && self
                .panes
                .pane
                .commands
                .as_ref()
                .is_some_and(|current| current.same_channel(&request.commands))
    }

    pub(super) fn apply_clipboard_image_events(&mut self) {
        let Some(mut request) = self.clipboard.image_request.take() else {
            return;
        };
        if !self.image_request_is_current(&request) {
            request.cancel();
        }
        loop {
            match request.events.try_recv() {
                Ok(ImageEvent::Ready) => {
                    if let Some(decisions) = &request.decisions {
                        let _ = decisions.try_send(ImageDecision::Publish);
                    }
                }
                Ok(ImageEvent::Published {
                    path,
                    verified,
                    expires,
                }) => {
                    let admitted = image_publication_can_admit(
                        verified,
                        expires,
                        self.image_request_is_current(&request),
                    ) && admit_image_path(
                        &request.commands,
                        &self.input.pending_terminal_input,
                        &path,
                        self.panes
                            .pane
                            .snapshot
                            .as_ref()
                            .is_some_and(|snapshot| snapshot.input_modes.bracketed_paste),
                    )
                    .is_ok();
                    if admitted {
                        if let Some(decisions) = &request.decisions {
                            let _ = decisions.try_send(ImageDecision::Admitted);
                        }
                        self.clipboard_image_notice(
                            "Clipboard PNG saved; path admitted to pane input",
                        );
                    } else {
                        self.clipboard_image_notice(&retained_notice(&path));
                    }
                    return;
                }
                Ok(ImageEvent::Failed(error)) => {
                    let bounded: String = error
                        .chars()
                        .take(512)
                        .flat_map(char::escape_debug)
                        .collect();
                    self.clipboard_image_notice(&format!("Clipboard image save failed: {bounded}"));
                    return;
                }
                Err(mpsc::TryRecvError::Disconnected) => return,
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
        self.clipboard.image_request = Some(request);
    }
}

fn image_publication_can_admit(verified: bool, expires: Instant, current: bool) -> bool {
    verified && Instant::now() < expires && current
}

fn admit_image_path(
    commands: &Sender<WindowCommand>,
    pending: &std::collections::VecDeque<super::PendingTerminalInput>,
    path: &std::path::Path,
    bracketed: bool,
) -> Result<()> {
    // Do not bypass earlier input or defer a saved-path batch whose eventual
    // teardown would otherwise be silent. Admission is not daemon acknowledgment.
    anyhow::ensure!(pending.is_empty(), "earlier terminal input is pending");
    let path = path.to_str().context("saved path is not UTF-8")?;
    let payload = super::file_drop::quote_posix_path(path);
    super::try_window_command(
        commands,
        WindowCommand::Input(super::encode_bracketed_paste(payload.as_bytes(), bracketed)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use splinterm_core::{DojoId, SplintId, TopologyRevision};
    use std::{
        collections::VecDeque,
        fs,
        io::Write,
        os::unix::{fs::DirBuilderExt, net::UnixStream},
        path::Path,
        sync::Mutex,
        thread,
    };

    static ADMISSION_TEST: Mutex<()> = Mutex::new(());
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let base = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/clipboard-image-integration-tests");
            fs::create_dir_all(&base).unwrap();
            let path = base
                .canonicalize()
                .unwrap()
                .join(uuid::Uuid::new_v4().to_string());
            fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
            Self(path)
        }
        fn count(&self) -> usize {
            fs::read_dir(&self.0).unwrap().count()
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn png() -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut writer = png::Encoder::new(&mut bytes, 1, 1).write_header().unwrap();
            writer.write_image_data(&[0]).unwrap();
        }
        bytes
    }
    fn input(bytes: &[u8]) -> OwnedFd {
        let (reader, mut writer) = UnixStream::pair().unwrap();
        writer.write_all(bytes).unwrap();
        drop(writer);
        reader.into()
    }
    fn target() -> FileDropTarget {
        FileDropTarget {
            topology_revision: TopologyRevision::new(1),
            dojo_id: DojoId::new(),
            splint_id: SplintId::new(),
            incarnation: 1,
            input_generation: 1,
        }
    }

    #[test]
    fn image_mime_is_explicit_even_for_mixed_offers_and_text_paste_stays_text() {
        assert!(!png_offered(&[]));
        assert!(!png_offered(&["image/jpeg".into(), "text/plain".into()]));
        assert!(!png_offered(&["image/png;charset=utf-8".into()]));
        let mixed = ["text/plain".into(), "image/png".into()];
        assert!(png_offered(&mixed));
        assert_eq!(
            super::super::clipboard::accepted_text_mime(&mixed),
            Some("text/plain".into())
        );
    }

    #[test]
    fn image_eligibility_rejects_remote_unconfigured_modal_copy_history_and_control_loss() {
        let valid = ImageEligibility {
            local: true,
            configured: true,
            focused: true,
            obstructed: false,
            live: true,
            controlled: true,
        };
        valid
            .check(&super::super::SearchUiState::default(), false)
            .unwrap();
        for invalid in [
            ImageEligibility {
                local: false,
                ..valid
            },
            ImageEligibility {
                configured: false,
                ..valid
            },
            ImageEligibility {
                focused: false,
                ..valid
            },
            ImageEligibility {
                obstructed: true,
                ..valid
            },
            ImageEligibility {
                live: false,
                ..valid
            },
            ImageEligibility {
                controlled: false,
                ..valid
            },
        ] {
            assert!(
                invalid
                    .check(&super::super::SearchUiState::default(), false)
                    .is_err()
            );
        }
        assert!(
            ImageEligibility {
                local: false,
                ..valid
            }
            .check(&super::super::SearchUiState::default(), false)
            .unwrap_err()
            .to_string()
            .contains("local-only")
        );
    }

    #[test]
    fn clipboard_image_custom_binding_cannot_admit_input_behind_search_or_explorer() {
        let resolved = crate::keymap::resolve_keymap_text(
            crate::keymap::KeymapProfile::Splinterm,
            "version = 1\n[[binding]]\nsequence = [\"Ctrl+Alt+I\"]\naction = \"clipboard.save-image\"\n",
            Path::new("keybindings.toml"),
        ).unwrap();
        assert_eq!(
            resolved
                .keymap
                .primary_shortcut(crate::keymap::ActionId::ClipboardSaveImage),
            "Ctrl+Alt+I"
        );
        let eligibility = ImageEligibility {
            local: true,
            configured: true,
            focused: true,
            obstructed: false,
            live: true,
            controlled: true,
        };
        let mut search = super::super::SearchUiState {
            input: Some(super::super::new_search_editor()),
            ..Default::default()
        };
        // This is the same actual pane-search state passed by clipboard_image_target
        // for custom bindings, palette capture, and both async revalidations.
        assert!(eligibility.check(&search, false).is_err());
        search.input = None;
        assert!(eligibility.check(&search, true).is_err());
        eligibility.check(&search, false).unwrap();
    }

    #[test]
    fn image_target_requires_exact_topology_tab_pane_incarnation_generation_and_destination() {
        let captured = target();
        let directory = Path::new("/private/images");
        assert!(image_target_matches(
            captured,
            Some(captured),
            directory,
            Some(directory)
        ));
        for current in [
            None,
            Some(FileDropTarget {
                topology_revision: TopologyRevision::new(2),
                ..captured
            }),
            Some(FileDropTarget {
                dojo_id: DojoId::new(),
                ..captured
            }),
            Some(FileDropTarget {
                splint_id: SplintId::new(),
                ..captured
            }),
            Some(FileDropTarget {
                incarnation: 2,
                ..captured
            }),
            Some(FileDropTarget {
                input_generation: 2,
                ..captured
            }),
        ] {
            assert!(!image_target_matches(
                captured,
                current,
                directory,
                Some(directory)
            ));
        }
        assert!(!image_target_matches(
            captured,
            Some(captured),
            directory,
            None
        ));
        assert!(!image_target_matches(
            captured,
            Some(captured),
            directory,
            Some(Path::new("/other"))
        ));
    }

    #[test]
    fn image_path_admission_is_atomic_quoted_bracketed_and_never_submits() {
        let (commands, mut receiver) = tokio::sync::mpsc::channel(1);
        let pending = VecDeque::new();
        let path = Path::new("/private/a 'quote' 日本語.png");
        for bracketed in [false, true] {
            admit_image_path(&commands, &pending, path, bracketed).unwrap();
            let WindowCommand::Input(bytes) = receiver.try_recv().unwrap() else {
                panic!("input only")
            };
            let literal = b"'/private/a '\\''quote'\\'' \xe6\x97\xa5\xe6\x9c\xac\xe8\xaa\x9e.png'";
            assert_eq!(
                bytes,
                super::super::encode_bracketed_paste(literal, bracketed)
            );
            assert!(!bytes.contains(&b'\r') && !bytes.contains(&b'\n'));
            assert!(receiver.try_recv().is_err());
        }
    }

    #[test]
    fn image_admission_rejects_pending_full_and_closed_without_reordering_or_deferral() {
        let (commands, mut receiver) = tokio::sync::mpsc::channel(1);
        let mut pending = VecDeque::new();
        pending.push_back(super::super::PendingTerminalInput {
            commands: commands.clone(),
            bytes: b"earlier".to_vec(),
        });
        assert!(admit_image_path(&commands, &pending, Path::new("/image.png"), false).is_err());
        assert_eq!(pending.front().unwrap().bytes, b"earlier");
        assert!(receiver.try_recv().is_err());
        pending.clear();
        commands
            .try_send(WindowCommand::Input(b"earlier".to_vec()))
            .unwrap();
        assert!(admit_image_path(&commands, &pending, Path::new("/image.png"), true).is_err());
        assert_eq!(
            receiver.try_recv().unwrap(),
            WindowCommand::Input(b"earlier".to_vec())
        );
        assert!(pending.is_empty());
        drop(receiver);
        assert!(admit_image_path(&commands, &pending, Path::new("/image.png"), false).is_err());
    }

    #[test]
    fn image_worker_cancelled_before_staging_and_after_readiness_leaves_no_named_file() {
        let directory = Directory::new();
        let (events, incoming) = mpsc::sync_channel(2);
        let (decisions, receive) = mpsc::sync_channel(1);
        let cancelled = AtomicBool::new(true);
        assert!(
            image_worker(
                input(&png()),
                &directory.0,
                &events,
                &receive,
                Waker::noop(),
                &cancelled
            )
            .is_err()
        );
        assert_eq!(directory.count(), 0);
        cancelled.store(false, Ordering::Release);
        thread::scope(|scope| {
            let directory = &directory;
            let cancelled = &cancelled;
            let worker = scope.spawn(move || {
                image_worker(
                    input(&png()),
                    &directory.0,
                    &events,
                    &receive,
                    Waker::noop(),
                    cancelled,
                )
            });
            assert!(matches!(
                incoming.recv_timeout(Duration::from_secs(5)).unwrap(),
                ImageEvent::Ready
            ));
            assert_eq!(directory.count(), 0);
            // Even a buffered authorization must not revive a cancelled request.
            cancelled.store(true, Ordering::Release);
            decisions.send(ImageDecision::Publish).unwrap();
            assert!(worker.join().unwrap().is_err());
        });
        assert_eq!(directory.count(), 0);
    }

    #[test]
    fn image_worker_ui_disconnect_before_publish_cleans_stage_and_wait_has_deadline() {
        let directory = Directory::new();
        for disconnect in [true, false] {
            let (events, incoming) = mpsc::sync_channel(2);
            let (decisions, receive) = mpsc::sync_channel(1);
            let cancelled = AtomicBool::new(false);
            thread::scope(|scope| {
                let directory = &directory;
                let cancelled = &cancelled;
                let worker = scope.spawn(move || {
                    image_worker(
                        input(&png()),
                        &directory.0,
                        &events,
                        &receive,
                        Waker::noop(),
                        cancelled,
                    )
                });
                assert!(matches!(
                    incoming.recv_timeout(Duration::from_secs(5)).unwrap(),
                    ImageEvent::Ready
                ));
                if disconnect {
                    drop(decisions);
                }
                assert!(worker.join().unwrap().is_err());
            });
            assert_eq!(directory.count(), 0);
        }
    }

    #[test]
    fn image_worker_retains_published_png_after_queue_failure_or_window_shutdown() {
        let directory = Directory::new();
        let bytes = png();
        for shutdown in [false, true] {
            let (events, incoming) = mpsc::sync_channel(2);
            let (decisions, receive) = mpsc::sync_channel(1);
            let cancelled = AtomicBool::new(false);
            thread::scope(|scope| {
                let directory = &directory;
                let bytes = &bytes;
                let cancelled = &cancelled;
                let worker = scope.spawn(move || {
                    image_worker(
                        input(bytes),
                        &directory.0,
                        &events,
                        &receive,
                        Waker::noop(),
                        cancelled,
                    )
                });
                assert!(matches!(
                    incoming.recv_timeout(Duration::from_secs(5)).unwrap(),
                    ImageEvent::Ready
                ));
                decisions.send(ImageDecision::Publish).unwrap();
                if shutdown {
                    drop(incoming);
                } else {
                    let ImageEvent::Published { path, verified, .. } =
                        incoming.recv_timeout(Duration::from_secs(5)).unwrap()
                    else {
                        panic!("publication")
                    };
                    assert!(verified);
                    let (commands, receiver) = tokio::sync::mpsc::channel(1);
                    drop(receiver);
                    assert!(admit_image_path(&commands, &VecDeque::new(), &path, true).is_err());
                    assert!(retained_notice(&path).contains(path.to_str().unwrap()));
                    assert_eq!(fs::read(path).unwrap(), *bytes);
                }
                drop(decisions);
                worker.join().unwrap().unwrap();
            });
        }
        assert_eq!(directory.count(), 2);
    }

    #[test]
    fn image_worker_success_and_expired_publication_preserve_file_without_duplicate_input() {
        let directory = Directory::new();
        for expire in [false, true] {
            let (events, incoming) = mpsc::sync_channel(2);
            let (decisions, receive) = mpsc::sync_channel(1);
            let cancelled = AtomicBool::new(false);
            thread::scope(|scope| {
                let directory = &directory;
                let cancelled = &cancelled;
                let worker = scope.spawn(move || {
                    image_worker(
                        input(&png()),
                        &directory.0,
                        &events,
                        &receive,
                        Waker::noop(),
                        cancelled,
                    )
                });
                assert!(matches!(
                    incoming.recv_timeout(Duration::from_secs(5)).unwrap(),
                    ImageEvent::Ready
                ));
                decisions.send(ImageDecision::Publish).unwrap();
                let ImageEvent::Published {
                    path,
                    verified,
                    expires,
                } = incoming.recv_timeout(Duration::from_secs(5)).unwrap()
                else {
                    panic!("publication")
                };
                let (commands, mut receiver) = tokio::sync::mpsc::channel(1);
                if expire {
                    // Worker-side final acknowledgment wait is bounded too.
                    worker.join().unwrap().unwrap();
                    assert!(!image_publication_can_admit(verified, expires, true));
                } else {
                    assert!(!image_publication_can_admit(false, expires, true));
                    assert!(!image_publication_can_admit(verified, expires, false));
                    assert!(image_publication_can_admit(verified, expires, true));
                    admit_image_path(&commands, &VecDeque::new(), &path, true).unwrap();
                    decisions.send(ImageDecision::Admitted).unwrap();
                    worker.join().unwrap().unwrap();
                    assert!(matches!(
                        receiver.try_recv().unwrap(),
                        WindowCommand::Input(_)
                    ));
                }
                assert!(receiver.try_recv().is_err());
                assert_eq!(fs::read(&path).unwrap(), png());
            });
        }
        assert_eq!(directory.count(), 2);
    }

    #[test]
    fn image_worker_bad_png_and_bounded_read_never_reach_publication() {
        let directory = Directory::new();
        let (events, incoming) = mpsc::sync_channel(2);
        let (_decisions, receive) = mpsc::sync_channel(1);
        assert!(
            image_worker(
                input(b"not PNG"),
                &directory.0,
                &events,
                &receive,
                Waker::noop(),
                &AtomicBool::new(false)
            )
            .is_err()
        );
        assert!(incoming.try_recv().is_err());
        assert_eq!(directory.count(), 0);
        // Exercise the same bounded pipe reader using a small cap, including
        // exactly-at-limit success and over-limit failure without large writes.
        assert_eq!(
            read_clipboard_with_deadline(&input(b"1234"), Duration::from_secs(1), 4).unwrap(),
            b"1234"
        );
        assert!(read_clipboard_with_deadline(&input(b"12345"), Duration::from_secs(1), 4).is_err());
        let (idle, _writer) = UnixStream::pair().unwrap();
        assert_eq!(
            read_clipboard_with_deadline(&idle.into(), Duration::from_millis(5), MAX_PNG_BYTES)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::TimedOut
        );
    }

    #[test]
    fn image_request_spawn_cancellation_disconnects_and_releases_global_slot() {
        let _lock = ADMISSION_TEST.lock().unwrap();
        let directory = Directory::new();
        let (commands, _receiver) = tokio::sync::mpsc::channel(1);
        let mut request = spawn_image_read(
            input(&png()),
            target(),
            directory.0.clone(),
            commands.clone(),
            Waker::noop().clone(),
        )
        .unwrap();
        assert!(
            spawn_image_read(
                input(&png()),
                target(),
                directory.0.clone(),
                commands,
                Waker::noop().clone()
            )
            .is_err()
        );
        assert!(matches!(
            request.events.recv_timeout(Duration::from_secs(5)).unwrap(),
            ImageEvent::Ready
        ));
        request.cancel();
        assert!(matches!(
            request.events.recv_timeout(Duration::from_secs(5)).unwrap(),
            ImageEvent::Failed(_)
        ));
        assert!(matches!(
            request.events.recv_timeout(Duration::from_secs(5)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ));
        let permit = ImagePermit::acquire().unwrap();
        drop(permit);
        assert_eq!(directory.count(), 0);
    }

    #[test]
    fn image_worker_global_admission_and_request_cancellation_are_bounded() {
        let _lock = ADMISSION_TEST.lock().unwrap();
        let permit = ImagePermit::acquire().unwrap();
        assert!(ImagePermit::acquire().is_err());
        drop(permit);
        let permit = ImagePermit::acquire().unwrap();
        drop(permit);
        let (commands, _receiver) = tokio::sync::mpsc::channel(1);
        let (_events, incoming) = mpsc::sync_channel(2);
        let (decisions, receive) = mpsc::sync_channel(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut request = ImageRequest {
            cancelled: cancelled.clone(),
            target: target(),
            directory: "/private".into(),
            commands,
            events: incoming,
            decisions: Some(decisions),
        };
        request.cancel();
        assert!(cancelled.load(Ordering::Acquire));
        assert!(request.decisions.is_none());
        assert_eq!(
            receive.recv_timeout(Duration::from_millis(5)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
        let path = Path::new("/private/a\u{202e}b.png");
        let notice = retained_notice(path);
        assert!(!notice.contains('\u{202e}'));
        assert!(notice.contains("\\u{202e}"));
    }
}
