//! Input: the controller's path (`pad`), the virtual touchpad, and the devices watched for
//! activity and hardware keys.

pub mod confirm;
mod flick;
mod keyboard;
pub(crate) mod pad;
pub(crate) mod touchpad;
pub(crate) mod watch;
