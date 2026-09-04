//! Manual AT-SPI inspection harness for the non-graphical accessibility spike.

use std::{
    thread,
    time::{Duration, Instant},
};

use anyhow::Result;
use splinterm::accessibility::{
    SemanticAction, SemanticActionQueue, SemanticAvailability, SemanticFocus,
    SemanticNavigationSnapshot, SemanticNodeId, SemanticOwnerTurn, SemanticTreeItem,
    UnixAccessibilityAdapter,
};

fn item(
    id: u64,
    parent: Option<u64>,
    level: usize,
    name: &str,
    expanded: Option<bool>,
) -> SemanticTreeItem {
    SemanticTreeItem {
        id: SemanticNodeId::item(id).expect("manual fixture item ID"),
        parent: parent.map(|id| SemanticNodeId::item(id).expect("manual fixture parent ID")),
        name: name.to_owned(),
        status: String::new(),
        level,
        expanded,
        selected: false,
        current: false,
        availability: SemanticAvailability::Enabled,
    }
}

fn main() -> Result<()> {
    let mut lair = item(16, None, 1, "work", Some(true));
    "Saved".clone_into(&mut lair.status);
    let mut dojo = item(17, Some(16), 2, "editor", Some(false));
    "Here, 2 of 2 running".clone_into(&mut dojo.status);
    dojo.selected = true;
    dojo.current = true;
    let mut splint = item(18, Some(17), 3, "shell", None);
    "Starting".clone_into(&mut splint.status);
    splint.availability = SemanticAvailability::Pending;
    let mut unavailable = item(19, Some(16), 2, "offline", None);
    "Unavailable".clone_into(&mut unavailable.status);
    unavailable.availability = SemanticAvailability::Disabled;
    let snapshot = SemanticNavigationSnapshot {
        items: vec![lair, dojo, splint, unavailable],
        query: String::new(),
        result_count: 4,
        status: Some("4 results".to_owned()),
        focus: SemanticFocus::Item(SemanticNodeId(17)),
    };
    let actions = SemanticActionQueue::new(|| {});
    let mut current = snapshot;
    let mut adapter = UnixAccessibilityAdapter::new(current.clone(), actions.clone())?;
    if let Some(error) = adapter.transport_error() {
        anyhow::bail!("AT-SPI transport failed: {error}");
    }
    adapter.update_window_focus_state(true);
    let started = Instant::now();
    let mut announced = false;
    let mut owner_turn = 0_u64;
    eprintln!(
        "Accessibility spike active; one status update follows in 5 seconds. Press Ctrl+C to stop."
    );
    loop {
        owner_turn = owner_turn.wrapping_add(1);
        let mut changed = false;
        for action in actions.drain() {
            eprintln!("AT-SPI action: {action:?}");
            match action {
                SemanticAction::Focus(node) => {
                    current.focus = match node {
                        SemanticNodeId(2) => SemanticFocus::Terminal,
                        SemanticNodeId(3) => SemanticFocus::Search,
                        SemanticNodeId(4) => SemanticFocus::Tree,
                        node => SemanticFocus::Item(node),
                    };
                    changed = true;
                }
                SemanticAction::Activate(node) => {
                    current.status = Some(format!("Activated {}", node.0));
                    changed = true;
                }
                SemanticAction::SetExpanded { node, expanded } => {
                    if let Some(item) = current.items.iter_mut().find(|item| item.id == node) {
                        item.expanded = Some(expanded);
                        changed = true;
                    }
                }
                SemanticAction::SetSearch(query) => {
                    current.query = query;
                    changed = true;
                }
            }
        }
        if !announced && started.elapsed() >= Duration::from_secs(5) {
            current.status = Some("Navigation ready".to_owned());
            announced = true;
            changed = true;
        }
        if changed {
            adapter.stage(current.clone());
            let _ = adapter.publish_pending(SemanticOwnerTurn(owner_turn))?;
            adapter.stage(current.clone());
            debug_assert!(!adapter.publish_pending(SemanticOwnerTurn(owner_turn))?);
        }
        thread::sleep(Duration::from_millis(50));
    }
}
