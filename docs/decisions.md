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

## D-018 Start-up recovery waits while a profile's game runs

- Date: 12 Nov 2026.
- Decision: When ResSwap starts and its journal holds a change it never put back, and the exe of any saved profile is in the process list, recovery waits until no such exe is running, then restores. It never changes the desktop resolution under a running game. The wait is built in P4, when the process list reader exists; until then recovery restores at once, as REQ-SAFE-06 says.
- Reason: a start of ResSwap during a game would otherwise change that game's resolution. Pillar 3 says ResSwap never acts against a running game.
- Departs from: REQ-SAFE-06, which restores at once, and D-007 and REQ-WATCH-02, which take snapshots only while auto-switch runs: while recovery waits, one snapshot every 2 s runs even with auto-switch paused. P4 updates REQ-WATCH-02 and the safety rules with the wait.
- Requirements: REQ-SAFE-06, REQ-WATCH-02.

## D-019 Release builds clear WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS at start

- Date: 14 Nov 2026 (the answer of 12 Nov 2026 to the P0 report, built at P1).
- Decision: main() calls system::webview_env::clear_browser_arguments() right after the DLL search limit and before it reads its arguments, starts a thread or builds the web view. In a release build it removes WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS from the process environment with one std::env::remove_var call; a debug build keeps it, because the e2e:app runner opens port 9223 on its own debug build that way. remove_var is unsafe in edition 2024; the Rust 1.99.1 documentation calls it sound in a single-threaded program and always sound on Windows, and the one call sits in src-tauri/src/system/webview_env.rs with its SAFETY comment.
- Reason: WebView2 adds that variable's text to the browser arguments of every web view in the process, so any program that can set the player's environment could open a remote debugging port on ResSwap's window and call every command that window may call.
- Checked: system::webview_env::tests::req_sec_06_release_build_clears_webview_arguments passes under cargo test (variable kept) and cargo test --release (variable removed), 14 Nov 2026. Not checked: a release ResSwap.exe started with the variable set keeps the port closed; that needs a release-exe start, which this build leaves to the hardware checklist.
- Departs from: nothing in the brief; it narrows the earlier view that this property belongs to WebView2 alone.
- Requirements: REQ-SEC-06.

## D-020 A debug app run on the fake backend gets a clock that follows the real time

- Date: 14 Nov 2026 (the answer of 12 Nov 2026 to the P0 report, built at P1).
- Decision: FakeClock has two modes. FakeClock::new() stands still until a test advances it, and every test keeps it. FakeClock::following_real_time() starts at monotonic time 0 and epoch time FAKE_EPOCH_MS (11 Nov 2026 00:00 UTC) and then moves with std::time::Instant, so a change to the Windows clock cannot bend it; advance() still adds time, and its readings are not logged, so a long run does not grow the call log. select_backends gives the real-time mode to a debug app run with RESSWAP_BACKEND=fake; Fakes::new keeps the still clock.
- Reason: e2e:app and the Narrator checks drive the real window over the fake backend. With a still clock the keep-mode countdown would never reach 0 there, so the Timeout revert could not be seen. The epoch stays fixed so screenshots do not depend on the day of the run.
- Departs from: the first wording of the fake clock rule (it advanced only when a test advanced it).
- Requirements: REQ-SAFE-03, REQ-SAFE-05, REQ-DISP-01.

## D-021 Hidden monitors keep their mode list but get no stretched presets

- Date: 14 Nov 2026.
- Decision: a monitor hidden as a virtual display (REQ-DISP-07) is still returned by list_monitors with its mode list, so the player can pick a listed mode with the show-hidden toggle on, but it gets an empty list of stretched presets.
- Reason: a virtual display has no panel to stretch, and the preset buttons would suggest a stretch that means nothing there. Keeping the mode list keeps the toggle useful for a real monitor that is hidden by mistake.
- Departs from: nothing in the brief; it settles how the "no stretched modes for virtual displays" rule and the show-hidden toggle work together.
- Requirements: REQ-DISP-07, REQ-DISP-08, REQ-DISP-09.

## D-022 Interlaced modes are left out of the mode list

