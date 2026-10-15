//! BlueZ's D-Bus interfaces (system bus), as much of them as the Bluetooth tab uses, and the pairing
//! agent we register while pairing.

use std::collections::HashMap;

use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue};

pub const ADAPTER: &str = "org.bluez.Adapter1";
pub const DEVICE: &str = "org.bluez.Device1";
pub const AGENT_PATH: &str = "/org/clamshell/agent";

/// Every object BlueZ has, with its interfaces' properties.
pub type ManagedObjects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

#[zbus::proxy(interface = "org.freedesktop.DBus.ObjectManager", default_service = "org.bluez", default_path = "/", gen_async = false)]
pub trait Objects {
    fn get_managed_objects(&self) -> zbus::Result<ManagedObjects>;
}

#[zbus::proxy(interface = "org.bluez.Adapter1", default_service = "org.bluez", gen_async = false)]
pub trait Adapter {
    fn start_discovery(&self) -> zbus::Result<()>;
    fn stop_discovery(&self) -> zbus::Result<()>;
    fn remove_device(&self, device: &ObjectPath<'_>) -> zbus::Result<()>;
    #[zbus(property)]
    fn set_powered(&self, powered: bool) -> zbus::Result<()>;
}

#[zbus::proxy(interface = "org.bluez.Device1", default_service = "org.bluez", gen_async = false)]
pub trait Device {
    fn connect(&self) -> zbus::Result<()>;
    fn disconnect(&self) -> zbus::Result<()>;
    fn pair(&self) -> zbus::Result<()>;
    #[zbus(property)]
    fn paired(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn set_trusted(&self, trusted: bool) -> zbus::Result<()>;
}

#[zbus::proxy(interface = "org.bluez.AgentManager1", default_service = "org.bluez", default_path = "/org/bluez", gen_async = false)]
pub trait AgentManager {
    fn register_agent(&self, agent: &ObjectPath<'_>, capability: &str) -> zbus::Result<()>;
    fn unregister_agent(&self, agent: &ObjectPath<'_>) -> zbus::Result<()>;
}

/// A device's object path: the adapter's, then `dev_` and its address with underscores.
pub fn device_path(adapter: &OwnedObjectPath, address: &str) -> String {
    format!("{}/dev_{}", adapter.as_str(), address.replace(':', "_"))
}

/// The pairing agent: no screen or keyboard to show or type a code with; what BlueZ asks to confirm is
/// confirmed (the user asked to pair).
pub struct Agent;

#[zbus::interface(name = "org.bluez.Agent1")]
impl Agent {
    fn release(&self) {}

    fn request_confirmation(&self, _device: OwnedObjectPath, _passkey: u32) {}

    fn request_authorization(&self, _device: OwnedObjectPath) {}

    fn authorize_service(&self, _device: OwnedObjectPath, _uuid: String) {}

    fn request_pin_code(&self, _device: OwnedObjectPath) -> zbus::fdo::Result<String> {
        Err(zbus::fdo::Error::Failed("no way to enter a PIN".into()))
    }

    fn request_passkey(&self, _device: OwnedObjectPath) -> zbus::fdo::Result<u32> {
        Err(zbus::fdo::Error::Failed("no way to enter a passkey".into()))
    }

    fn display_pin_code(&self, _device: OwnedObjectPath, _pincode: String) {}

    fn display_passkey(&self, _device: OwnedObjectPath, _passkey: u32, _entered: u16) {}

    fn cancel(&self) {}
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn device_paths_are_the_adapters_and_the_address() {
        let adapter = OwnedObjectPath::try_from("/org/bluez/hci0").unwrap();
        assert_eq!(device_path(&adapter, "AA:BB:CC:00:11:22"), "/org/bluez/hci0/dev_AA_BB_CC_00_11_22");
    }
}
