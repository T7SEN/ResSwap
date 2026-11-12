# Decisions

Each entry records a choice that departs from the original plan for ResSwap, or settles a point that plan left open, with its reason and the requirements it touches. Numbers are never reused. When a later entry changes an earlier one, both entries name each other.

## D-001 Only reg.exe and pnputil.exe run elevated

- Date: 11 Nov 2026.
- Decision: ResSwap's own code never runs elevated. Only two Windows tools may run elevated: `%SystemRoot%\System32\reg.exe`, for the NV_Modes write and its undo, and `%SystemRoot%\System32\pnputil.exe /restart-device`, for the graphics driver restart. ResSwap starts each one by full path through ShellExecuteExW with the runas verb, only after an in-app explanation, with arguments built from validated values only. It reads the tool's exit code, then reads the system again to confirm the result. The install stays per-user, with no administrator prompt at install or update.
- Instead of: a helper mode of ResSwap.exe itself, started elevated.
- Reason: ResSwap.exe sits in a folder the user can write to. Microsoft's guidance on DLL planting says a library planted in the search path of an administrator process may give local privilege elevation, so an elevated copy of ResSwap.exe would widen what a program running as that user can do. The single-instance plugin would also make an elevated second copy forward its arguments to the running copy and exit.
- Requirements: REQ-SAFE-10, REQ-SAFE-11, REQ-SAFE-12, REQ-SAFE-13, REQ-SAFE-14, REQ-SEC-01, REQ-SEC-03.

## D-002 TypeScript 6.0.3, not 7.0.2

- Date: 11 Nov 2026.
- Decision: TypeScript is pinned at 6.0.3, the last 6.x release.
- Reason: typescript-eslint 8.72.2 declares the peer range `typescript >=4.8.4 <6.1.0`, and TypeScript 7.0.2 offers its compiler API only under `./unstable/*`.
- Requirements: none; this is a toolchain choice.

## D-003 Four Tauri plugins, each used from Rust

- Date: 11 Nov 2026.
- Decision: Kept: tauri-plugin-global-shortcut, tauri-plugin-single-instance, tauri-plugin-updater and tauri-plugin-log, each used from Rust only. tauri-plugin-updater joins with the release workflow. Dropped: tauri-plugin-autostart, tauri-plugin-store, tauri-plugin-process, tauri-plugin-opener and tauri-plugin-shell. No `@tauri-apps/plugin-*` npm package is installed, the web view gets core permissions only, and it calls only ResSwap's own commands.
- Reason: tauri-plugin-autostart tries HKLM first and writes the exe path without quotes, so ResSwap writes its own quoted value under HKCU. ResSwap's own settings file replaces tauri-plugin-store (D-004). Rust restarts the app with `app.restart()`, and the log folder opens through ShellExecuteExW from Rust, so the process, opener and shell plugins add nothing. An npm plugin package would give the web view plugin permissions.
- Requirements: REQ-SEC-07, REQ-SHELL-06, REQ-SHELL-07, REQ-SET-01, REQ-SET-09.

## D-004 Settings in ResSwap's own versioned JSON file

- Date: 11 Nov 2026.
- Decision: Rust alone reads and writes `settings.json`, a JSON file that carries a version number. Loading runs each migration in order. Saving writes a temporary file in the same folder, calls `sync_all`, keeps a `.bak` copy of the current file and renames the temporary file over it. The folder named at the time was `%LOCALAPPDATA%\ResSwap`.
- Changed by: D-011, which moves the folder.
- Reason: tauri-plugin-store 2.5.1 writes straight onto its target file, has no migrations and keeps its file in the Roaming folder.
- Requirements: REQ-SET-01, REQ-SET-03, REQ-SET-04, REQ-SET-05, REQ-SET-06.

## D-005 One lowercase name outside the repository

- Date: 11 Nov 2026.
- Decision: The name is ResSwap everywhere. The one exception is a maintainer tool kept outside this repository: its name and its folder are `resswap`.
- Changed by: D-010, which adds a short list of identifiers to the exception.
- Reason: that tool's format allows only lowercase letters, digits and hyphens in a name.
- Requirements: none.

## D-006 "Usually without a Windows restart"