- Date: 14 Nov 2026.
- Decision: a GDI mode entry with DM_INTERLACED in dmDisplayFlags is left out of every mode list and is never applied.
- Reason: interlaced modes are not stretched-mode candidates, and leaving them out means a progressive refresh rational from the CCD target is never attached to an interlaced entry of the same size and whole-hertz rate. No Windows document requires either choice.
- Departs from: REQ-DISP-06 as written, which lists every 32 bpp mode Windows offers.
- Requirements: REQ-DISP-06, REQ-DISP-17.

## D-023 Damaged or newer settings and journal files are moved aside

- Date: 14 Nov 2026.
- Decision: a settings.json that fails to parse is renamed settings.damaged-<epoch ms>.json, and one written by a newer ResSwap is renamed settings.newer-<epoch ms>.json; memory then gets settings.json.bak or the defaults, which are also written back to settings.json without touching the .bak, and start-up reports SETTINGS_CORRUPT. The journal does the same with journal.damaged-<ms>.json and journal.newer-<ms>.json and reports RECOVERY_INCOMPLETE. If a rename fails, the file stays and every save of that file is refused (SETTINGS_WRITE_FAILED or JOURNAL_WRITE_FAILED) until the next start, so it is never overwritten and no change runs without a record. Each of the three files is read with a cap of 1 MiB.
- Reason: the damaged or newer file stays for the player and the log, the next start reads a good file, and a later save cannot copy a damaged file over the last good copy.
- Departs from: REQ-SET-05, which says the fallback replaces the file in memory only.
- Requirements: REQ-SET-03, REQ-SET-05, REQ-SET-06, REQ-SAFE-01.

## D-024 One monitor goes back on its own route; the saved snapshot only when every monitor does

- Date: 14 Nov 2026.
- Decision: putting one monitor back (Revert, the end of the countdown, closing the keep-mode window, a superseded or rejected change, Restore native, Quit) uses the same apply call and route as the change, on that monitor only. The saved CCD snapshot, taken in the same journal write as the first open change and removed with the last, is applied only when every journaled monitor returns to its baseline: Restore everything, and start-up recovery when every journaled monitor needs it, with SDC_USE_DATABASE_CURRENT as the fallback. A snapshot restore keeps every available monitor connected since the snapshot, with its live modes.
- Reason: applying the whole snapshot to put back one monitor would also reset every other monitor, and would turn off a monitor connected after the first change. The same route keeps a CDS_FULLSCREEN safety net in place.
- Departs from: the first wording of the restore rules, which put every return to the baseline through the snapshot.
- Requirements: REQ-SAFE-02, REQ-DISP-13, REQ-DISP-21, REQ-DISP-22.

## D-025 Restore everything reports a disconnected monitor as still changed

- Date: 14 Nov 2026.
- Decision: when Restore everything finds a journaled monitor that is not connected, it keeps that monitor's entries, restores the rest, and answers RECOVERY_INCOMPLETE with the monitor in stillChanged; MONITOR_GONE goes to the log. When another mode replaces a pending change during the countdown, the countdown stops, nothing is put back and no mode-reverted is sent; the entry closes when the screen shows the state before the change, and otherwise stays open with status applied, as an active change Restore can still undo.
- Reason: the window can then list everything that is still changed in one place, and ResSwap never fights a mode a game or Windows set (REQ-WATCH-07).
- Departs from: the first error list of restore_everything, which named MONITOR_GONE, and the journal status moves, which had no move for the kept-open case.
- Requirements: REQ-DISP-22, REQ-DISP-23, REQ-SAFE-09, REQ-WATCH-07.

## D-026 Change IDs are built without a new crate

- Date: 14 Nov 2026.
- Decision: a change ID is a lowercase UUID in the version 4 layout, built from the standard library's per-process random hash keys mixed with a serial number and the time.
- Reason: no UUID or random crate is pinned, a new dependency needs a pin refresh, and the IDs only tell changes apart in the journal and the window; they guard nothing.
- Departs from: nothing in the brief.
- Requirements: REQ-SAFE-01.

## D-027 Log files rotate at about 1 MB and five old files are kept

