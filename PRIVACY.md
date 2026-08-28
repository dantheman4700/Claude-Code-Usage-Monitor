# Headroom privacy policy

Last updated: 2026-08-28

Headroom is a Windows desktop utility that shows how much of your usage
allowance is left on the AI coding services you have signed in to. This
policy describes everything it touches.

## What Headroom reads

To know how much of your allowance is used, Headroom reads the sign-in
credentials that each provider's own tools already keep on your computer:

- Claude Code: `%USERPROFILE%\.claude\.credentials.json` and the Claude
  desktop app's token cache
- Codex: `%USERPROFILE%\.codex\auth.json` (or `CODEX_HOME`)
- Antigravity: the Windows credential `gemini:antigravity`
- Grok: `%USERPROFILE%\.grok\auth.json`
- Cursor: the Cursor desktop app's local state database, or
  `%USERPROFILE%\.config\cursor\auth.json` for the `cursor-agent` CLI
- OpenCode Go: helper-app configuration files or environment variables you
  set
- Fireworks AI and Devin: API keys in environment variables or in
  `%USERPROFILE%\.claude\.env.fireworks` / `.env.devin`

If you use the Windows Subsystem for Linux, Headroom also reads the same
files inside your WSL distributions.

Headroom reads these files only. It never modifies them, except that for
some providers it may run that provider's own command-line tool (for example
`agy models`) so the tool refreshes its own expired token.

## What Headroom sends, and to whom

Headroom sends each credential only to the provider that issued it, to ask
that provider's usage API how much of your allowance is used:

- api.anthropic.com (Claude)
- chatgpt.com (Codex)
- cloudcode-pa.googleapis.com (Antigravity)
- cli-chat-proxy.grok.com (Grok)
- cursor.com (Cursor)
- opencode.ai (OpenCode Go)
- api.fireworks.ai (Fireworks AI)
- api.devin.ai (Devin)

Nothing is sent anywhere else. Headroom has no server, no analytics, no
telemetry, no crash reporting service, and no advertising. The author never
receives your credentials, your usage figures, or any information about you.

## What Headroom stores

Headroom keeps the usage readings it receives, a rolling history of them, an
activity log of provider state changes, and your settings, in
`%APPDATA%\Headroom` on your computer. Credentials are not stored there. You
can delete that folder at any time; Headroom recreates what it needs.

An optional diagnostic log (only when started with `--diagnose`) and a crash
log are written to your temporary folder.

## Your controls

- Turn any provider off in Settings and Headroom stops reading and sending
  its credentials.
- Sign out of a provider's tool and Headroom can no longer read it.
- Uninstall Headroom and delete `%APPDATA%\Headroom` to remove everything.

## Contact

Questions about this policy: open an issue at
https://github.com/dantheman4700/headroom.
