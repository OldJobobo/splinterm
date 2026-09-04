//! Manual AT-SPI inspection harness for the non-graphical accessibility spike.

use std::{
    thread,
    time::{Duration, Instant},
};

use anyhow::Result;
use splinterm::accessibility::{
    SemanticActionQueue, SemanticAvailability, SemanticFocus, SemanticNavigationSnapshot,
    SemanticNodeId, SemanticOwnerTurn, SemanticTreeItem, UnixAccessibilityAdapter,
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
    let mut dojo = item(17, Some(16), 2, "editor", Some(true));
    "Here, 2 of 2 running".clone_into(&mut dojo.status);
    dojo.selected = true;
    dojo.current = true;
    let splint = item(18, Some(17), 3, "shell", None);
    let snapshot = SemanticNavigationSnapshot {
        items: vec![lair, dojo, splint],
        query: String::new(),
        result_count: 3,
        status: Some("3 results".to_owned()),
        focus: SemanticFocus::Item(SemanticNodeId(17)),
    };
    let actions = SemanticActionQueue::new(|| {});
    let mut adapter = UnixAccessibilityAdapter::new(snapshot.clone(), actions.clone())?;
    adapter.update_window_focus_state(true);
    let started = Instant::now();
    let mut announced = false;
    let mut owner_turn = 0_u64;
    eprintln!(
        "Accessibility spike active; one status update follows in 5 seconds. Press Ctrl+C to stop."
    );
    loop {
        owner_turn = owner_turn.wrapping_add(1);
        for action in actions.drain() {
            eprintln!("AT-SPI action: {action:?}");
        }
        if !announced && started.elapsed() >= Duration::from_secs(5) {
            let mut refreshed = snapshot.clone();
            refreshed.status = Some("Navigation ready".to_owned());
            adapter.stage(refreshed.clone());
            let _ = adapter.publish_pending(SemanticOwnerTurn(owner_turn))?;
            adapter.stage(refreshed);
            debug_assert!(!adapter.publish_pending(SemanticOwnerTurn(owner_turn))?);
            announced = true;
        }
        thread::sleep(Duration::from_millis(50));
    }
}
