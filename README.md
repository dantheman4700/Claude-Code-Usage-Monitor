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
