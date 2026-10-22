//! Development: a fake program with a tray icon (StatusNotifierItem and its dbusmenu menu), for testing
//! ClamShell's tray without a real one. It registers with the watcher, which is ClamShell in the device's
//! session, and prints on standard output what it is asked: `activate` or `clicked ID`. When activated it
//! opens a window, a terminal (foot) in a process of its own, as a program's window would be. Its menu
//! has Open (1), a submenu Recent (2) with First (3) and Second (4), Disabled (5, greyed out), and Exit
//! (6, which ends it).
//!
//!     cargo run -p clamshell-system --example fake_tray

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use clamshell_system::os::dbus::{self, Bus, Message, Path, PropMap, Value, value};

/// A menu node: id, properties, and children wrapped in variants.
fn node(id: i32, label: &str, extra: Vec<(&str, Value)>, children: Vec<Value>) -> Value {
    let mut props = PropMap::from([("label".to_owned(), value(label.to_owned()))]);
    props.extend(extra.into_iter().map(|(k, v)| (k.to_owned(), v)));
    value((id, props, children))
}

/// The menu's layout as (revision, (id, properties, children)).
fn layout() -> (u32, (i32, PropMap, Vec<Value>)) {
    let recent =
        node(2, "_Recent", vec![("children-display", value("submenu".to_owned()))], vec![node(3, "First", vec![], vec![]), node(4, "Second", vec![], vec![])]);
    let children = vec![
        node(1, "_Open", vec![], vec![]),
        recent,
        node(7, "", vec![("type", value("separator".to_owned()))], vec![]),
        node(5, "Disabled", vec![("enabled", value(false))], vec![]),
        node(6, "E_xit", vec![], vec![]),
    ];
    (1, (0, PropMap::new(), children))
}

fn item_property(name: &str) -> Option<Value> {
    Some(match name {
        "Id" => value("fake-tray".to_owned()),
        "Title" => value("Fake Tray".to_owned()),
        "Status" => value("Active".to_owned()),
        "IconName" => value("utilities-terminal".to_owned()),
        "Menu" => value(Path::from("/Menu")),
        _ => return None,
    })
}

/// The reply to a call, if it is one of ours.
fn answer(call: &Message, quit: &AtomicBool) -> Option<Message> {
    let path = call.path()?.to_string();
    let member = call.member()?.to_string();
    match (path.as_str(), member.as_str()) {
        ("/StatusNotifierItem", "Activate") => {
            println!("activate");
            match std::process::Command::new("foot").arg("--title=Fake Tray").spawn() {
                Ok(child) => drop(std::thread::spawn(move || drop(child.wait_with_output()))),
                Err(e) => println!("no window: {e}"),
            }
            Some(call.method_return())
        }
        ("/StatusNotifierItem", "Get") => {
            let (_, name): (String, String) = call.read2().ok()?;
            Some(call.return_with_args((item_property(&name)?,)))
        }
        ("/StatusNotifierItem", "GetAll") => {
            let all: PropMap = ["Id", "Title", "Status", "IconName", "Menu"].iter().filter_map(|&n| Some((n.to_owned(), item_property(n)?))).collect();
            Some(call.return_with_args((all,)))
        }
        ("/Menu", "GetLayout") => Some(call.return_with_args(layout())),
        ("/Menu", "AboutToShow") => Some(call.return_with_args((false,))),
        ("/Menu", "Event") => {
            let (id, kind): (i32, String) = call.read2().ok()?;
            println!("{kind} {id}");
            if kind == "clicked" && id == 6 {
                quit.store(true, Ordering::Relaxed);
            }
            Some(call.method_return())
        }
        _ => None,
    }
}

fn main() -> dbus::Result<()> {
    let quit = Arc::new(AtomicBool::new(false));
    let mut bus = Bus::session()?;
    let asked = quit.clone();
    bus.serve(Box::new(move |call| answer(call, &asked)));
    bus.call::<()>(
        "org.kde.StatusNotifierWatcher",
        "/StatusNotifierWatcher",
        "org.kde.StatusNotifierWatcher",
        "RegisterStatusNotifierItem",
        ("/StatusNotifierItem",),
    )?;
    println!("registered as {}", bus.unique_name().unwrap_or_default());
    while !quit.load(Ordering::Relaxed) {
        bus.wait(Some(Duration::from_millis(100)))?;
    }
    Ok(())
}
