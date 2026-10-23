# ClamShell: how changes are made

The workspace is `clamshell/`; `tools/check.sh` is the check every commit passes (the pre-commit hook runs
it). Docs on design and plans are in `docs/`.

## The state (crates/state)

- `State` (lib.rs) holds the areas (shelf, session, bottom, radios, ...) and coordinates them; it has one
  `impl State`. An area owns its fields privately, with its rules and the getters the UI reads.
- A rule that needs another area's data gets it as a parameter; a change another area alone owns is that
  area's `&mut self` method, passed as narrowly as practical. A request that needs several areas is queued
  as an `Event` (as the rule's last act); State handles it after the rule returns.
- `apply` runs handle, settle, sync. Settle holds what depends on history, in an ordered list; sync diffs
  what the outside should have (`Wanted`) against what it was last sent. Something that can be computed
  from state is computed, not stored and kept in step.
- Outside code reads areas through `State`'s accessors and changes state only through events.
- No allocation on the per-event path or in drawing a frame (`tests/apply_alloc.rs` and the UI's no-alloc
  test hold this). No function in the state crate returns a `Vec`: append to one passed in.

## State machines and their tests

Every feature's state is described in `docs/state-machines.md`: its states, its transitions
(`state --event/condition--> state [effects]`, including failures, timeouts, things vanishing and repeated
events), and how it interacts with other areas and by which route. A new feature or a change to one updates
that document in the same change, and:

- each transition and each interaction has a test that asserts its outcome (effects in order, and the
  resulting state), not one that merely passes through it;
- an order that matters is pinned by a test of the exact sequence, and a comment says why the order matters;
- invariants that must hold after every event go into the randomized event-sequence test, and the
  feature's events into its generator;
- a transition that can't be tested from the state crate (it needs the device, sway or a service) is
  marked in the document with how it is checked instead (the nested-sway test, the VM suite, or on the
  device).

## Failures

Don't crash unless there's no way on; a failure handled by falling back is logged (`warn` when it
shouldn't happen, `debug` when expected or repeating on a timer).

## Writing

Comments, docs and commit messages are plain and matter-of-fact. Commits: short imperative subjects scoped
by area (`state/shelf: ...`), one concern each.