- Date: 11 Nov 2026.
- Decision: Text about adding a stretched resolution on an NVIDIA card says it works "usually without a Windows restart". No text links NVIDIA Control Panel to the NVAPI custom display functions. No text says that the registry method works on the test PC until a hardware check confirms it.
- Reason: every documented way to restart the graphics driver needs administrator rights, and Windows can still ask for a restart afterwards. No source checked says that NVIDIA Control Panel calls the NVAPI custom display functions. The evidence on the test PC is not conclusive, because its NVIDIA settings hold a separate custom display entry for the same size.
- Requirements: REQ-NV-20, REQ-NV-21, REQ-DOC-07.

## D-007 Defaults

- Date: 11 Nov 2026.
- Decision: Auto-switch is paused at every start, in debug and release builds, until the player turns it on. No global hotkey is registered until the player sets one. The keep-this-mode countdown is 10 s. The restore delay after a game exits is 10 s. The watcher takes one process snapshot every 2 s, and only while auto-switch runs and at least one profile has it on.
- Reason: a start never changes the display without the player's action, no hotkey is taken from another app by default, and no process snapshot runs unless a profile needs it.
- Requirements: REQ-WATCH-01, REQ-WATCH-02, REQ-SHELL-05, REQ-SAFE-03, REQ-SET-07.

## D-008 Windows 10 22H2 is best effort

- Date: 11 Nov 2026.
- Decision: Windows 11 x64 is the target. On Windows 10 22H2 x64, ResSwap installs, but it is not tested for each release.
- Reason: the hardware checks run on a Windows 11 PC.
- Requirements: REQ-DOC-05, REQ-REL-12.

## D-009 The debug-build first paint gates until phase P7

- Date: 12 Nov 2026.
- Decision: Until phase P7 (hardening), the first-paint budget of REQ-PERF-05, under 1 s, is checked against the number the end-to-end app tests measure on the debug build they start with the fake backend. From P7, the number measured on the release build on the test PC is checked instead.
- Reason: the automated build phases start no release build, and the WebView2 debugging port that the measurement reads is opened only for the debug build the test runner starts (REQ-SEC-06).
- Departs from: the budget rule that only release-build numbers are checked. The debug build is not the build players get, so its number stands in until P7.
- Requirements: REQ-PERF-05, REQ-PERF-07, REQ-SEC-06.

## D-010 Lowercase identifiers

- Date: 12 Nov 2026.
- Decision: Besides the tool in D-005, the lowercase form is used only for the Cargo package name `resswap`, the library name `resswap_lib`, the diagnostics binary `resswap-diag`, and the name field of one local maintainer file that is excluded from version control. Prose, UI text, docs, commit messages and file names keep ResSwap.
- Extends: D-005.
- Reason: Cargo and rustc warn on capital letters in package and crate names, and the maintainer file's format allows only lowercase letters, digits and hyphens in a name.
- Requirements: none.

## D-011 The data folder is named after the bundle identifier

- Date: 12 Nov 2026.
- Decision: `settings.json`, `journal.json` and the `backups` folder live in `%LOCALAPPDATA%\io.github.T7SEN.ResSwap`, the folder named after the bundle identifier, beside the `logs` folder Tauri writes there. The per-user install folder, `%LOCALAPPDATA%\ResSwap`, holds program files only.
- Reason: when the player ticks the uninstaller's option to delete app data, the uninstaller removes the folders named after the bundle identifier, so ResSwap's data and logs go with it. With the option left unticked, they stay, whatever the uninstaller does with its install folder.
- Departs from: the folder D-004 named, `%LOCALAPPDATA%\ResSwap`, and the file layout of the setup decisions.
- Requirements: REQ-SET-01, REQ-SET-02, REQ-SET-09.

## D-012 Release builds unwind on a panic

- Date: 12 Nov 2026.
- Decision: The release profile in `src-tauri/Cargo.toml` does not set `panic = "abort"`, which the project template set.
- Reason: with unwinding, the command wrapper turns a panic inside a command into the INTERNAL error code in release builds as well as in debug builds and tests, so the window shows an error with its own message instead of the app ending at once. Start-up recovery (REQ-SAFE-06) stays the way back after a crash that ends the process.
- Departs from: the project template's release profile.
- Requirements: REQ-SAFE-06, REQ-UI-08.

