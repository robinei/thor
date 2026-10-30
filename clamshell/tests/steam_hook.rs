//! Steam's launch hook (`steam/steam-launch-wrapper`), run with a fake `clamshell`, `logger` and Valve's
//! wrapper on its PATH: what it finds the app id by, what it exports, and that the game runs whatever
//! fails. Whether Steam calls it, and that the variables reach a game in Valve's container, is checked
//! by hand (docs/runners.md).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Fakes {
    dir: tempfile::TempDir,
}

impl Fakes {
    /// `clamshell --launch-env` prints `env` (or fails with `fail`), `clamshell ctl` and `logger` write
    /// their arguments to files, and Valve's wrapper writes its arguments and the variables asked for.
    fn new(env: &str, fail: bool) -> Fakes {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let d = dir.path().display();
        let clamshell = if fail {
            format!("#!/bin/sh\n[ \"$1\" = ctl ] && {{ shift; echo \"$*\" >> {d}/ctl; exit 0; }}\necho 'launch-env: library.json: broken' >&2\nexit 1\n")
        } else {
            format!(
                "#!/bin/sh\n[ \"$1\" = ctl ] && {{ shift; echo \"$*\" >> {d}/ctl; exit 0; }}\necho \"$* preload=${{LD_PRELOAD:-}}\" >> {d}/asked\nprintf '{env}'\n"
            )
        };
        script(&bin.join("clamshell"), &clamshell);
        // (A message after `--`, else stdin's lines, as logger takes them.)
        script(&bin.join("logger"), &format!("#!/bin/sh\ncase \" $* \" in *\" -- \"*) echo \"$*\" >> {d}/log ;; *) cat >> {d}/log ;; esac\n"));
        script(
            &dir.path().join("valve"),
            &format!("#!/bin/sh\necho \"$*\" > {d}/ran\nfor v in A B LD_PRELOAD; do printenv $v >> {d}/ran || echo - >> {d}/ran; done\n"),
        );
        Fakes { dir }
    }

    /// Runs the hook with `args` and the variables `vars`, Valve's wrapper being `valve` under the fakes'
    /// folder.
    fn run(&self, args: &[&str], vars: &[(&str, &str)], valve: &str) {
        let hook = Path::new(env!("CARGO_MANIFEST_DIR")).join("steam/steam-launch-wrapper");
        let path = format!("{}:{}", self.dir.path().join("bin").display(), std::env::var("PATH").unwrap());
        let status = Command::new(hook)
            .args(args)
            .env_clear()
            .env("PATH", path)
            .env("HOME", self.dir.path())
            .env("CLAMSHELL_STEAM_WRAPPER", self.dir.path().join(valve))
            .envs(vars.iter().copied())
            .stdin(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.path(name)).unwrap_or_default()
    }

    /// The log once it holds `text`: clamshell's errors reach it through a logger of their own, which
    /// may still be writing when the hook has execed the game.
    fn logged(&self, text: &str) -> String {
        for _ in 0..100 {
            if self.read("log").contains(text) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        self.read("log")
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }
}

fn script(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

const GAME: [&str; 6] = ["--", "reaper", "SteamLaunch", "AppId=620", "--", "game"];

#[test]
fn the_games_variables_are_exported_before_valves_wrapper_runs() {
    let f = Fakes::new("A=1\\nB=x y\\nA=2\\n", false);
    // A as the user's launch options would set it: ClamShell's replaces it. Steam's overlay, preloaded
    // for the game, isn't preloaded into ClamShell.
    f.run(&GAME, &[("A", "launch options"), ("LD_PRELOAD", "overlay.so")], "valve");
    assert_eq!(f.read("asked"), "--launch-env steam:620 preload=\n");
    assert_eq!(f.read("ran"), "-- reaper SteamLaunch AppId=620 -- game\n2\nx y\noverlay.so\n", "the arguments unchanged, the later A winning");
    // (The log has only ld.so's complaint that overlay.so, which this test doesn't have, isn't there.)
    assert!(!f.read("log").contains("started without"), "{}", f.read("log"));
    // Without AppId= among the arguments, Steam's SteamAppId.
    let f = Fakes::new("A=1\\n", false);
    f.run(&["--", "game"], &[("SteamAppId", "70")], "valve");
    assert_eq!(f.read("asked"), "--launch-env steam:70 preload=\n");
    assert!(f.read("ran").starts_with("-- game\n1\n"));
}

#[test]
fn a_failure_starts_the_game_as_it_is_and_says_so() {
    let f = Fakes::new("", true);
    f.run(&GAME, &[("A", "launch options")], "valve");
    assert_eq!(f.read("ran"), "-- reaper SteamLaunch AppId=620 -- game\nlaunch options\n-\n-\n");
    assert_eq!(f.read("ctl"), "launch-env failed steam:620\n");
    let log = f.logged("launch-env: library.json: broken");
    assert!(log.contains("launch-env: library.json: broken"), "clamshell's error: {log}");
    assert!(log.contains("steam:620: started without ClamShell's settings"), "{log}");
    // No app id: logged, nothing to tell ClamShell about.
    let f = Fakes::new("A=1\\n", false);
    f.run(&["--", "game"], &[], "valve");
    assert!(f.read("ran").starts_with("-- game\n-\n"));
    assert!(f.logged("no app id").contains("no app id"));
    assert_eq!(f.read("ctl"), "");
    // Valve's wrapper gone (Steam's update may remove it): the command after `--` runs.
    let f = Fakes::new("A=1\\n", false);
    let game = f.path("game");
    script(&game, &format!("#!/bin/sh\necho \"$* $A\" > {}\n", f.path("ran").display()));
    f.run(&["--", game.to_str().unwrap(), "AppId=620"], &[], "absent");
    assert_eq!(f.read("ran"), "AppId=620 1\n");
    assert!(f.logged("is missing").contains("is missing"));
}
