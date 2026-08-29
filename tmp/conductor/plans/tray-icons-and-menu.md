# Tray icons, the tray menu, and the last greens (2026-08-29)

Owner ask: the right-click menu must line up with Settings; the leftover green
theming goes; the tray icon becomes a proper feature — a good set of options,
several icons at once.

## Waves
A. Settings model: `TrayIconSettings` gains window (incl. monthly), used/left,
   column style, ring mark (digits/initials/none), alert colour; `extra_tray_icons`
   list (≤7). Schema 4 (older builds leave a v4 file alone).
B. Painter: content per icon, `shown_used_percent` for alert tint, column and
   row layouts, 3×5 letters for provider marks, tinted render, previews.
C. Tray: N notification icons (uID 1..), per-icon tooltip, thresholds in state,
   menu mirrors Settings (tray icon, appearance, frequency, providers, startup,
   updates, Settings…); tray-side saves persist tray icons + appearance.
D. Panel: reloads the settings file when the tray changes it (no clobber);
   Tray icons section = primary editor + extra icons list + previews.
E. Greens: `success()` retired; ok/normal states are neutral; dropdown selected
   text uses the palette (was white-on-light in light mode).
F. Gate, deploy, preview sheet, cross-model review, commit, push.
