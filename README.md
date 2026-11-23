# Activity Journal Desktop

A tray icon, a window and a settings form for the Activity Journal engine (a separate Python
project, not yet public). It changes nothing about how the engine collects, guards or publishes:
it reads the files the engine writes and starts `uv run journal …` the way you would by hand
(or, when **Settings** turns the Rust runner on, the engine's own `journal-rs` for a final run or
a checkpoint), reading the engine's `--json` output (`run`, `timeline`, `history`; schema
version 1, and a version it does not know is refused). It needs an engine with those commands. Windows only, like
the engine.

Native Rust GUI: `egui` driven directly on `winit` + `wgpu` (no `eframe`, no webview, no node).

## What it does

| Area | Behaviour |
| --- | --- |
| Tray | Coloured disc: green idle, blue running, amber last run partial, red needs attention. Menu: Open, Run checkpoint, Run final journal, Quit. Closing the window keeps the tray. |
| Status | Every day in `<data_dir>/state.json` with its lifecycle stage and stuck cause; the pid in the engine's run lock when another run holds it. |
| Run | Checkpoint, final run and dry run. The engine's human output streams into the Run tab; its JSON document is parsed for the cause and the degraded sources. The exit code is reported with its real meaning: `0` ok, `10` partial, `20` stuck, `30` privacy blocked, `40` day already closed, `75` lock held. |
| Notifications | A Windows notification when a run started from the app ends, naming its exit code and meaning, plus the cause when the run was blocked and the degraded sources when it was partial (from `run --json`; `state.json`'s stuck cause for an older engine); and one when the screenshot loop stops for lack of disk space. Runs started by Task Scheduler do not notify. |
| Dashboard | History from `journal history --json` for the last 7, 14 or 30 days: each day's active time and events, and days without a stats file shown as missing, never as zeros. Pick a day, or **Load today** (`journal timeline --json`, only when you click: today is not stored before the nightly run, so the engine collects it under its run lock). For the day: totals and every source's status with degraded ones named, an hourly chart stacked by project (active minutes or events), project and source breakdowns that add up to the totals, top apps and domains, and cost per provider. **Open journal** shows the narrative read-only. **This week** adds up the Monday-to-Sunday week that contains today from the same history: active time per day, then the same project, source, app and cost panels over the sum; days without stats are named as missing. **Search journals** finds the lines of the published journals that contain every word you type (any case), newest day first, at most 5 lines per day and 200 in all; a final journal is searched instead of its checkpoint, and files that cannot be read are counted, not hidden. Click a day to open it. |
| Journal | The published `journal/YYYY/MM/*.md` files, read-only, final or checkpoint. |
| Settings | Paths, timezone, model, router URL, router and privacy toggles, and enable/disable for every plugin found under `plugins/*/plugin.json`. Validated before saving. Only changed values are written, into `config/settings.local.toml` and `config/pipeline.local.toml`, keeping your comments. Secrets stay in the environment. |
| Screenshots | Optional, off by default. The engine's screenshot daemon, ported: every `[screenshot] interval_seconds` it stops below `minimum_free_megabytes`, skips without grabbing when the foreground process or title is excluded, and otherwise writes a PNG of all screens to `<data_dir>/screenshots/<day>/<HHMMSS>.png`, where the engine's purge removes it. Replaces the `ActivityJournal-Daemon` scheduled task; disable that task when you turn this on. |
| Rust runner | Optional, off by default. The final run and the checkpoint start the engine's `rust/core/target/release/journal-rs.exe run [--checkpoint|--dry-run] --json` instead of `uv run journal run`: each plugin in its own process, the same `--json` document and exit codes. The dry run goes through it too, and the final run and dry run pass `--catch-up` so they cover the same pending days as `journal run` (which catches up by default). An unbuilt binary falls back to the Python engine with a line in the Run log. Build it with `cargo build --release` in the engine's `rust/core`. |
| Sync to GitHub | Optional, off by default. **Settings → Sync to GitHub** publishes a checkpoint every N minutes (default 10, 1 to 1440) while the app is open, on whichever runner is selected. It runs quietly in the background, takes the engine's run lock, and notifies only when it needs attention. |
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
