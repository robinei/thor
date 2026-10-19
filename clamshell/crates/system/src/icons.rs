//! Pictures for entries found by their window that have no desktop file: the window's app id looked up in
//! the icon themes as a desktop file's icon is (as it is, lowercase, and its last part lowercase:
//! `org.gnome.Calculator`, `calculator`), on a thread of its own. None found: the UI's app glyph stays.

use std::io;
use std::sync::Arc;

use clamshell_core::{Event, Reading};

use crate::worker::Worker;
use crate::{Sink, desktop};

/// The icons thread; its requests: an entry and the app id to look up.
pub fn spawn(sink: impl Sink) -> io::Result<Worker<(Arc<str>, String)>> {
    Worker::spawn("icons", move |requests: std::sync::mpsc::Receiver<(Arc<str>, String)>| {
        let dirs = desktop::data_dirs();
        for (entry, app_id) in requests {
            let last = app_id.rsplit('.').next().unwrap_or_default().to_lowercase();
            let names = [app_id.clone(), app_id.to_lowercase(), last];
            if let Some(path) = names.iter().filter(|n| !n.is_empty()).find_map(|n| desktop::find_icon(n, &dirs))
                && sink.send(Event::Reading(Reading::Icon { entry, path })).is_err()
            {
                return;
            }
        }
    })
}
