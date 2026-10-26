//! The parts of BlueZ's D-Bus interfaces on the system bus that the Bluetooth tab uses, and the pairing
//! agent we register while pairing.

use std::collections::HashMap;

use crate::os::dbus::{Message, Path, PropMap};

pub(crate) const SERVICE: &str = "org.bluez";
pub(crate) const ADAPTER: &str = "org.bluez.Adapter1";
pub(crate) const DEVICE: &str = "org.bluez.Device1";
pub(crate) const AGENT_MANAGER: &str = "org.bluez.AgentManager1";
pub(crate) const AGENT_PATH: &str = "/org/clamshell/agent";

/// Every object BlueZ has, with its interfaces' properties.
pub(crate) type ManagedObjects = HashMap<Path<'static>, HashMap<String, PropMap>>;

/// A device's object path: the adapter's path, then `dev_` and the address with underscores.
pub(crate) fn device_path(adapter: &str, address: &str) -> String {
    format!("{adapter}/dev_{}", address.replace(':', "_"))
}

/// The pairing agent (org.bluez.Agent1 at [`AGENT_PATH`]), served while pairing. It has no screen or
/// keyboard to show or type a code with, and it confirms whatever BlueZ asks, since the user asked to
/// pair. Returns its reply to a call, if the call is for it.
pub(crate) fn agent(call: &Message) -> Option<Message> {
    if call.path().as_deref() != Some(AGENT_PATH) || call.interface().as_deref() != Some("org.bluez.Agent1") {
        return None;
    }
    match &*call.member()? {
        "RequestPinCode" | "RequestPasskey" => Some(call.error(&"org.bluez.Error.Rejected".into(), c"no way to enter a code")),
        "Release" | "RequestConfirmation" | "RequestAuthorization" | "AuthorizeService" | "DisplayPinCode" | "DisplayPasskey" | "Cancel" => {
            Some(call.method_return())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_paths_are_the_adapters_and_the_address() {
        assert_eq!(device_path("/org/bluez/hci0", "AA:BB:CC:00:11:22"), "/org/bluez/hci0/dev_AA_BB_CC_00_11_22");
    }
}
