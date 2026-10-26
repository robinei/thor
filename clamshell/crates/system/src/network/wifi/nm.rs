//! The parts of NetworkManager's D-Bus interfaces on the system bus that the Wi-Fi tab uses, and the
//! codes it reports states and reasons with.

pub(crate) const SERVICE: &str = "org.freedesktop.NetworkManager";
pub(crate) const PATH: &str = "/org/freedesktop/NetworkManager";
pub(crate) const MANAGER: &str = "org.freedesktop.NetworkManager";
pub(crate) const DEVICE: &str = "org.freedesktop.NetworkManager.Device";
pub(crate) const WIRELESS: &str = "org.freedesktop.NetworkManager.Device.Wireless";
pub(crate) const ACCESS_POINT: &str = "org.freedesktop.NetworkManager.AccessPoint";
pub(crate) const SETTINGS_PATH: &str = "/org/freedesktop/NetworkManager/Settings";
pub(crate) const SETTINGS: &str = "org.freedesktop.NetworkManager.Settings";
pub(crate) const CONNECTION: &str = "org.freedesktop.NetworkManager.Settings.Connection";
pub(crate) const ACTIVE: &str = "org.freedesktop.NetworkManager.Connection.Active";

/// NMDeviceType: the one we look for.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeviceType {
    Wifi = 2,
}

/// NMDeviceState: the ones we look at.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeviceState {
    Unmanaged = 10,
    Activated = 100,
    Failed = 120,
}

/// NMDeviceStateReason: the ones that tell failed joins apart.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeviceReason {
    /// No address from the network (IP configuration unavailable).
    NoAddress = 5,
    /// The secrets, meaning the password, are missing or were refused.
    NoSecrets = 7,
}

/// NMActiveConnectionState: the ones we look at.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActiveState {
    Activated = 2,
    Deactivated = 4,
}

/// NMActiveConnectionStateReason: the ones that tell failed joins apart.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActiveReason {
    /// No address from the network.
    NoAddress = 5,
    /// The secrets, meaning the password, are missing or were refused.
    NoSecrets = 9,
}

/// NM80211ApFlags: privacy, meaning WEP or any encryption.
pub(crate) const AP_PRIVACY: u32 = 0x1;
/// NM80211ApSecurityFlags: key management PSK (WPA personal), 802.1X (enterprise), SAE (WPA3).
pub(crate) const KEY_MGMT_PSK: u32 = 0x100;
pub(crate) const KEY_MGMT_8021X: u32 = 0x200;
pub(crate) const KEY_MGMT_SAE: u32 = 0x400;
