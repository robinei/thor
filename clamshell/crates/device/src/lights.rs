//! The lights under the sticks: where their LEDs are, and the order their colour channels are written in.
//! Neither can be detected: the LEDs are multicolour LED class devices whose `multi_index` names the
//! channels red, green, blue, but on the Thor the first value written drives blue and the last red.

/// A colour channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Red,
    Green,
    Blue,
}

/// A device's stick lights.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StickLights {
    /// Each stick's LEDs, as LED class directories relative to /sys, each with `brightness`,
    /// `max_brightness` and `multi_intensity`.
    pub left: &'static [&'static str],
    pub right: &'static [&'static str],
    /// The channel each value written to `multi_intensity` drives, in the order written.
    pub order: [Channel; 3],
}

impl StickLights {
    /// Four LEDs round each stick (HTR3212). The order is ROCKNIX's (`quirks/devices/AYN Thor/bin/`
    /// `analog_sticks_ledcontrol` writes blue, green, red; `battery_led_status` writes red as `0 0 255`).
    pub const fn thor() -> StickLights {
        StickLights {
            left: &[
                "devices/platform/multi-ledl1/leds/rgb:l1",
                "devices/platform/multi-ledl2/leds/rgb:l2",
                "devices/platform/multi-ledl3/leds/rgb:l3",
                "devices/platform/multi-ledl4/leds/rgb:l4",
            ],
            right: &[
                "devices/platform/multi-ledr1/leds/rgb:r1",
                "devices/platform/multi-ledr2/leds/rgb:r2",
                "devices/platform/multi-ledr3/leds/rgb:r3",
                "devices/platform/multi-ledr4/leds/rgb:r4",
            ],
            order: [Channel::Blue, Channel::Green, Channel::Red],
        }
    }

    /// `rgb` as the values written to `multi_intensity`, in [`StickLights::order`].
    pub fn written(&self, [r, g, b]: [u8; 3]) -> [u8; 3] {
        self.order.map(|c| match c {
            Channel::Red => r,
            Channel::Green => g,
            Channel::Blue => b,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_thors_channels_are_written_blue_green_red() {
        let thor = StickLights::thor();
        assert_eq!(thor.written([255, 0, 0]), [0, 0, 255], "red, as ROCKNIX writes it");
        assert_eq!(thor.written([255, 20, 0]), [0, 20, 255], "ROCKNIX's orange");
        assert_eq!(thor.written([1, 2, 3]), [3, 2, 1]);
        assert_eq!((thor.left.len(), thor.right.len()), (4, 4));
    }
}
