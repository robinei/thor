# How the code would be criticised

Written 2026-10-22, after the quality fixes (`quality-plan.md`) and the source reorganisation
(`organisation-plan.md`). The obvious structural complaints are gone; what is left are deeper choices. Roughly
in order of how often a reviewer would raise them. Written by the same agent that did most of the
reorganising, so a fresh outside review would be a fairer test.

## 1. No CI

Every quality claim rests on the pre-commit hook (`tools/check.sh`) on one machine. There is no run history
for a visitor to see, and the device-only paths (sensor hub, gyro, rumble) are `#[ignore]` tests that never
run. The cheapest gap to close; GitHub Actions was deferred in the quality plan.

## 2. The history

About 140 commits in one day, each with a `Claude-Session:` trailer, and earlier long "review fixes"
subjects. To someone looking for AI-generated work that is the evidence, whatever the code is like. The plan
and review files in `docs/` (quality plan, organisation plan, review) read the same way: process notes
rather than project documentation.

## 3. `State` is one large object

Core's rules are `impl State` blocks across about 40 files. `State`'s fields are private to the crate root,
so every module can reach every field. The directories group the code, but nothing enforces the boundaries
between areas: a god object with good file organisation.

## 4. Written by hand instead of taken from crates

- a protobuf codec, QMI framing and a Wayland wire proxy;
- MD5, CRC32, percent encoding;
- HTTP by running curl.

Each has a reason (few dependencies, a small binary, no async), but together they read as not-invented-here,
and the parsers are where subtle bugs live, covered only by unit tests.

## 5. Command-line tools and their text output

`timedatectl`, `wpctl`, `pw-dump`, `udisksctl`, `grim`, `systemd-run`, `bsdtar` and others. Their output
formats change between versions, and failures mostly end up as log lines. Errors are often
`Result<_, String>` or `Box<dyn Error>`.

## 6. The no-panic lints produce silent fallbacks

Banning `unwrap` and indexing gives code like `.get(i).copied().unwrap_or(0)`: a broken invariant becomes a
quietly wrong value instead of a crash. The `debug_assert!` convention covers this only partly; some readers
prefer the crash.

## 7. Visibility widened by the splits

Moving code into sibling files opened things up:

- the canvas's fields are `pub(super)`;
- two fields of the Waydroid proxy's state are `pub(super)`, only for its tests;
- most of the dispatch methods in core's `apply.rs` are `pub(crate)`.

Correct, but the encapsulation is looser than before.

## 8. Placements someone will dispute

- the dock under core's `session/`;
- the settings router in `wiring/settings.rs` while its struct is `Tuning`;
- the MangoHud config writer under `readings/` although launching uses it;
- `device.rs` left at the system crate's top level.

## 9. The comment style

Dense, colon-heavy prose and long lines (rustfmt's width is set high). Consistent, but unusual enough that
some will find it hard to read.
