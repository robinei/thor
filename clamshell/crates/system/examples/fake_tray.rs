//! Development: a fake program with a tray icon (StatusNotifierItem and its dbusmenu menu), for testing
//! ClamShell's tray without one. It registers with the watcher (ClamShell, in the device's session) and
//! says on standard output what it is asked: `activate`, `clicked ID`. Activated, it opens a window: a
//! terminal (foot) as a process of its own, as a program's window would be. Its menu: Open (1), a submenu
//! Recent (2) with First (3) and Second (4), Disabled (5, greyed), Exit (6, which ends it).
//!
//!     cargo run -p clamshell-system --example fake_tray

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use zbus::blocking::Connection;
use zbus::zvariant::{ObjectPath, OwnedValue, Structure, Value};

struct Item;

#[zbus::interface(name = "org.kde.StatusNotifierItem")]
impl Item {
    fn activate(&self, _x: i32, _y: i32) {
        println!("activate");
        match std::process::Command::new("foot").arg("--title=Fake Tray").spawn() {
            Ok(child) => drop(std::thread::spawn(move || drop(child.wait_with_output()))),
            Err(e) => println!("no window: {e}"),
        }
    }

    #[zbus(property)]
    fn id(&self) -> &str {
        "fake-tray"
    }

    #[zbus(property)]
    fn title(&self) -> &str {
        "Fake Tray"
    }

    #[zbus(property)]
    fn status(&self) -> &str {
        "Active"
    }

    #[zbus(property)]
    fn icon_name(&self) -> &str {
        "utilities-terminal"
    }

    #[zbus(property)]
    fn menu(&self) -> ObjectPath<'static> {
        ObjectPath::from_static_str_unchecked("/Menu")
    }
}

/// A menu's layout node: id, properties, children (nodes, in variants).
type Node = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

struct Menu {
    quit: Arc<AtomicBool>,
}

/// A menu node: id, properties, children (in variants).
fn node(id: i32, label: &str, extra: &[(&str, Value<'static>)], children: Vec<Value<'static>>) -> Value<'static> {
    let mut props: HashMap<String, Value<'static>> = HashMap::from([("label".to_owned(), Value::from(label.to_owned()))]);
    for (k, v) in extra {
        props.insert((*k).to_owned(), v.try_clone().unwrap_or(Value::Bool(false)));
    }
    Value::from(Structure::from((id, props, children)))
}

#[zbus::interface(name = "com.canonical.dbusmenu")]
impl Menu {
    fn get_layout(&self, _parent: i32, _depth: i32, _props: Vec<String>) -> (u32, Node) {
        let recent = node(
            2,
            "_Recent",
            &[("children-display", Value::from("submenu"))],
            vec![Value::Value(Box::new(node(3, "First", &[], vec![]))), Value::Value(Box::new(node(4, "Second", &[], vec![])))],
        );
        let children = [
            node(1, "_Open", &[], vec![]),
            recent,
            node(7, "", &[("type", Value::from("separator"))], vec![]),
            node(5, "Disabled", &[("enabled", Value::from(false))], vec![]),
            node(6, "E_xit", &[], vec![]),
        ];
        let children = children.into_iter().filter_map(|c| OwnedValue::try_from(Value::Value(Box::new(c))).ok()).collect();
        (1, (0, HashMap::new(), children))
    }

    fn event(&self, id: i32, kind: &str, _data: OwnedValue, _time: u32) {
        println!("{kind} {id}");
        if kind == "clicked" && id == 6 {
            self.quit.store(true, Ordering::Relaxed);
        }
    }

    fn about_to_show(&self, _id: i32) -> bool {
        false
    }
}

fn main() -> zbus::Result<()> {
    let quit = Arc::new(AtomicBool::new(false));
    let conn = Connection::session()?;
    conn.object_server().at("/StatusNotifierItem", Item)?;
    conn.object_server().at("/Menu", Menu { quit: quit.clone() })?;
    let watcher = zbus::blocking::Proxy::new(&conn, "org.kde.StatusNotifierWatcher", "/StatusNotifierWatcher", "org.kde.StatusNotifierWatcher")?;
    watcher.call::<_, _, ()>("RegisterStatusNotifierItem", &("/StatusNotifierItem",))?;
    println!("registered as {}", conn.unique_name().map(ToString::to_string).unwrap_or_default());
    while !quit.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}
