use std::sync::Arc;

use splinterm::{
    FontUpdate, WindowTopologyUpdate, WindowUpdate,
    config::FontAuthority,
    renderer::{
        FontFingerprint, FontGeneration, probe_live_font_sources, stage_live_font_generation,
    },
};
use tokio::sync::mpsc;

pub(in crate::app) enum FontUpdateSink {
    Panes(Vec<mpsc::Sender<WindowUpdate>>),
    Topology(mpsc::Sender<WindowTopologyUpdate>),
}

#[derive(Debug)]
struct FontReloadState<F> {
    observed: Option<F>,
    current: F,
}

impl<F: Eq> FontReloadState<F> {
    fn new(current: F) -> Self {
        Self {
            observed: None,
            current,
        }
    }

    fn observe(&mut self, fingerprint: F) -> bool {
        if self.observed.as_ref() == Some(&fingerprint) {
            return false;
        }
        self.observed = Some(fingerprint);
        true
    }

    fn reject_observed(&mut self) {
        self.observed = None;
    }

    fn accept(&mut self, fingerprint: F) -> bool {
        if self.current == fingerprint {
            return false;
        }
        self.current = fingerprint;
        true
    }
}

#[derive(Default)]
struct FontReloadDiagnostics {
    rejection_reported: bool,
}

impl FontReloadDiagnostics {
    fn accepted(&mut self) {
        self.rejection_reported = false;
    }

    fn rejected(&mut self, error: &anyhow::Error) -> Option<String> {
        if self.rejection_reported {
            return None;
        }
        self.rejection_reported = true;
        Some(format!("splinterm live font update rejected: {error:#}"))
    }
}

async fn probe(pattern: String, authority: FontAuthority) -> anyhow::Result<FontFingerprint> {
    tokio::task::spawn_blocking(move || probe_live_font_sources(&pattern, authority))
        .await
        .map_err(|error| anyhow::anyhow!("font probe worker failed: {error}"))?
}

async fn stage(pattern: String, authority: FontAuthority) -> anyhow::Result<FontGeneration> {
    tokio::task::spawn_blocking(move || stage_live_font_generation(&pattern, authority))
        .await
        .map_err(|error| anyhow::anyhow!("font staging worker failed: {error}"))?
}

async fn deliver(sink: &FontUpdateSink, generation: Arc<FontGeneration>) -> bool {
    let update = FontUpdate { generation };
    match sink {
        FontUpdateSink::Panes(updates) => {
            let mut delivered = false;
            for updates in updates {
                delivered |= updates
                    .send(WindowUpdate::Font(update.clone()))
                    .await
                    .is_ok();
            }
            delivered
        }
        FontUpdateSink::Topology(updates) => updates
            .send(WindowTopologyUpdate::Font(update))
            .await
            .is_ok(),
    }
}

pub(in crate::app) async fn watch_font(
    pattern: String,
    authority: FontAuthority,
    current: Arc<FontGeneration>,
    sink: FontUpdateSink,
) {
    if authority != FontAuthority::NativeOmarchy {
        return;
    }
    let mut state = FontReloadState::new(current.fingerprint().clone());
    let mut diagnostics = FontReloadDiagnostics::default();
    let mut retry_after = None;
    let mut poll = tokio::time::interval(std::time::Duration::from_secs(1));
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        poll.tick().await;
        if retry_after.is_some_and(|deadline| tokio::time::Instant::now() < deadline) {
            continue;
        }
        retry_after = None;
        let fingerprint = match probe(pattern.clone(), authority).await {
            Ok(fingerprint) => fingerprint,
            Err(error) => {
                if let Some(diagnostic) = diagnostics.rejected(&error) {
                    eprintln!("{diagnostic}");
                }
                continue;
            }
        };
        if !state.observe(fingerprint) {
            continue;
        }
        match stage(pattern.clone(), authority).await {
            Ok(generation) => {
                let generation = Arc::new(generation);
                if !state.accept(generation.fingerprint().clone()) {
                    diagnostics.accepted();
                    continue;
                }
                diagnostics.accepted();
                if !deliver(&sink, generation).await {
                    break;
                }
            }
            Err(error) => {
                state.reject_observed();
                retry_after = Some(tokio::time::Instant::now() + std::time::Duration::from_secs(5));
                if let Some(diagnostic) = diagnostics.rejected(&error) {
                    eprintln!("{diagnostic}");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reload_state_coalesces_probes_and_publishes_only_changed_candidates() {
        let mut state = FontReloadState::new(1_u8);
        assert!(state.observe(1));
        assert!(!state.accept(1));
        assert!(!state.observe(1));
        assert!(state.observe(2));
        assert!(state.accept(2));
        assert!(!state.accept(2));
        assert!(state.observe(1));
        assert!(state.accept(1));
    }

    #[test]
    fn failed_staging_retries_the_same_observed_fingerprint() {
        let mut state = FontReloadState::new(1_u8);
        assert!(state.observe(2));
        state.reject_observed();
        assert!(state.observe(2));
        assert!(state.accept(2));
    }

    #[test]
    fn reload_rejection_diagnostics_are_bounded_until_acceptance() {
        let mut diagnostics = FontReloadDiagnostics::default();
        let error = anyhow::anyhow!("invalid candidate");
        assert!(diagnostics.rejected(&error).is_some());
        assert!(diagnostics.rejected(&error).is_none());
        diagnostics.accepted();
        assert!(diagnostics.rejected(&error).is_some());
    }
}
