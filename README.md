# Activity Journal Desktop

A tray icon, a window and a settings form for the Activity Journal engine (a separate Python
project, not yet public). It runs the engine **unchanged**:
it reads the files the engine already writes and starts `uv run journal run` the way you would by
hand. Windows only, like the engine.

Native Rust GUI: `egui` driven directly on `winit` + `wgpu` (no `eframe`, no webview, no node).

## What it does

| Area | Behaviour |
| --- | --- |
| Tray | Coloured disc: green idle, blue running, amber last run partial, red needs attention. Menu: Open, Run checkpoint, Run final journal, Quit. Closing the window keeps the tray. |
| Status | Every day in `<data_dir>/state.json` with its lifecycle stage and stuck cause; the pid in the engine's run lock when another run holds it. |
| Run | Checkpoint, final run and dry run. Output streams into the Run tab. The exit code is reported with its real meaning: `0` ok, `10` partial, `20` stuck, `30` privacy blocked, `40` day already closed, `75` lock held. |
| Journal | The published `journal/YYYY/MM/*.md` files, read-only, final or checkpoint. |
| Settings | Paths, timezone, model, router URL, router and privacy toggles, and enable/disable for every plugin found under `plugins/*/plugin.json`. Validated before saving. Only changed values are written, into `config/settings.local.toml` and `config/pipeline.local.toml`, keeping your comments. Secrets stay in the environment. |
| Autostart | Optional per-user logon entry that starts the app in the tray. The engine's scheduled runs stay in Task Scheduler, so they still happen when the app is closed. |

What it deliberately does not do: change the engine, its output, its lifecycle or its privacy
gate; parse numbers out of the Markdown; edit a journal.

## Run it

1. Have a working Activity Journal checkout (`uv sync` done, `uv run journal run --dry-run` works).
2. Build and start:

   ```
   cargo run --release -- --engine D:\path\to\activity-journal
   ```

   Or set `ACTIVITY_JOURNAL_ROOT`, or pick the folder once in **Settings**; it is saved to
   `%APPDATA%\activity-journal-desktop\desktop.toml`.
3. `--hidden` starts in the tray only (what autostart uses).

Building needs the MSVC toolchain: Visual Studio Build Tools with the **Desktop development with
C++** workload (MSVC + Windows SDK).

## Develop

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

CI runs the same on `windows-latest`.

## License

MIT
