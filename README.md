# Zebra Configure

A desktop app (Tauri + SvelteKit) that sets up Zebra printers over the network, as if a
USB mirror stick had been plugged into each one.

The app has three pages, picked from the bar on the left:

- **Configure** — add printers by IP address or hostname, pick a profile for each, choose whether
  to send the Zebra folder too, and apply. The app only ever talks to printers you enter; it
  never scans the network.
- **Printer** — one printer's alerts, status, network, media and counters, every setting it reports,
  and commands such as calibrate (choosing gap/notch, black mark, continuous or auto-detect), feed,
  print the configuration label, pause and restart. Profiles can be applied from here too, each
  block of details copies to the clipboard, and the printer's own web page opens in the browser.
  Factory reset puts the printer settings (`^JUF`) or the network settings (`^JUN`, then a
  restart) back to Zebra's defaults.
- **Diagnose** — runs chosen checks on one printer and lists what's wrong, with fixes to apply and
  links to Zebra's guides. Read-only checks cover errors and warnings, printer state, the
  printhead, media settings and sensor, profile settings, memory, network, weblink, the clock,
  printhead wear and restarts. Tests that feed media (feed, calibrate, printhead element test,
  sensor profile) are opt-in and only run when media is loaded. A log shows every command sent
  and each reply.
- **Profiles** — add, edit and delete profiles, with a list of every setting the selected one changes.
  Export saves one profile or all of them to a file, for sharing; Import adds profiles from such
  files to the list, to look over before saving. The Zebra folders tab sets the folder for each
  model family, and its firmware version.
- **Settings** — light, dark or system theme, where the data is saved, and the keyboard shortcuts
  (Ctrl+1–5 for the pages, F5 to refresh, Ctrl+F to filter, Ctrl+S to save, Ctrl+Enter to diagnose).

The Printer page only polls its printer while the window is focused. Applying profiles and running
diagnoses happen in the backend, so they carry on whichever page is open and whether or not the
window has focus. Closing the window while printers are still being configured keeps the app in the
notification area until they're done; it then quits, or stays there if any failed.

### Mobile printers (ZQ)

- Settings a printer doesn't have, like the wired network on a mobile printer, are skipped and
  listed rather than failing the profile.
- Darkness is set through `print.tone_zpl`, so it's on the 0–30 scale even on a printer set to CPCL.
- Profiles can use continuous media, for receipts.
- The Printer page shows the battery (charge, health, cycles, capacity), and doesn't auto-refresh a
  battery printer unless asked, since polling keeps it from turning itself off when idle.
- Diagnose checks the battery's health and charge, and whether the printer is in a ZPL mode; many
  mobile printers ship in CPCL line print, where the ZPL commands this app sends don't run.

## Installing

