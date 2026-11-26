# NagaButtons

A lightweight macOS menu bar app that turns the Razer Naga V2 HyperSpeed's side buttons, a top button and a Hypershift layer into **25 real mouse buttons** (BUTTON6 to BUTTON30).

Because they are mouse buttons, not keys, keyboard modifiers work with every one of them, just like Shift + left click. Side 1, Shift + side 1, Command + Option + side 1 and Hypershift + Shift + side 1 are all different bindings: 25 buttons × 16 modifier combinations.

> **The app you bind them in must accept mouse buttons above 5.** NagaButtons sends the buttons; whether an application can bind them is up to that application. Tested with World of Warcraft on macOS. Many macOS apps only read buttons 1 to 5.

## Button map

| Control | Mouse button |
|---|---|
| Side 1 to 12 | BUTTON6 to BUTTON17 |
| Top front | BUTTON18 |
| Top rear | Hypershift (hold it; sends nothing) |
| Hypershift + side 1 to 12 | BUTTON19 to BUTTON30 |

## Why not keys?

The mouse normally types `1` to `=`. macOS cannot tell those keys from your keyboard's, and it keeps modifier state per keyboard, so Shift on the keyboard plus a mouse key often arrives without Shift. Remapping to rarer keys (F13 to F20, keypad keys) runs out of keys before 25, collides with full-size keyboards, and any modifier you build into a layer can no longer be added from the keyboard. Mouse buttons have none of these problems: macOS supports 32 of them and no keyboard sends them.

## How it works

- Reads the raw HID input of the Naga's keyboard interface only (USB receiver `1532:00b4`). Other keyboards are never touched, so your keyboard and numpad work as usual.
- Mutes those 14 keys on the Naga's keyboard interfaces with `hidutil` `UserKeyMapping`, so they never reach macOS as typing. Quitting or pausing gives them back.
- Posts each press as a mouse button with the keyboard modifiers held at that moment.
- Two small processes. The daemon, `nagabuttonsd`, reads the mouse and posts the buttons; it links no AppKit and uses about 4 MB. The menu bar app only shows the status and keeps the daemon running, and starts it again if it dies.
- A press that reaches the daemon more than 50 ms late is dropped, not posted: in a game a missed press costs less than a late one. A release is always posted.
- Event-driven: no event tap, no network.

## Requirements

- macOS 11 or later. The release build is for Apple silicon; Intel Macs can build from source.
- Razer Naga V2 HyperSpeed on its HyperSpeed USB receiver (2.4 GHz). Bluetooth is not supported.
- The mouse's onboard profile:
  - Side buttons 1 to 12 send `1` … `0`, `-`, `=`. This is the factory setting.
  - For the top button and Hypershift, set the top front button to **F13** and the top rear button to **F14** in the mouse's memory, for example with [OpenNaga](https://github.com/Zer0codestuff/OpenNaga)'s *Save to Mouse*. Without this, the 12 side buttons still work.

## Install

1. Download `NagaButtons-<version>-macos-arm64.zip` from [Releases](https://github.com/fol2/nagabuttons/releases), unzip it and move `NagaButtons.app` to Applications.
2. The app is not notarised. Clear the download flag once:
   ```bash
   xattr -dr com.apple.quarantine /Applications/NagaButtons.app
   ```
3. Open NagaButtons. Turn it on in **System Settings › Privacy & Security › Input Monitoring** and **› Accessibility** (use **+** if it is not listed). It restarts itself once both are granted.
4. To start it at login, add it in **System Settings › General › Login Items**.

Do not run OpenNaga's remapping at the same time; use OpenNaga only to change the mouse's memory, DPI or polling rate, then quit it.

## Menu bar

The mouse icon shows the status (Active, Paused, Mouse not connected, permissions needed) and the last button pressed. **Enabled** (⌘E) pauses the remapping, so the side buttons type `1` to `=` again. **Quit** (⌘Q) does the same and exits.

## Build from source

Needs Rust and the Xcode Command Line Tools.

```bash
./install.sh     # build, copy to /Applications and open it
./bundle.sh      # only build dist/NagaButtons.app
./uninstall.sh   # remove the app
cargo test      # button map check
```

`APP_DIR` changes the install folder. `SIGN_ID` picks a code-signing identity; the default ad-hoc signature means macOS asks for the permissions again after every rebuild.

## Troubleshooting

For lag or stray presses, turn on the event log and restart the app:

```bash
defaults write io.github.fol2.nagabuttons Trace -bool true
```

`~/Library/Logs/NagaButtons.log` then records each HID report with its delay and the button posted. While World of Warcraft is the front app it also records the clicks and keys the game receives; nothing typed into another app is recorded. `defaults delete io.github.fol2.nagabuttons Trace` turns it off.

## Limitations

- NagaButtons must be running: the mouse's memory cannot store buttons above 5.
- Holding a side button while moving sends normal mouse moves, not drag events.
- Not affiliated with or endorsed by Razer.

## Licence

[MIT](LICENSE). The onboard-profile approach builds on [OpenNaga](https://github.com/Zer0codestuff/OpenNaga) (MIT).
