# Zebra Configure

A desktop app (Tauri + SvelteKit) that sets up Zebra printers over the network, as if a
USB mirror stick had been plugged into each one.

Enter a printer's IP address or hostname, pick a profile, and apply. The app only ever
talks to printers you enter; it never scans the network.

## What applying does

1. **Zebra folder** — the printer's model picks a folder laid out like a USB mirror stick:
   - `appl/` firmware, sent raw on port 9100, then the app waits for the printer to restart.
     Skipped when the printer already reports the folder's firmware version.
   - `files/` stored on `E:`.
   - `commands/` sent in filename order. If one resets the printer (`device.reset` or `~JR`),
     the app waits for it to come back.
2. **Profile** — media settings (type, sensor, print width) and any extra commands.
3. **Checks** — every `setvar` is read back, and every file the folder puts on `E:` (including
   uploads inside command files, such as `WEBLINK1_CA.NRD`) must be there.
4. **Calibrate and test print** — for media profiles, only if there is media and the head is
   closed. If the printer is stuck in an error, it is reset and left paused with nothing queued.

Unreachable printers are retried with backoff.

## Settings

Stored in the app data folder (`~/.local/share/com.jt1280.zebra_configure` on Linux):

- `settings.json` — Zebra folders per model family and their firmware versions. Edited in the app.
- `profiles.json` — profiles, edited by hand. Example:

```json
{
  "id": "zd-2x1",
  "name": "2x1 labels",
  "models": ["ZD"],
  "media": { "width_in": 2, "media_type": "gap", "sensor": "transmissive" },
  "commands": []
}
```

`models` are matched as substrings of the printer's model. Leave out `width_in` to use the
printer's full width in whole inches (406 dots on a 2 inch 203 dpi printer).

## Development

```sh
bun install
bun run tauri dev
```

Tests: `cargo test` in `src-tauri`. One test talks to a real printer and only runs when asked:

```sh
ZEBRA_TEST_IP=192.168.1.50 ZEBRA_TEST_FOLDER=/path/to/Zebra \
ZEBRA_TEST_PROFILE='{"id":"t","name":"2x1 labels","models":[],"media":{"width_in":2,"media_type":"gap","sensor":"transmissive"}}' \
cargo test -- --ignored --nocapture
```

Both `ZEBRA_TEST_FOLDER` and `ZEBRA_TEST_PROFILE` are optional. A media profile calibrates and
prints a test label when media is loaded.
