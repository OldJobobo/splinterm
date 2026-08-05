//! Binary-owned application services.

mod consent;
mod human_output;
mod local_service;
mod machine;
mod pane_bridge;
mod sessions;
mod theme_watch;

pub(super) use consent::run_consent_client;
pub(super) use human_output::{print_lairs, print_response};
pub(super) use local_service::{
    confirm_kill, run_policy_command, run_relay_command, run_reset_command, usage_error,
};
pub(super) use sessions::{
    create_request, launch, launch_parameters, recent_dojo_ids, remember_dojo, reopen_recent,
    run_sessions, select_dojo, select_dojo_from, session_picker_item,
};
pub(super) use theme_watch::{ThemeUpdateSink, load_startup_theme, watch_theme};

pub(super) use pane_bridge::{
    ControllerOutputs, EventAction, PaneTask, attach, classify_subscription_event,
    layout_splint_ids, lease_snapshot_images, lease_update_images, load_authority_status,
    pane_claims_initial_control, prepare_live_pane, resolve_image_contents, resolve_update_images,
    resynchronize, run_controller, update_advances_from, validate_attached_snapshot,
};
#[cfg(test)]
pub(super) use pane_bridge::{
    PendingPaneResize, handle_control_event, optional_pane_controller, queue_pane_resize,
    resolved_resize_request, terminal_action_matches, validate_scrollback_page_response,
};

pub(super) use machine::{
    machine_exit_code, require_expected_incarnation, require_incarnation, run_machine_command,
    run_machine_subscription,
};
