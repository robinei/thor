# Window management: candidate improvements

Written 2026-10-26, from the design as it is (`architecture.md`, "Windows") and the apps run so far
(`clamshell/tests/real-apps.py`), not from reported bugs. Each item says what to settle first; none is
decided.

Today each screen shows one window: the top screen the selected window of the group in front (an
entry's, or the window's own), the bottom screen the window moved there (remembered per program).
Dialogs are centred at their own size over their window, which is darkened under them. Switching is through the dock, Home, and the program's previous
and next window.

## 1. Launchers handing over to their game

Steam, Heroic and Lutris open a launcher window, then the game opens its own. To find out, and settle:

- whether the game comes in front, and the launcher gets out of the way;
- where a quit game leaves you: shelf, or the launcher;
- whether play time is recorded against the right entry (a Proton game's especially).

The most common path on a handheld. Steam is the case `real-apps.py` doesn't cover: it allows one
instance per user, so the desktop's has to be quit while it runs.

## 2. A program's windows shown together on the top screen

Built (`session/overview.rs`), as below, except that the windows go on drawing in their cells rather than
showing their last frame: shown, they get their frame callbacks as the window in front does. `clamshell
ctl window overview` opens and closes it as the tap does.

The bar has no room for another tab, and the dock already switches between programs; what is missing
is a choice between one program's windows. So: a tap on the dock entry of the program already in front
shows all its windows on the top screen at once. A tap on one brings it in front; another tap on the
entry, or B, goes back to the window that was in front. With one window, the tap does nothing.

- Layout: a grid by the count (two side by side, three or four in two rows, ...), each window scaled
  into its cell in its own shape. Predictable, and windows don't jump as the count changes; bin packing
  can replace it later if shapes vary a lot.
- Only top-level windows: a dialog stays with its parent.
- The pad as well as touch: the D-pad moves between the windows, A picks one, B goes back, as the focus
  ring does on the screens.
- Each window's last frame, scaled from the textures the compositor already holds: no new buffers. A
  hidden window (a paused game's) shows where it left off.
- The state holds that the overview of a program is open, and the choice; the compositor lays it out
  and draws it, as it places windows today. Taps on the top screen hit the cells.
- The bottom screen's window stays out of it; moving between the screens stays its own action.

This also answers programs with many windows on the top screen (a browser, a launcher and its friends
list), which previous and next handle clumsily beyond two or three.

## 3. Dual-screen apps placed at once

DS and 3DS emulators (melonDS, Azahar) open a second window for the lower screen, or can be set to. The
rule that sends a program's windows below learns only from what the user does. A few rules shipped for
known emulators would place them right at the first launch: facts about apps, not personal presets.

## 4. Apps asking for attention while a game is in front

An app asking to be shown could become a notification to tap, rather than taking the top screen from
the game. What happens today is to be checked first.

## Order

2 is built; 1 next. 3 and 4 as they come up in use.