## D-013 shadcn and tw-animate-css as exact devDependencies

- Date: 12 Nov 2026.
- Decision: shadcn 4.23.0 and tw-animate-css 1.4.0 stay in `package.json` as exact devDependencies.
- Reason: `src/styles/globals.css` imports `shadcn/tailwind.css`, which the build reads from the installed shadcn package, so running shadcn only through `pnpm dlx` is not enough. shadcn init added tw-animate-css, `globals.css` imports it, and the components shadcn adds use its animation classes.
- Departs from: the pin list's plan to run shadcn only through `pnpm dlx` for init and add.
- Requirements: none.

## D-014 Node 26.13.0 on the maintainer's PC and in CI

- Date: 12 Nov 2026.
- Decision: Node 26.13.0, the first long-term support release of Node 26, is the version for the maintainer's PC and for CI. `package.json` keeps the engines range `>=26.12.1 <27`, so the checks also run on 26.12.1 until the PC is updated.
- Reason: the Node.js 26.13.0 release notes say Node 26 receives long-term support updates until the end of April 2029, and that 26.13.0 changes only release metadata from 26.12.1.
- Departs from: the setup record, which named the installed 26.12.1 and left the CI version open.
- Requirements: REQ-REL-01.

## D-015 Format and lint commands name what they read

- Date: 12 Nov 2026.
- Decision: `pnpm format:check` and `pnpm format` run Prettier on an explicit list of the project's own paths (src, e2e, docs, scripts, .github, src-tauri, README.md and the configuration files at the root through one glob) instead of `prettier --check .`. A new top-level folder joins both scripts in the change that creates it. `pnpm lint` runs ESLint with `--flag unstable_native_nodejs_ts_config`, so Node's own type stripping loads `eslint.config.ts`.
- Reason: Prettier 3.9.10 skips only what `.gitignore` and `.prettierignore` list, and it reads the patterns of any further ignore file relative to that file's own folder, so files kept out of version control on the maintainer's PC alone would still be checked; the explicit list covered every Prettier-parsable file git could add on 12 Nov 2026. Without the flag, ESLint 10.14.0 loads a TypeScript configuration through jiti, which is not a dependency of this project and was found only through another package; Node 26 strips types natively.
- Departs from: the planned `prettier --check .` and the jiti row of the pin list.
- Requirements: none; this is a toolchain choice.

## D-016 What P0 builds do at start and for later commands

- Date: 12 Nov 2026.
- Decision: Until the tray arrives in P5, every start of ResSwap opens the main window, a start with `--autostart` or `--play` included, and the process ends when the last window closes. Until each command's own phase, the commands answer as follows. `get_state` and `list_monitors` answer from the display backend; `list_profiles` and `get_settings` answer from memory (no profiles, the default settings); `confirm_mode` and `revert_mode` answer that nothing is waiting for a decision; `dismiss_error` removes a raised error. Every other command answers INTERNAL with the detail that it is not available in this build and that nothing was changed, and calls no backend method. Until P1 builds the Win32 display backend, a release build raises one INTERNAL error at start, shown in the window, because its display backend reads nothing.
- Reason: without a tray, a process with no window could only be ended from Task Manager. INTERNAL is the code for a failure no other code describes, and no step may be reported as done when it did not happen.
- Departs from: the start rules and the exit rule of the finished app, for P0 to P4 only.
- Requirements: REQ-SHELL-09, REQ-SHELL-14, REQ-UI-09.

## D-017 How the debug first paint is measured and judged

- Date: 12 Nov 2026.
- Decision: The first-paint test runs after the other tests of the real debug window, so the exe has started before it measures, and it discards two warm-up starts before its five measured starts. `pnpm budgets` judges a gate row on the newest result file of the current phase; every earlier file stays in docs/results and every gate run stays in docs/test-log.md.
- Reason: in the first P0 gate run, the first starts right after a fresh debug build took 24,454 and 18,102 ms, while a repeat one minute later on the same exe took 387 to 478 ms on all five starts. The slow starts come from the exe being new to Windows, not from ResSwap. With every file of the phase counted, one such run would fail every later gate of that phase.
- Departs from: the budget rule that a gate row counts every result of the phase, and the single warm-up start.
- Requirements: REQ-PERF-05, REQ-PERF-07.
