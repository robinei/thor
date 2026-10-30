//! The x86 emulation presets ([`clamshell_state::settings::X86`]): Armada's Fast and Compatible
//! (`usr/share/armada/fex-profiles.json`), as FEX's app config files, `{"Config": {...}}` with string
//! values as FEX's own Config.json has them. A game is given one by `FEX_APP_CONFIG`, which every FEX reads,
//! the one inside Proton ARM64 (ARM64EC) included. They are built into ClamShell and written to
//! `~/.cache/clamshell/fex` at its start, since Valve's runtime container, which Proton's games run in,
//! replaces `/usr` but shares the home.

use std::path::Path;

/// The presets, by the file name [`clamshell_state::settings::X86::preset`] gives.
const PRESETS: [(&str, &str); 2] = [("fast.json", include_str!("fex/fast.json")), ("safe.json", include_str!("fex/safe.json"))];

/// Writes the presets into `dir` where they are missing or differ. Run at ClamShell's start and by
/// `clamshell --launch-env`. A failure is logged: the games chosen to get a preset then run as Proton sets
/// FEX up.
pub fn write_presets(dir: Option<&Path>) {
    let Some(dir) = dir else {
        log::warn!("x86 emulation: no cache folder (no HOME): the presets aren't written, and games run as Proton sets FEX up");
        return;
    };
    for (name, text) in PRESETS {
        let path = dir.join(name);
        if std::fs::read(&path).is_ok_and(|on_disk| on_disk == text.as_bytes()) {
            continue;
        }
        match crate::os::fs::write_file(&path, text.as_bytes()) {
            Ok(()) => log::info!("x86 emulation: {} written", path.display()),
            Err(e) => log::warn!("x86 emulation: {}: {e}: games given it run as Proton sets FEX up", path.display()),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use clamshell_state::settings::{Choice, X86};

    #[test]
    fn every_preset_is_written_once_and_kept_while_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let fex = dir.path().join("fex");
        write_presets(Some(&fex));
        for x86 in [X86::Fast, X86::Safe] {
            let file = fex.join(x86.preset().unwrap());
            let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
            assert_eq!(json["Config"]["TSOEnabled"], if x86 == X86::Fast { "0" } else { "1" }, "{x86:?}");
            assert_eq!(json["Config"].as_object().unwrap().len(), 6);
        }
        assert_eq!(X86::of(0).preset(), None, "Proton's own: no file");
        let fast = fex.join("fast.json");
        std::fs::write(&fast, "{}").unwrap();
        let before = std::fs::metadata(fex.join("safe.json")).unwrap().modified().unwrap();
        write_presets(Some(&fex));
        assert_eq!(std::fs::read_to_string(&fast).unwrap(), include_str!("fex/fast.json"), "a changed one is written again");
        assert_eq!(std::fs::metadata(fex.join("safe.json")).unwrap().modified().unwrap(), before, "an unchanged one is left");
    }
}
