//! Pictures for entries that were found by their window and have no desktop file. On a thread of its
//! own, the window's app id is looked up in the icon themes the way a desktop file's icon is: as it is,
//! in lowercase, and its last part in lowercase (`org.gnome.Calculator`, `calculator`). If none is found,
//! the UI's app glyph stays.

use std::io;
use std::sync::Arc;

use clamshell_state::{Event, Reading};

use crate::Sink;
use crate::os::worker::Worker;
use crate::os::xdg;

/// Starts the icons thread. Each request is an entry and the app id to look up.
pub(crate) fn spawn(sink: impl Sink) -> io::Result<Worker<(Arc<str>, String)>> {
    Worker::spawn(clamshell_state::Thread::Icons.name(), move |requests: &std::sync::mpsc::Receiver<(Arc<str>, String)>| {
        let dirs = xdg::data_dirs();
        for (entry, app_id) in requests {
            let last = app_id.rsplit('.').next().unwrap_or_default().to_lowercase();
            let names = [app_id.clone(), app_id.to_lowercase(), last];
            if let Some(path) = names.iter().filter(|n| !n.is_empty()).find_map(|n| xdg::find_icon(n, &dirs))
                && sink.send(Event::Reading(Reading::Icon { entry, path })).is_err()
            {
                return;
            }
        }
    })
}