Download the latest installer from the repository's
[Releases](https://github.com/jctaylorjr/zebra_config/releases) page:

- `Zebra.Configure_…_x64-setup.exe` installs for the current user, without admin rights.
- `Zebra.Configure_…_x64_en-US.msi` installs for every user of the PC, for IT to deploy.

They aren't code-signed yet, so Windows SmartScreen may warn about them: choose More info, then
Run anyway. Updating keeps profiles, settings and recent printers.

To publish a new version, raise `version` in `src-tauri/tauri.conf.json` and push a matching tag,
e.g. `git tag v0.2.0 && git push origin v0.2.0`. GitHub Actions builds the installers and attaches
them to a release for that tag.

### Building it yourself

Where the installers can't be downloaded, build the app on the PC from its source instead. Download
the source (Code, then Download ZIP on GitHub), extract it, and double-click `build.cmd`, or run:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\build-windows.ps1
```

It installs what the build needs, if it isn't there already: Visual Studio Build Tools with the C++
tools (for every user, so Windows asks for admin), and Rust and Bun (into your user profile). Then
it builds `dist\Zebra Configure.exe`, which runs on its own; add `-Installer` to build the setup
program too. Run it as yourself rather than as administrator.

The first build takes a while: the Build Tools take 10–20 minutes to install, and the app another
5–10 minutes to build. The build's files (about 1.5 GB) go in `%LOCALAPPDATA%\zebra-configure-build`,
which every copy of the source shares, so building a newer download only rebuilds what changed;
delete the folder to free the space. Extract the source to a folder on the PC rather than a network
drive. Run from anywhere else, the script downloads the source from GitHub first; `-Branch` picks
the branch.

## What applying does

1. **Zebra folder** — the printer's model picks a folder laid out like a USB mirror stick:
   - `appl/` firmware, sent raw on port 9100, then the app waits for the printer to restart.
     Skipped when the printer already reports the folder's firmware version, or with none set,
     the firmware file's name, as a USB mirror does.
   - `files/` stored on `E:`.
   - `commands/` sent in filename order. If one resets the printer (`device.reset` or `~JR`),
     the app waits for it to come back.
2. **Profile** — hostname, darkness, network, media (type, sensor, print width) and any extra
   commands. The hostname goes to `ip.hostname` on Link-OS 7.6 and later, and to
   `device.friendly_name` on every printer, since that is the hostname on older firmware. These
   are sent just before the first command file that resets the printer, so that restart puts the
   hostname and network settings into effect while the app is watching; with no such file they
   go last, and take effect at the printer's next restart.
3. **Checks** — every `setvar` is read back, and every file the folder puts on `E:` (including
   uploads inside command files, such as `WEBLINK1_CA.NRD`) must be there.
4. **Calibrate and test print** — for media profiles, only if there is media and the head is
   closed. If the printer is stuck in an error, it is reset and left paused with nothing queued.

Unreachable printers are retried with backoff, at least every 20 seconds. A restart, a new name or
a switch between wired and wireless can move a printer, so while it isn't answering the app also
looks for it, by its serial, at the IPs it has answered at (including on the Printer and Diagnose
pages) and under the names it may have now: the profile's hostname, `{model}-{serial}`, and its
serial. When it answers somewhere else, its row moves there. The app still never scans the network.

## Settings

Stored in the app data folder (`%APPDATA%\com.jt1280.zebra-configure` on Windows,
`~/.local/share/com.jt1280.zebra-configure` on Linux):

- `settings.json` — Zebra folders per model family, their firmware versions, and the theme.
- `recent.json` — the last 20 printers opened on the Printer and Diagnose pages.
- `profiles.json` — profiles. Edited in the app; it won't save over a file it couldn't read. Example:

```json
{
  "id": "zd-2x1",
  "name": "2x1 labels",
  "models": ["ZD"],
  "hostname": "CSC{serial}",
  "darkness": 25.0,
  "network": { "wired_dhcp": true, "wireless_dhcp": true, "primary": "wired" },
  "media": { "width_in": 2, "media_type": "gap", "sensor": "transmissive" },
  "commands": []
}
```

Every part after `models` is optional; leave one out and the app doesn't touch those settings.
`models` are matched as substrings of the printer's model. Leave out `width_in` to use the
printer's full width in whole inches (406 dots on a 2 inch 203 dpi printer).

## Development

```sh
bun install
bun run tauri dev
```

Tests: `cargo test` in `src-tauri`, and `bun run test` for the frontend. One test talks to a real printer and only runs when asked:

```sh
ZEBRA_TEST_IP=192.168.1.50 ZEBRA_TEST_FOLDER=/path/to/Zebra \
ZEBRA_TEST_PROFILE='{"id":"t","name":"2x1 labels","models":[],"media":{"width_in":2,"media_type":"gap","sensor":"transmissive"}}' \
cargo test -- --ignored --nocapture
```

Both `ZEBRA_TEST_FOLDER` and `ZEBRA_TEST_PROFILE` are optional. A media profile calibrates and
prints a test label when media is loaded.
