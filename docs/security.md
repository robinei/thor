# Security notes

The Thor is a single-user device: everything ClamShell does runs as that user, in the user's systemd
manager. These are the places where something is kept, accepted or run that a reader might ask about.

## The clipboard

ClamShell keeps the clipboard's text (see `crates/system/src/session/clipboard.rs`): it offers it again when the
program that copied it quits, and writes it to `$XDG_RUNTIME_DIR/clamshell/clipboard` (mode 0600, on a
tmpfs only the user can read, gone at a reboot) so that a ClamShell started again offers it too. Text
copied anywhere stays there until something else is copied or the device reboots: a password copied from
a terminal or a game's chat too.

A password manager's copy is marked `x-kde-passwordManagerHint` (KeePassXC and others do this): such a
copy is not kept, not written, not mirrored to the primary selection and not pasted into ClamShell's
fields, and what was kept is forgotten. The manager clears the clipboard itself after a while; keeping
the copy would undo that.

Any Wayland client can read and replace the clipboard: `ext_data_control` is offered to every client
(`crates/compositor/src/protocols.rs`), as wl-clipboard and clipboard managers need, and X11 clients
read the bridged clipboard as X11 always lets them. So a game, a Wine program or an app can see what
was copied, including a password typed into a terminal; a password manager's marked copy is still
dropped by ClamShell's keeper, but the program that copied it holds it until it clears it. Restricting
the protocol to ClamShell's own thread was tried and reverted: it broke wl-clipboard and the tests'
clipboard tool.

## The control pipe

`$XDG_RUNTIME_DIR/clamshell-control` (a FIFO, mode 0600; see `clamshell/src/control/`) takes commands as the
device's keys and the user's taps would make them: `clamshell ctl` and the tests use it. Any process of
the user can write to it, and so can tap ClamShell's screens and type into the window in front through
ClamShell's virtual keyboard (`type TEXT`). Android's apps run as other users inside Waydroid's
container and can't reach it; the `pyclip` stand-in Waydroid's clipboard sharing uses runs
`clamshell --clipboard get|set` only.

## Programs adopted and ended

Programs ClamShell didn't start (found by a window or a tray icon) are moved into scopes of ours in the
user manager, and Quit or Force kill stops or kills those scopes (see process-control.md). The user
manager only moves and signals the user's own processes; nothing here needs or asks for more rights. A
process a systemd service runs is never moved.

## Archives

An archive opened in the Files tab is mounted with ratarmount (FUSE; Python's and libarchive's parsers)
as the user, in a service of its own, read-only; its index is kept in `~/.cache/clamshell/archive-index`
(see files-tab.md). A damaged or hostile archive can make ratarmount fail or hang: it fails the folder's
listing with a notice, or leaves a service to stop; it can't change the archive or anything else.

## Android's ADB server

ClamShell starts its own ADB server on `tcp:localhost:5039`. Only local clients can use its authorised
Waydroid transport; the server must never listen on all network interfaces. The Android launch test
pins that binding without starting a real server.

## Waydroid's Wayland filter

`clamshell --wayland-filter` sits between Waydroid and the compositor and reads the Wayland messages
Android sends (see android.md). It parses them by hand, without `unsafe` code: malformed input can at
worst desynchronise that one connection, which ends Android's windows (the session is bound to the
filter and restarts with the next launch).

## Sleep, idle and the network

What ClamShell reads to decide (input devices, the compositor's idle protocol, the user's cgroup's CPU,
`/proc/net/dev`) is read-only. Suspending goes through logind as the session's user, as the session's
polkit rules allow.

## The account, sudo and ssh

The session's user, `thor`, logs in on its own (tty1's autologin: the device is the session) with its
account locked until a password is set. `thor-account` (thor-system's, root through pkexec; polkit allows
the active local session's user without asking: `50-thor.rules`) sets the first password without asking
for one, changes it only with the current one (checked as PAM does, by `unix_chkpwd` as the user), and
turns sshd on only once there is a password. So any program running as `thor` (a game, a script Steam
runs) could set the first password, and with it gain sudo and ssh, if it ran while there was none: which
is why ClamShell's Setup sets it before anything else of the user's runs (below). Passwords
are 8 characters at least; sshd (`/etc/ssh/sshd_config.d/50-thor.conf`) takes only `thor`, no root, 3
tries a connection (and OpenSSH's own penalties for repeated failures).

sudo asks the password, per terminal, as usual. During a maintenance pass (`thor-maintain`) it is shared:
the pass asks it once, writes `/etc/sudoers.d/zz-thor-maintenance` (`timestamp_type=global`) so that
Claude's commands, which have no terminal, use it, and keeps it fresh; so while a pass runs, every
process of `thor` has sudo without a prompt. With the file it starts a root unit
(`thor-maintenance-end-<pid>`) that waits for the pass's process to end, however it ends (SIGKILL, or
after the device slept past the timestamp), and removes the file; the pass forgets the timestamp as it
ends, and one killed outright leaves it for up to 15 minutes. Should that unit be cut off too, the next
boot removes the file (`tmpfiles.d/thor-maintenance.conf`).

## The account and the first password

The image starts with the session's user in `wheel` and no password: no sudo and no ssh until one is
set. `thor-account` (run as root through pkexec, which polkit allows the active session without asking)
sets the first one without asking for one, which any process of the user could do. So the first password
is Setup's first step, which can't be skipped, and nothing of the user's is started until the account has
one (`clamshell_state::setup::account::Account::lets_programs_run`): every launch is held, and "Start with
ClamShell" waits, until the account, read at every start, reads with a password. Unread, or unreadable
(the helper failing, as with no polkit agent yet), it is held as one without: Setup shows over anything
else, reading it again, and its password step can't be passed; only a machine with no helper at all lets
programs run without one. While Setup shows, the second screen shows only the keyboard. What runs before
the password is ClamShell and the image's own services. Changing a password asks for the current one.

## The power group's files

The session's user is in the `power` group, which writes what ClamShell's settings set (the CPU and GPU
caps, the battery's limits: `73-thor-power.rules`) and the top panel's calibration
(`/var/lib/thor/panel-calibration`). Root reads only root-owned paths, or regular files in root-owned
directories: `/var/lib/thor` is root's (`tmpfiles.d/thor-panel.conf`), so the group can change the
calibration file but not put a symlink in its place, and its readers (`thor-selftest`,
`/usr/lib/thor/panel-calibration`) read it only as a regular file, never through a symlink.

## Input devices

The session's user is in the `input` group, for ClamShell's virtual pad and keyboard (uinput) and for
reading the hardware keys. So any program of the user can read the raw input devices (keys typed on an
attached keyboard too) and make virtual ones. That is accepted on a single-user device: no program of the
user runs before the first password is set (above), so none can type it.

## Notifications

ClamShell serves `org.freedesktop.Notifications`, so any program on the session bus can show a
notification over the bottom screen, and a critical one wakes the screens. What it shows is plain text:
markup is taken out, icons and images are never read, and at most 32 are held, the oldest closed first.

## Tray icons

A tray item names its icon and a folder for it over the bus. The name must be a name (no `/`, no `..`),
and an SVG icon's `<image>` is drawn only from data inside the file, never from a file it names, so a
program can't make ClamShell show other files of the user's.
