# QPeek

Lightweight native Windows tray & desktop app for peeking at AI coding assistant usage limits, burn rate, and local API-equivalent spend.

## Architecture

QPeek does not talk to AI providers itself. It wraps the unmodified [Win-CodexBar](https://github.com/Finesssee/Win-CodexBar) core (MIT) as a sidecar (`codexbar-cli.exe`) and focuses exclusively on a fast, low-memory native Windows UI built with Rust + [Slint](https://slint.dev) (`renderer-software`, 0 WebView2/GPU overhead, ~3.6 MB private RAM in tray, 0.00% idle CPU).

```text
QPeek/
├─ vendor/Win-CodexBar/   git submodule, pinned to a release tag, never edited
├─ src/qpeek/             QPeek UI (Rust + Slint)
│  ├─ build.rs            embeds icons & generates 87-provider metadata from core
│  ├─ ui/app.slint        MainWindow (tray), AppWindow (studio), AboutWindow
│  └─ src/                backend, accounts, activation, theme, i18n, notify, win, icon
├─ build.ps1              builds core + UI into dist/
└─ dist/                  build output (git-ignored): QPeek.exe, codexbar-cli.exe
```

## Features

- **Two-mode single-instance experience (`AppUserModelID: Callidus.QPeek`):**
  - **Tray flyout (`384 × 610`):** Click the live Swiss-Q tray gauge to open a frameless, always-fixed flyout above the clock (`Esc` or outside click hides; right-click refreshes; multi-account switcher `‹ 1/3 ›`).
  - **Frameless ticket-stub desktop studio (`1410 × 810`):** Pinnable to the Windows taskbar, split by a perforated ticket divider with real Win32 transparent semicircular cutouts (`SetWindowRgn` + `DWMWA_COLOR_NONE`). Left studio holds **Providers**, **Cost**, and **General**; right stub (`470 px`) streams live **Limits**.
- **87 dynamic core providers (`Master-Detail`):** Search, `All / Connected` filter, 43 SVG brand icons + monogram badges, multi-account CLI support (without logging out of official CLIs), and 52 API-key provider templates generated at build time.
- **5 live design philosophies:** `Obsidian`, `Paper`, `Brutalist`, `Terminal`, and `Nordic` — switching palette, corner radius, borders, typography, and Windows 11 styling across all windows simultaneously.
- **Burn Rate (`Pace`):** Subtle color-coded pace tags beneath quota bars (`Calm` / `On track` / `Fast` / `Burning`) parsed directly from `usage --json` with zero extra CLI calls.
- **Local Cost (`Cost` / `Maliyet`) tab:** Scans local Codex & Claude Code session logs (`codexbar-cli cost -p both --days <1|7|30> --format json`) to show API-equivalent USD value, tokens, sessions, daily spend bars, and per-model breakdown.
- **Limit Activation (Auto 5h Starter):** Master schedule + Quiet Hours + per-account `Follow master` or custom schedule for accounts with a 5-hour rolling window (`Codex`, `Claude`, `Antigravity`), firing silent ephemeral pings with a `4h 55m` cooldown guard.
- **Limits snapshot for agents & scripts (`%LOCALAPPDATA%\QPeek\limits.json`):** After every refresh (every 5 min), QPeek writes all connected accounts with provider, email, plan, per-window used/remaining percent and reset time (`schema: 1`, atomic temp-file + rename). Tools can read live quotas from this file instead of querying providers themselves.
- **Windows 11 Toast notifications & i18n:** 80% / 100% / reset threshold alerts, `Ctrl+Alt+Q` global shortcut, Windows startup (`--tray`), and live Turkish / English (`TR` / `EN`) switching.

## Building

Requirements (Windows):

- Visual Studio Build Tools 2022 with the **Desktop development with C++** workload
- Rust via [rustup](https://rustup.rs) (stable, `x86_64-pc-windows-msvc`)

```powershell
git clone --recursive https://github.com/Caliredus/QPeek
cd QPeek
.\build.ps1
```

Already cloned without `--recursive`? Run `git submodule update --init` first.

## Updating the core

```powershell
cd vendor/Win-CodexBar
git fetch --tags
git checkout vX.Y.Z
cd ../..
git add vendor/Win-CodexBar
git commit -m "Update core to vX.Y.Z"
```

## License

QPeek is MIT licensed (see `LICENSE`). The bundled Win-CodexBar core keeps its
own MIT license and NOTICE; both are copied to `dist/third-party/` on every build.
The UI uses [Slint](https://slint.dev) under the Slint Royalty-free License 2.0
(attribution via the AboutSlint widget in the About window) and
[tray-icon](https://github.com/tauri-apps/tray-icon) (MIT/Apache-2.0); their license
texts are copied to `dist/third-party/` as well.
QPeek is an independent project and is not affiliated with CodexBar or Win-CodexBar.
