//! The environment a launch gets: one function for ClamShell's own launches and for Steam's launch hook
//! (`clamshell --launch-env`, docs/runners.md), so a game gets the same variables either way.

use std::path::Path;

use super::{Setting, Values, X86};

/// What a launch's environment takes from outside the settings.
#[derive(Debug, Clone, Copy, Default)]
pub struct LaunchFacts<'a> {
    /// The folder the x86 emulation presets are copied to (`~/.cache/clamshell/fex`), where Valve's
    /// runtime container, which replaces `/usr`, still sees them. None without a home; the presets'
    /// writer says so at the start.
    pub fex: Option<&'a Path>,
    /// The controllers SDL programs are to ignore, as `SDL_GAMECONTROLLER_IGNORE_DEVICES` takes them, if
    /// any (the grabbed controllers, `clamshell_system::input::pad::Taken`).
    pub ignore: Option<&'a str>,
}

/// Appends a launch's environment to `env`, later variables overriding earlier ones:
///
/// 1. What the settings acting at launch set ([`Setting::through_launch`]): the cap's FIFO and the x86
///    emulation preset.
/// 2. `SDL_GAMECONTROLLER_IGNORE_DEVICES`, the grabbed controllers.
/// 3. The Env tile's variables, `tile`: All's, the runner's, the entry's ([`super::Settings::env_in`]).
///
/// The Env tile comes last so that the user's own variables win over what ClamShell sets.
pub fn launch_env(values: &Values, facts: LaunchFacts, tile: &[(String, String)], env: &mut Vec<(String, String)>) {
    // A capped game presents in FIFO, so that it waits for the frame callbacks the compositor paces to the
    // cap: in mailbox or immediate it wouldn't. Mesa's Vulkan WSI, and GL's swap interval kept at 1 or
    // more (`vblank_mode` 3). Set at launch, so a cap set while the game runs reaches only games
    // presenting in FIFO already.
    if values.get(Setting::FpsCap) != 0 {
        env.extend([("MESA_VK_WSI_PRESENT_MODE".into(), "fifo".into()), ("vblank_mode".into(), "3".into())]);
    }
    // FEX reads an app config named by `FEX_APP_CONFIG` whichever Proton runs it (Armada's way).
    if let (Some(preset), Some(dir)) = (values.choice::<X86>().preset(), facts.fex) {
        env.push(("FEX_APP_CONFIG".into(), dir.join(preset).to_string_lossy().into_owned()));
    }
    if let Some(ignore) = facts.ignore {
        env.push(("SDL_GAMECONTROLLER_IGNORE_DEVICES".into(), ignore.into()));
    }
    env.extend(tile.iter().cloned());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }

    /// The order is pinned: a later variable overrides an earlier one of the same name, so the Env tile's
    /// must come after what ClamShell sets, for the user's own to win.
    #[test]
    fn the_settings_then_the_controllers_then_the_env_tile() {
        let mut values = Values::default();
        let facts = LaunchFacts { fex: Some(Path::new("/h/.cache/clamshell/fex")), ignore: Some("0x2dc8/0x6003") };
        let tile = vars(&[("vblank_mode", "0"), ("A", "1")]);
        let mut env = Vec::new();
        launch_env(&values, facts, &tile, &mut env);
        assert_eq!(env, vars(&[("SDL_GAMECONTROLLER_IGNORE_DEVICES", "0x2dc8/0x6003"), ("vblank_mode", "0"), ("A", "1")]), "nothing set: no cap, Proton's own");
        values.set(Setting::FpsCap, 40);
        values.set(Setting::X86, X86::Safe as u16);
        env.clear();
        launch_env(&values, facts, &tile, &mut env);
        assert_eq!(
            env,
            vars(&[
                ("MESA_VK_WSI_PRESENT_MODE", "fifo"),
                ("vblank_mode", "3"),
                ("FEX_APP_CONFIG", "/h/.cache/clamshell/fex/safe.json"),
                ("SDL_GAMECONTROLLER_IGNORE_DEVICES", "0x2dc8/0x6003"),
                ("vblank_mode", "0"),
                ("A", "1"),
            ])
        );
        values.set(Setting::X86, X86::Fast as u16);
        env.clear();
        launch_env(&values, LaunchFacts::default(), &[], &mut env);
        assert_eq!(env, vars(&[("MESA_VK_WSI_PRESENT_MODE", "fifo"), ("vblank_mode", "3")]), "no folder for the presets, no controllers");
        env.clear();
        launch_env(&values, LaunchFacts { fex: facts.fex, ignore: None }, &[], &mut env);
        assert_eq!(env.last(), Some(&("FEX_APP_CONFIG".to_owned(), "/h/.cache/clamshell/fex/fast.json".to_owned())));
    }
}
