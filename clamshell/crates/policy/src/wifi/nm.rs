//! NetworkManager's D-Bus interfaces (system bus), as much of them as the Wi-Fi tab uses, and the codes
//! it reports states and reasons with.

use std::collections::HashMap;

use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

pub const DEVICE: &str = "org.freedesktop.NetworkManager.Device";
pub const ACTIVE: &str = "org.freedesktop.NetworkManager.Connection.Active";

#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager",
    default_service = "org.freedesktop.NetworkManager",
    default_path = "/org/freedesktop/NetworkManager",
    gen_async = false
)]
pub trait Manager {
    fn get_devices(&self) -> zbus::Result<Vec<OwnedObjectPath>>;
    fn activate_connection(&self, connection: &ObjectPath<'_>, device: &ObjectPath<'_>, specific_object: &ObjectPath<'_>) -> zbus::Result<OwnedObjectPath>;
    /// The new connection's settings path and the active connection made for it.
    fn add_and_activate_connection(
        &self,
        connection: HashMap<&str, HashMap<&str, Value<'_>>>,
        device: &ObjectPath<'_>,
        specific_object: &ObjectPath<'_>,
    ) -> zbus::Result<(OwnedObjectPath, OwnedObjectPath)>;
    #[zbus(property)]
    fn wireless_enabled(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn set_wireless_enabled(&self, enabled: bool) -> zbus::Result<()>;
}

#[zbus::proxy(interface = "org.freedesktop.NetworkManager.Device", default_service = "org.freedesktop.NetworkManager", gen_async = false)]
pub trait Device {
    #[zbus(property)]
    fn device_type(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn state(&self) -> zbus::Result<u32>;
}

#[zbus::proxy(interface = "org.freedesktop.NetworkManager.Device.Wireless", default_service = "org.freedesktop.NetworkManager", gen_async = false)]
pub trait Wireless {
    fn request_scan(&self, options: HashMap<&str, Value<'_>>) -> zbus::Result<()>;
    fn get_all_access_points(&self) -> zbus::Result<Vec<OwnedObjectPath>>;
    #[zbus(property)]
    fn active_access_point(&self) -> zbus::Result<OwnedObjectPath>;
}

#[zbus::proxy(interface = "org.freedesktop.NetworkManager.AccessPoint", default_service = "org.freedesktop.NetworkManager", gen_async = false)]
pub trait AccessPoint {
    #[zbus(property)]
    fn ssid(&self) -> zbus::Result<Vec<u8>>;
    #[zbus(property)]
    fn strength(&self) -> zbus::Result<u8>;
    #[zbus(property)]
    fn flags(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn wpa_flags(&self) -> zbus::Result<u32>;
    #[zbus(property)]
    fn rsn_flags(&self) -> zbus::Result<u32>;
}

#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager.Settings",
    default_service = "org.freedesktop.NetworkManager",
    default_path = "/org/freedesktop/NetworkManager/Settings",
    gen_async = false
)]
pub trait Settings {
    fn list_connections(&self) -> zbus::Result<Vec<OwnedObjectPath>>;
}

#[zbus::proxy(interface = "org.freedesktop.NetworkManager.Settings.Connection", default_service = "org.freedesktop.NetworkManager", gen_async = false)]
pub trait Connection {
    fn get_settings(&self) -> zbus::Result<HashMap<String, HashMap<String, OwnedValue>>>;
    fn delete(&self) -> zbus::Result<()>;
}

#[zbus::proxy(interface = "org.freedesktop.NetworkManager.Connection.Active", default_service = "org.freedesktop.NetworkManager", gen_async = false)]
pub trait Active {
    #[zbus(property)]
    fn state(&self) -> zbus::Result<u32>;
}

/// NMDeviceType, the one we look for.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceType {
    Wifi = 2,
}

/// NMDeviceState, the ones we look at.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceState {
    Unmanaged = 10,
    Activated = 100,
    Failed = 120,
}

/// NMDeviceStateReason, the ones a failed join is told apart by.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceReason {
    /// No address from the network (IP configuration unavailable).
    NoAddress = 5,
    /// The secrets (the password) missing or refused.
    NoSecrets = 7,
}

/// NMActiveConnectionState, the ones we look at.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveState {
    Activated = 2,
    Deactivated = 4,
}

/// NMActiveConnectionStateReason, the ones a failed join is told apart by.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveReason {
    /// No address from the network.
    NoAddress = 5,
    /// The secrets (the password) missing or refused.
    NoSecrets = 9,
}

/// NM80211ApFlags: privacy (WEP, or any encryption).
pub const AP_PRIVACY: u32 = 0x1;
/// NM80211ApSecurityFlags: key management PSK (WPA personal), 802.1X (enterprise), SAE (WPA3).
pub const KEY_MGMT_PSK: u32 = 0x100;
pub const KEY_MGMT_8021X: u32 = 0x200;
pub const KEY_MGMT_SAE: u32 = 0x400;
