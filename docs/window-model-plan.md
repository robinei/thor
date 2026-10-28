# Plan, 2026-10-28: the session's windows computed from state where they can be

The third review round's bugs in the window model were mostly a stored value that one change forgot to
keep in step: the keyboard on a window no screen shows (a dialog opening behind, a window sent back from
below, the window below closing under its dialog), `top` holding the topmost window's id so that stepping
and Home with B acted on the wrong one, and a found program's later window outside its group. What can be
computed from the windows is computed instead; what is a decision or history stays stored.

Each step ends in commits that pass `tools/check.sh`, with the randomized test at 3000 seeds; the VM
suite runs at the end.

## Steps

- [x] 1. **The screens' slots hold stacks' roots.** `top` (and each group's selection) holds the root of
  the stack in front, as `bottom` already does; the window taking the input is computed (`topmost`).
  Opening, closing and selecting a dialog no longer move the slot; the root closing hands it to its first
  dialog, as for the bottom screen.
- [ ] 2. **The keyboard computed.** What is stored is where the keyboard is, `Keys { Top, Bottom, Ui }`;
  the window with it is that screen's topmost, or none for ClamShell's UI or a screen with nothing on it.
  The compositor's reports and the session's choices set where, by the screen of the window they name (a
  window on neither is ignored). A window no screen shows can't have the keyboard.
- [ ] 3. **A window's owner by its process at every settle.** The rule that a window of the process and
  app id of one open is that one's program moves from `open` into `Session::running_of`, so that the
  settle's placing of owned windows applies it to windows already open too.

Stays stored, as history rather than state: a window's group as claimed (a file's window taken by the
entry waiting for it, a service dialog given to the program in front), and a launcher's promotion for its
dialogs (`FrontLaunch::promoted`: who brought the launcher in front isn't in the windows).
