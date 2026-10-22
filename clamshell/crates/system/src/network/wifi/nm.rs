//! The parts of NetworkManager's D-Bus interfaces on the system bus that the Wi-Fi tab uses, and the
//! codes it reports states and reasons with.

pub const SERVICE: &str = "org.freedesktop.NetworkManager";
pub const PATH: &str = "/org/freedesktop/NetworkManager";
pub const MANAGER: &str = "org.freedesktop.NetworkManager";
pub const DEVICE: &str = "org.freedesktop.NetworkManager.Device";
pub const WIRELESS: &str = "org.freedesktop.NetworkManager.Device.Wireless";
pub const ACCESS_POINT: &str = "org.freedesktop.NetworkManager.AccessPoint";
pub const SETTINGS_PATH: &str = "/org/freedesktop/NetworkManager/Settings";
pub const SETTINGS: &str = "org.freedesktop.NetworkManager.Settings";
pub const CONNECTION: &str = "org.freedesktop.NetworkManager.Settings.Connection";
pub const ACTIVE: &str = "org.freedesktop.NetworkManager.Connection.Active";

/// NMDeviceType: the one we look for.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceType {
    Wifi = 2,
}

/// NMDeviceState: the ones we look at.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceState {
    Unmanaged = 10,
    Activated = 100,
    Failed = 120,
}

/// NMDeviceStateReason: the ones that tell failed joins apart.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceReason {
    /// No address from the network (IP configuration unavailable).
    NoAddress = 5,
    /// The secrets, meaning the password, are missing or were refused.
    NoSecrets = 7,
}

/// NMActiveConnectionState: the ones we look at.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveState {
    Activated = 2,
    Deactivated = 4,
}

/// NMActiveConnectionStateReason: the ones that tell failed joins apart.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveReason {
    /// No address from the network.
    NoAddress = 5,
    /// The secrets, meaning the password, are missing or were refused.
    NoSecrets = 9,
}

/// NM80211ApFlags: privacy, meaning WEP or any encryption.
pub const AP_PRIVACY: u32 = 0x1;
/// NM80211ApSecurityFlags: key management PSK (WPA personal), 802.1X (enterprise), SAE (WPA3).
pub const KEY_MGMT_PSK: u32 = 0x100;
pub const KEY_MGMT_8021X: u32 = 0x200;
pub const KEY_MGMT_SAE: u32 = 0x400;
