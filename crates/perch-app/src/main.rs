//! Perch: a read-only GitHub notifications inbox for the desktop.

mod backend;
mod filter;
mod icons;
mod review;
mod state;
mod theme;
mod timefmt;
mod ui;

use std::rc::Rc;

use floem::ext_event::update_signal_from_channel;
use floem::kurbo::Size;
use floem::reactive::{RwSignal, SignalGet};
use floem::window::WindowConfig;
use floem::Application;

use crate::state::AppState;

mod dump;

fn main() {
    // A debugging mode that renders a file the way the editor would, without
    // a window. The screen has to be unlocked to look at the real thing, and
    // this checks everything up to the pixels.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--dump-file") {
        dump::run(&args[2..]);
        return;
    }

    tracing_subscriber::fmt()
        // stderr, so logs are not held in a block-buffered stdout when the
        // app is launched with its output redirected to a file.
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "perch=info,warn".into()),
        )
        .init();
    tracing::info!("perch starting");

    let (backend, events) = backend::spawn();
    let backend = Rc::new(backend);

    Application::new()
        .window(
            move |_| {
                let state = AppState::new();
                let review = review::model::ReviewState::new();

                // Backend events arrive on a std channel. floem drains it on
                // the UI thread and writes each one into this signal; the
                // effect below folds them into app state one at a time.
                let incoming = RwSignal::new(None::<backend::Event>);
                update_signal_from_channel(incoming.write_only(), events);
                floem::reactive::Effect::new(move |_| {
                    if let Some(event) = incoming.get() {
                        state.apply_with_review(event, review);
                    }
                });

                ui::app_view(state, review, backend.clone())
            },
            Some(
                WindowConfig::default()
                    .size(Size::new(1280.0, 820.0))
                    .min_size(Size::new(900.0, 600.0))
                    .title("Perch"),
            ),
        )
        .run();
}
