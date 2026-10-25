//! Package scripts consume the same facts as the session, without repeating device identification.
use clamshell_device::{Profile, TouchSelector};

pub fn outputs(profile: &Profile) -> String {
    let mut out = format!(
        "output {} transform {} position 0 0\noutput {} transform {}\n",
        profile.top.connector,
        profile.top.rotation.degrees(),
        profile.bottom.connector,
        profile.bottom.rotation.degrees()
    );
    for touch in profile.touchscreens {
        let selector = match touch.selector {
            TouchSelector::Named(name) => format!("\"{name}\""),
            TouchSelector::All => "type:touch".into(),
        };
        out.push_str(&format!("input {selector} map_to_output {}\n", profile.screen(touch.screen).connector));
    }
    out
}
pub fn panel(profile: &Profile) -> serde_json::Value {
    let calibration = profile.refresh_calibration.map(|calibration| {
        serde_json::json!({
            "program": calibration.program, "step": calibration.step,
            "rates": calibration.rates.iter().map(|rate| serde_json::json!({
                "hz": rate.hz, "attribute": rate.attribute, "low": rate.low, "high": rate.high,
                "tolerance_millihz": rate.tolerance_millihz, "big_error_millihz": rate.big_error_millihz,
            })).collect::<Vec<_>>(),
        })
    });
    serde_json::json!({ "top": profile.top.connector, "bottom": profile.bottom.connector, "calibration": calibration,
        "colour": { "top": colour(profile.top.colour), "bottom": colour(profile.bottom.colour) } })
}

fn colour(colour: clamshell_device::colour::PanelColour) -> serde_json::Value {
    use clamshell_device::colour::Transfer;
    let xy = |point: clamshell_device::colour::Xy| [point.x, point.y];
    serde_json::json!({
        "primaries_millionths": { "red": xy(colour.primaries.red), "green": xy(colour.primaries.green), "blue": xy(colour.primaries.blue), "white": xy(colour.primaries.white) },
        "transfer": match colour.transfer { Transfer::Gamma22 => "gamma22" },
        "luminance_millinits": colour.luminance.map(|luminance| serde_json::json!({ "peak": luminance.peak, "frame_average": luminance.frame_average, "black": luminance.black })),
        "link_bits_per_channel": colour.link_bits_per_channel, "hdr_capable": colour.hdr_capable,
        "sdr_white_millinits": colour.sdr_white,
    })
}
