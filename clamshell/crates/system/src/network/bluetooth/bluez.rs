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

/// The pairing agent (org.bluez.Agent1 at [`AGENT_PATH`]), served while pairing. It shows a code to type
/// on the device through `show` (a keyboard pairing by passkey: BlueZ gives the code; an old one asking
/// for a PIN: it is made here), and clears it when the pairing is over or cancelled. It has no keyboard to
/// type a code shown on the device, and it confirms whatever else BlueZ asks, since the user asked to
/// pair. Returns its reply to a call, if the call is for it.
pub(crate) fn agent(call: &Message, show: &mut dyn FnMut(Option<String>)) -> Option<Message> {
    if call.path().as_deref() != Some(AGENT_PATH) || call.interface().as_deref() != Some("org.bluez.Agent1") {
        return None;
    }
    match &*call.member()? {
        "DisplayPasskey" => {
            // (The device, the passkey, and how many digits have been typed so far.)
            let (_, passkey): (dbus::Path, u32) = call.read2().ok()?;
            show(Some(format!("{passkey:06}")));
            Some(call.method_return())
        }
        "DisplayPinCode" => {
            let (_, pin): (dbus::Path, String) = call.read2().ok()?;
            show(Some(pin));
            Some(call.method_return())
        }
        // An old keyboard asks the host for a PIN, which is then typed on it.
        "RequestPinCode" => {
            let pin = format!("{:06}", random() % 1_000_000);
            show(Some(pin.clone()));
            Some(call.method_return().append1(pin))
        }
        "RequestPasskey" => Some(call.error(&"org.bluez.Error.Rejected".into(), c"no way to enter a code")),
        "Release" | "Cancel" => {
            show(None);
            Some(call.method_return())
        }
        "RequestConfirmation" | "RequestAuthorization" | "AuthorizeService" => Some(call.method_return()),
        _ => None,
    }
}

/// A random number, for a PIN.
fn random() -> u32 {
    use std::io::Read;
    let mut bytes = [0u8; 4];
    if let Err(e) = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes)) {
        log::warn!("bluetooth: no random number for a PIN: {e}; the time's");
        return std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    }
    u32::from_ne_bytes(bytes)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn device_paths_are_the_adapters_and_the_address() {
        assert_eq!(device_path("/org/bluez/hci0", "AA:BB:CC:00:11:22"), "/org/bluez/hci0/dev_AA_BB_CC_00_11_22");
    }

    /// The agent shows the code a keyboard wants typed (a passkey BlueZ gives, or a PIN it makes for an
    /// old keyboard), clears it when the pairing is cancelled, and turns down typing a device's code.
    #[test]
    fn the_agent_shows_a_keyboards_code_and_clears_it() {
        // (A serial as one received would have, which a reply refers to.)
        let call = |member: &str| {
            let mut m = Message::new_method_call("org.bluez", AGENT_PATH, "org.bluez.Agent1", member).unwrap();
            m.set_serial(1);
            m
        };
        let device = Path::from("/org/bluez/hci0/dev_AA");
        let mut shown = None;
        let mut show = |code: Option<String>| shown = code;
        let reply = agent(&call("DisplayPasskey").append3(device.clone(), 42u32, 0u16), &mut show).unwrap();
        assert_eq!(reply.msg_type(), dbus::MessageType::MethodReturn);
        assert_eq!(shown.as_deref(), Some("000042"), "six digits");
        let reply = agent(&call("RequestPinCode").append1(device.clone()), &mut |code| shown = code).unwrap();
        let pin: String = reply.read1().unwrap();
        assert_eq!(shown.as_deref(), Some(pin.as_str()), "the PIN given is the one shown");
        assert_eq!(pin.len(), 6);
        agent(&call("Cancel"), &mut |code| shown = code).unwrap();
        assert_eq!(shown, None);
        let refused = agent(&call("RequestPasskey").append1(device), &mut |code| shown = code).unwrap();
        assert_eq!(refused.msg_type(), dbus::MessageType::Error);
    }
}