- Date: 14 Nov 2026.
- Decision: the log plugin writes ResSwap.log up to 1,000,000 bytes, then starts a new file and keeps the five newest old ones (tauri-plugin-log 2.10.2 RotationStrategy::KeepSome(5)), so the log folder holds about 6 MB at most.
- Reason: the plugin's defaults, a 40,000 byte file and one kept file that is deleted on rotation, would lose the timing and error lines of a hardware round, because a debug build logs at the Debug level. The logs stay on the PC (REQ-PRIV-04).
- Departs from: nothing in the brief; it sets the size and count the logging rules left open.
- Requirements: REQ-SET-09, REQ-PRIV-04, REQ-PERF-03.

## D-028 The revert countdown setting is between 1 and 60 seconds until P2

- Date: 14 Nov 2026.
- Decision: save_settings refuses a revert countdown of 0 or of more than 60 seconds (INTERNAL with a plain detail); the default stays 10 seconds (D-007). The P2 open item on the allowed range replaces this bound.
- Reason: with no upper bound a value from the window could keep an untested mode on screen for years, which breaks the safety of the countdown. 60 seconds is the widest range offered for the P2 choice.
- Departs from: nothing in the brief; REQ-SET-08 leaves the range to P2.
- Requirements: REQ-SET-08, REQ-SEC-08, REQ-SAFE-03.

## D-029 A revert that cannot write its record leaves the change pending

- Date: 14 Nov 2026.
- Decision: the countdown stops only once a revert has written its reverting status, the write that gives the Ticket. If that write fails (JOURNAL_WRITE_FAILED), nothing is put back: the change stays pending with its countdown, the keep-mode window stays, and changes for other monitors keep waiting. The countdown reverts at its end and after that tries again once a second, raising only its first failed try. A Revert from the window returns the error; closing the keep-mode window raises it and leaves the countdown to revert; a superseding apply or Restore native fails with it; a resume raises it. Restore everything restores the change anyway once its own restoring status is on disk, taking it over as an active change. Quit cannot wait, so it reports RECOVERY_INCOMPLETE and leaves the entry, still pending, for start-up recovery.
- Reason: stopping the countdown before that write left an unconfirmed mode, perhaps a black screen, with nothing left to revert it.
- Departs from: the first wording of the countdown rules, which stopped the countdown at the start of every revert.
- Requirements: REQ-SAFE-01, REQ-SAFE-03, REQ-SAFE-05, REQ-SAFE-06.

## D-030 Putting a previous state back verifies the scaling too

- Date: 14 Nov 2026.
- Decision: a put-back counts as verified only when the mode and the scaling both read back as recorded. When the mode reads back but the scaling differs, the entry stays open, so the baseline is never closed with the scaling changed. The player sees no mode-reverted; the keep-mode window closes, the countdown stops, Home lists an active change with the previous mode and the scaling read back, and the error is RECOVERY_INCOMPLETE with stillChanged. Restore everything then puts the scaling back from the saved snapshot. Writing the exact recorded scaling during a put-back waits for the route and scaling control the HW-P1 round picks.
- Reason: RECOVERY_INCOMPLETE is the code for a put-back that did not verify while the journal still lists the change, and it leads the player to Restore everything. Reporting the revert as done would claim a step that did not finish (REQ-SAFE-17).
- Departs from: the first put-back rule, which checked the size and rate only.
- Requirements: REQ-SAFE-17, REQ-NV-25, REQ-DISP-22.

## D-031 A failed read after a successful apply is INTERNAL

- Date: 14 Nov 2026.
- Decision: when the apply call succeeds but the read after it fails, the read error is logged and the previous state is still put back. The error is INTERNAL, naming the failed read and how the put-back ended, or RECOVERY_INCOMPLETE naming the failed read when the put-back fails too. It is never MODE_REJECTED.
- Reason: a failed read says nothing about whether Windows accepted the mode, so calling it a rejected mode would disguise one error as another.
- Departs from: nothing in the brief; it narrows when MODE_REJECTED is used.
- Requirements: REQ-UI-09, REQ-SAFE-17, REQ-DISP-13.
