//! Development: an app asking xdg-desktop-portal for a file, with `--folder` a folder, or with `--save NAME`
//! where to save NAME, as a file chooser does, for testing ClamShell's portal (clamshell-system's `session::portal`). It stays on the bus until
//! the portal answers, as the portal ends a request whose app has gone, and prints the response (0 chosen,
//! 1 cancelled, 2 other) and the URIs chosen, a line each.
//!
//!     cargo run -p clamshell-system --example portal_ask -- [--folder | --save NAME] [TITLE]

use clamshell_system::os::dbus::{Bus, Path, PropMap, Result, value};

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PATH: &str = "/org/freedesktop/portal/desktop";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let folder = args.iter().any(|a| a == "--folder");
    let save = args.iter().position(|a| a == "--save").and_then(|i| args.get(i + 1)).cloned();
    let title = args.iter().find(|a| *a != "--folder" && *a != "--save" && Some(*a) != save.as_ref()).cloned().unwrap_or_default();
    let mut bus = Bus::session()?;
    // Before asking, so that a quick answer isn't missed.
    bus.add_match("type='signal',interface='org.freedesktop.portal.Request',member='Response'")?;
    let (method, options) = match save {
        Some(name) => ("SaveFile", PropMap::from([("current_name".to_owned(), value(name))])),
        None => ("OpenFile", PropMap::from([("directory".to_owned(), value(folder))])),
    };
    let (handle,): (Path<'static>,) = bus.call(PORTAL, PATH, "org.freedesktop.portal.FileChooser", method, ("", title, options))?;
    loop {
        bus.wait(None)?;
        for signal in bus.signals()? {
            if signal.path().as_deref() != Some(&*handle) {
                continue;
            }
            let Ok((response, results)) = signal.read2::<u32, PropMap>() else { continue };
            println!("response {response}");
            let uris = results.get("uris").and_then(|v| v.0.as_iter());
            for uri in uris.into_iter().flatten().filter_map(|u| u.as_str()) {
                println!("{uri}");
            }
            return Ok(());
        }
    }
}
