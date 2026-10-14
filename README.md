# thor (working name)

Linux on the AYN Thor: a system program for its two screens, its pad and its power, plus the setup around
it (Arch Linux ARM, packages built on the device). Planning lives in the GPD repo's `docs/plans.md` for now
(`~/Code/gpdwin`); it moves here once the first parts run.

## sysprog (working name)

`sysprog/` is the system program: one process with one in-memory state for shelf (the game list on the top
screen), the second screen (bar, modes, the selected or running game's settings), and later the input
path (virtual pad, mouse emulation, touchpad) and power policy. Rust, a Cargo workspace:

| Crate | What |
|---|---|
| `crates/core` | the state and its rules: events in, dirty screens and effects out; no I/O, unit-tested |
| `crates/policy` | threads that watch the system and send events (today: the clock) |
| `crates/library` | the game library (today: demo games) |
| `crates/ui` | Wayland (smithay-client-toolkit), CPU drawing (tiny-skia, fontdue), the views, pad navigation |
| `src/main.rs` | wires them together |

No panics by construction: `unwrap`, `expect`, `panic!` and slice indexing are denied outside tests.

**Run on a desktop** (any Wayland compositor with xdg-shell; needs `fontconfig` and a font):

    cd sysprog
    cargo run                         # two windows at half the Thor's size: top and bottom
    cargo run -- --snapshot /tmp      # top.png and bottom.png at the Thor's size, no Wayland
    cargo test --workspace
    cargo clippy --workspace --all-targets

Keys stand in for the pad: arrows move focus (across both screens, as if the bottom one were below the
top one), Enter or Space is A, Escape quits. Clicks and touches tap. On the Thor (sway), the bottom view
is a layer-shell surface on output `DSI-1` (`SYSPROG_BOTTOM_OUTPUT` to change it).

Licence: MIT.
