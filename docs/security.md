# Security notes

The Thor is a single-user device: everything ClamShell does runs as that user, in the user's systemd
manager. These are the places where something is kept, accepted or run that a reader might ask about.

## The clipboard

ClamShell keeps the clipboard's text (see `crates/system/src/clipboard.rs`): it offers it again when the
program that copied it quits, and writes it to `$XDG_RUNTIME_DIR/clamshell/clipboard` (mode 0600, on a
tmpfs only the user can read, gone at a reboot) so that a ClamShell started again offers it too. Text
copied anywhere stays there until something else is copied or the device reboots: a password copied from
a terminal or a game's chat too.

A password manager's copy is marked `x-kde-passwordManagerHint` (KeePassXC and others do this): such a
copy is not kept, not written, not mirrored to the primary selection and not pasted into ClamShell's
fields, and what was kept is forgotten. The manager clears the clipboard itself after a while; keeping
the copy would undo that.

## The control pipe

`$XDG_RUNTIME_DIR/clamshell-control` (a FIFO, mode 0600; see `src/control.rs`) takes commands as the
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
runs) can set the first password, and with it gain sudo and ssh: the same trust as the control pipe, and
the reason to set a password early (ClamShell's Device view opens on the first start for it). Passwords
are 8 characters at least; sshd (`/etc/ssh/sshd_config.d/50-thor.conf`) takes only `thor`, no root, 3
tries a connection (and OpenSSH's own penalties for repeated failures).

sudo asks the password, per terminal, as usual. During a maintenance pass (`thor-maintain`) it is shared:
the pass asks it once, writes `/etc/sudoers.d/zz-thor-maintenance` (`timestamp_type=global`) so that
Claude's commands, which have no terminal, use it, keeps it fresh, and removes the file and the
timestamp when it ends; so while a pass runs, every process of `thor` has sudo without a prompt. A pass
killed outright (SIGKILL) leaves the file until the next boot, which removes it
(`tmpfiles.d/thor-maintenance.conf`), and a timestamp for up to 15 minutes.
