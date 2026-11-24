# QPeek

Windows tray app for peeking at AI coding assistant usage limits.

> **Status:** early skeleton. Only the core build works so far; the QPeek UI is not written yet.

## How it is put together

QPeek does not talk to AI providers itself. It wraps the actively maintained
[Win-CodexBar](https://github.com/Finesssee/Win-CodexBar) core (MIT), which
already handles 80+ providers, and focuses only on the Windows UI.

```
QPeek/
├─ vendor/Win-CodexBar/   git submodule, pinned to a release tag, never edited
├─ src/                   QPeek UI (coming)
├─ build.ps1              builds the core into dist/
└─ dist/                  build output (git-ignored)
```

The core runs as a sidecar process (`codexbar-cli.exe usage --json`) and QPeek
renders the result.

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
QPeek is an independent project and is not affiliated with CodexBar or Win-CodexBar.
