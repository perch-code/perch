//! The restart that installs an update. It stands in the drawer's footer
//! in the login's place while a downloaded release waits, so it's there
//! for as long as the update is, not only while a toast is up.

use std::rc::Rc;

use floem::prelude::*;
use floem::reactive::{SignalGet, SignalUpdate};

use crate::backend::Backend;
use crate::review::cards::tip_with;
use crate::state::AppState;
use crate::ui::widgets::action_button;
use crate::update::UpdateStatus;

/// Restart to update, or retry one that failed.
pub fn button(state: AppState, backend: Rc<Backend>) -> impl IntoView {
    let label = move || match state.update.get() {
        UpdateStatus::Current => String::new(),
        UpdateStatus::Ready { .. } => "Restart to update".into(),
        UpdateStatus::Installing { .. } => "Updating…".into(),
        UpdateStatus::Failed { .. } => "Retry update".into(),
    };
    let enabled = move || {
        matches!(
            state.update.get(),
            UpdateStatus::Ready { .. } | UpdateStatus::Failed { .. }
        )
    };
    let button = action_button(label, "update-button", true, enabled, move || {
        restart(state, &backend)
    });
    tip_with(button, move || match state.update.get() {
        UpdateStatus::Current => String::new(),
        UpdateStatus::Ready { version } => {
            format!("Perch {version} is downloaded. Restart Perch to use it.")
        }
        UpdateStatus::Installing { version } => format!("Installing Perch {version}"),
        UpdateStatus::Failed { version, error } => {
            format!("Couldn't update to Perch {version}: {error}")
        }
    })
}

fn restart(state: AppState, backend: &Backend) {
    let version = match state.update.get_untracked() {
        UpdateStatus::Ready { version } | UpdateStatus::Failed { version, .. } => version,
        UpdateStatus::Current | UpdateStatus::Installing { .. } => return,
    };
    state.update.set(UpdateStatus::Installing {
        version: version.clone(),
    });
    backend.install_update(&version);
}
