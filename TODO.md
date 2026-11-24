# TODO

## Type the password instead of copying it

Wayland lets no ordinary app send keystrokes to another app's window. The routes, from cleanest
to most general:

- **RemoteDesktop portal (`xdg-desktop-portal`):** the app asks for keyboard access once and you
  approve it in a dialog. KDE and GNOME both implement it. Probably the best fit.
- **KWin's fake-input protocol:** what KDE Connect uses for remote typing. KDE only.
- **`ydotool`:** a virtual keyboard through `/dev/uinput`. Works everywhere, but needs a
  privileged daemon or a udev rule.

Deferred because copying covers the need, and typing adds a permission flow on each desktop.

## Try the GNOME version on a real GNOME session

Everything up to the clipboard is tested over D-Bus, but not inside GNOME Shell. Open questions:

- **Clipboard:** Mutter has no data-control protocol (checked in its source, main branch,
  2026-11-20), so the `auto` backend falls back to `wl-copy`. Does `wl-copy` work when a
  background D-Bus service starts it?
- **Clearing:** does `wl-copy --clear` clear it, and does GNOME keep a copy of the text after
  `wl-copy` exits?
- **Provider file:** is `/usr/local/share` in `XDG_DATA_DIRS` there? GNOME Shell reads providers
  from system data directories only (`collectFromDatadirs('search-providers', false)` in
  `js/ui/remoteSearch.js`).
- **First search:** GNOME sends every search to every provider, so the first search after login
  starts the service and lists all vaults, even without `pp`.

## Proton Business accounts: listing marks every item as read

`pass-cli item list --show-secrets` calls `mark_items_read` on every listed item. On Business
plans that reaches the organisation's audit data (`pass-cli` source:
`src/commands/item/list.rs:319`, `pass/src/item/read.rs:59-60`). Without `--show-secrets`, the
listing has titles but no usernames or URLs. A titles-only mode would avoid the side effect at
the cost of searching titles only.

Deferred until it matters: it depends on the account type.

## Explain a locked `pass-cli` session

A session with a lock (`pass-cli session create-lock`) makes every command fail until
`pass-cli session unlock`, which needs a terminal. Today the raw error shows as a result row or
notification. A clear message with the unlock command would be friendlier. The exact error text
is still unknown.

## Smaller ideas

- **Other item types:** notes, aliases and cards are left out. Copying an alias address or a
  note would fit the same actions.
- **Tie-breaker:** `last_use_time` from the listing could rank recently used items first among
  equal scores.
- **Telemetry:** `pass-cli` sends anonymised usage telemetry unless
  `PROTON_PASS_DISABLE_TELEMETRY` is set. A flag could set it for the calls this service makes.
- **First KRunner query:** a freshly started KRunner showed no results for its very first
  normal-mode query in one manual test. Single-runner mode and later queries worked. Not
  reproduced or explained yet.
- **Licence:** none chosen yet.
