# proton-pass-search

Search your Proton Pass logins from KDE's KRunner, or from the GNOME Shell overview, and copy a
password, username or one-time code without opening the Proton Pass app.

The KRunner version works. The GNOME version is a work in progress: it passes the same tests,
but nobody has run it on a real GNOME session yet.

## Requirements

- **`pass-cli`:** Proton's command-line client, [logged in](https://protonpass.github.io/pass-cli/)
  with a normal session (`pass-cli login`). Access-token and agent sessions cannot list
  usernames and URLs.
- **Rust:** to build.
- **KDE:** Plasma 6.
- **GNOME:** `wl-clipboard` (`wl-copy`), because GNOME's compositor lacks the clipboard
  protocol this tool uses on KDE.

## Install

```sh
./install.sh            # KRunner
./install.sh --gnome    # KRunner files plus the GNOME search provider
./install.sh --uninstall
```

The script installs `~/.local/bin/proton-pass-search`, a D-Bus service file and the KRunner
plugin file, all without root. GNOME reads search providers only from system directories, so
for GNOME the script prints one `sudo install` command for you to run.

## Use

### KRunner

- **Search:** press Alt+Space and type `pp ` followed by part of a name, username or URL.
- **Search only Proton Pass:** bind a shortcut to
  `qdbus6 org.kde.krunner /App displaySingleRunner proton-pass-search`. In that mode the `pp`
  prefix is optional.
- **Copy:** Enter copies the password. The buttons on a result copy the username (👤) or the
  current one-time code (⏱); the ⏱ button appears only on items that have one.

### GNOME

GNOME results have no buttons, so a word after `pp` picks what to copy:

| Type | Copies |
|---|---|
| `pp github` | password |
| `pp u github` | username, or the email if the item has no username |
| `pp t github` | one-time code |

### What you see after copying

A notification names the item and what was copied, never the secret. Passwords and one-time
codes are cleared from the clipboard after 30 seconds, unless you have copied something else
since. On KDE, Klipper then puts back its latest history entry.

## How it works

One process serves both desktops over D-Bus: the KRunner interface at `/krunner` and the GNOME
search-provider interface at `/gnome`, under the name `local.ProtonPassSearch`. The desktop
starts it on the first search.

- **Loading:** at start-up, the service lists every vault with `pass-cli` in parallel. That took
  2–4 s for 473 logins in 14 vaults. Searches after that take milliseconds. The list is
  reloaded in the background on the first search after it is 5 minutes old.
- **Search:** ranks the way Proton Pass's own search does. Every word must occur in the title,
  username, email or a URL. A title match always beats any other match, and an exact match
  beats a prefix, which beats the start of a word, which beats any other substring. Case and
  accents are ignored.
- **Copying:** fetches the secret with `pass-cli` only when you pick a result.

### Security properties

- **No secrets in memory between copies:** the list keeps titles, usernames, emails, URLs and
  whether an item has a one-time code. Passwords in `pass-cli`'s output are skipped while
  parsing, and output buffers are wiped after use.
- **Nothing on disk:** the service writes no files.
- **No shell:** `pass-cli` runs with an argument list.
- **Kept out of clipboard history:** secrets are offered with `x-kde-passwordManagerHint:
  secret`, which Klipper honours.
- **One side effect on Proton Business accounts:** listing with `--show-secrets` marks every
  listed item as read in the organisation's audit data. See `TODO.md`.

### Options

```text
--clear-after <SECONDS>    clear copied secrets after this long [default: 30]
--refresh-after <SECONDS>  reload the list on the first search after this long [default: 300]
--clipboard <BACKEND>      auto | data-control | wl-copy [default: auto]
```

To change them, edit the `Exec=` line in
`~/.local/share/dbus-1/services/local.ProtonPassSearch.service`.

## Tests

```sh
cargo test
```

The integration tests start everything they need and stop it afterwards, so they never touch
your desktop:

- **D-Bus:** a private `dbus-daemon` for each test.
- **Clipboard:** a headless `kwin_wayland --virtual` for tests that copy.
- **`pass-cli`:** a fake one (`tests/fixtures/bin/pass-cli`) serving `tests/fixtures/data`.
- **Notifications:** a fake notification server that records what the service sends.

They need `dbus-daemon`, `kwin_wayland` and `wl-clipboard`. Clipboard tests run once for each
clipboard backend.
