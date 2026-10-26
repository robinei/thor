# Window management: candidate improvements

Written 2026-10-26, from the design as it is (`architecture.md`, "Windows") and the apps run so far
(`clamshell/tests/real-apps.py`), not from reported bugs. Each item says what to settle first; none is
decided.

Today each screen shows one window: the top screen the selected window of the group in front (an
entry's, or the window's own), the bottom screen the window moved there (remembered per program).
Dialogs are centred at their own size. Switching is through the dock, Home, and the program's previous
and next window.

## 1. Launchers handing over to their game

Steam, Heroic and Lutris open a launcher window, then the game opens its own. To find out, and settle:

- whether the game comes in front, and the launcher gets out of the way;
- where a quit game leaves you: shelf, or the launcher;
- whether play time is recorded against the right entry (a Proton game's especially).

The most common path on a handheld. Steam is the case `real-apps.py` doesn't cover: it allows one
instance per user, so the desktop's has to be quit while it runs.

## 2. A window switcher on the bottom screen

Every open window, grouped by program, perhaps with live thumbnails; a tap brings one in front, or
moves it down. The windows tab today shows only the window below. The second screen suits seeing
everything open at once.

## 3. Dual-screen apps placed at once

DS and 3DS emulators (melonDS, Azahar) open a second window for the lower screen, or can be set to. The
rule that sends a program's windows below learns only from what the user does. A few rules shipped for
known emulators would place them right at the first launch: facts about apps, not personal presets.

## 4. Apps asking for attention while a game is in front

An app asking to be shown could become a notification to tap, rather than taking the top screen from
the game. What happens today is to be checked first.

## 5. Programs with many windows on the top screen

A browser, or a launcher with its friends list: previous and next work within the program, but
switching between more than two or three full-screen windows that way may be clumsy. Item 2 may answer
it.

## Order

1 first, then 2. 3 to 5 as they come up in use.
