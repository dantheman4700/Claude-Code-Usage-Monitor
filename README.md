# Headroom

How much room is left on every AI coding provider you use — Claude Code,
Codex, Antigravity, Grok, Cursor, OpenCode, Fireworks and Devin — in one tray
icon and one panel.

Headroom sits in the Windows system tray. Click the icon for the dashboard:
every limit each provider reports, which one bites first, how fast each window
is filling, and where there is still room to send the next job. Right-click
for the menu.

It reads the credentials the provider CLIs already keep on this PC (and inside
WSL), asks each provider's own API how much of your plan is used, and keeps
the readings locally. Nothing leaves the machine except those requests to the
providers themselves. See PRIVACY.md.

## Credit

Headroom began as a fork of [Claude Code Usage Monitor](https://github.com/CodeZeno/Claude-Code-Usage-Monitor)
by Craig Constable (MIT), and its provider pollers descend from that work.
Headroom is not affiliated with Anthropic, OpenAI, Google, xAI, Cursor,
Fireworks AI or Cognition.

## License

MIT — see LICENSE.

## Install

**Microsoft Store** — coming; the first submission is being prepared.

**Portable** — download `headroom.exe` from the latest release, put it
anywhere, run it. It lives in the tray; there is nothing to install. Settings
and history go to `%APPDATA%\Headroom`.

**MSIX (sideload)** — every release also ships `Headroom_<version>_x64.msix`.
It is unsigned (the Store signs its own copy), so to sideload it you sign it
yourself: `packaging\msix\build-msix.ps1 -DevSign` makes a development
certificate, signs the package and prints the two commands that trust the
certificate and install it.

If you used Claude Code Usage Monitor before, Headroom carries your settings
and history over on first launch and takes over the "Start with Windows"
entry.

