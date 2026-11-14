# Test log

One entry per gate run, written after the run. A failed step is written as failed with its first error line; a step that did not run says so and why.

## 2026-11-12 P0 gate, run 1

Before the first commit, so no commit hash. Windows build 26200, WebView2 157.0.4341.46, Node 26.12.1, Rust 1.99.1.

| Step | Command                                                    | Result                                                                                                                                                                                                                                        |
| ---- | ---------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1    | pnpm typecheck                                             | pass                                                                                                                                                                                                                                          |
| 1    | pnpm lint                                                  | pass                                                                                                                                                                                                                                          |
| 1    | pnpm format:check                                          | pass                                                                                                                                                                                                                                          |
| 1    | pnpm test                                                  | pass, 6 files, 40 tests                                                                                                                                                                                                                       |
| 2    | cargo fmt --all --check                                    | pass                                                                                                                                                                                                                                          |
| 2    | cargo clippy --all-targets --all-features, warnings denied | pass                                                                                                                                                                                                                                          |
| 2    | cargo test --all-features                                  | pass, 81 tests (lib 67, 1 ignored; ipc 10; pipeline 3; shell 1)                                                                                                                                                                               |
| 3    | pnpm tauri build                                           | pass, installer ResSwap_0.1.0_x64-setup.exe 2,307,936 bytes; release exe 10,036,736 bytes                                                                                                                                                     |
| 3    | dist mock marker check                                     | pass, no RESSWAP_MOCK_CORE in dist                                                                                                                                                                                                            |
| 3    | mt.exe manifest check on the release exe                   | pass, Common-Controls 6.0.0.0 and asInvoker                                                                                                                                                                                                   |
| 4    | pnpm e2e:mock                                              | pass, 9 tests                                                                                                                                                                                                                                 |
| 5    | pnpm e2e:app (phase P0)                                    | pass, 5 tests                                                                                                                                                                                                                                 |
| 6    | pnpm budgets P0                                            | failed: first-paint-debug 24,454 ms, over its budget of under 1,000 ms (runs 24,454, 18,102, 1,091, 466 and 510 ms, measured right after a fresh debug build); installer pass; idle-open missing (a release-exe reading the maintainer takes) |

A repeat of the first-paint test one minute later, outside the gate, measured 413, 387, 420, 419 and 478 ms. Both result files stay in docs/results/2026-11-12/.

Screenshots and measurements: docs/results/2026-11-12/.

## 2026-11-12 P0 gate, run 2 (rerun after the review fixes)

Before the first commit, so no commit hash. Windows build 26200, WebView2 157.0.4341.46, Node 26.12.1, Rust 1.99.1. The fixes touched files every step builds or tests, so all six steps ran again.

| Step | Command                                                    | Result                                                                                                                                                                                                                                                           |
| ---- | ---------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1    | pnpm typecheck                                             | pass                                                                                                                                                                                                                                                             |
| 1    | pnpm lint                                                  | pass                                                                                                                                                                                                                                                             |
| 1    | pnpm format:check                                          | pass                                                                                                                                                                                                                                                             |
| 1    | pnpm test                                                  | pass, 6 files, 45 tests                                                                                                                                                                                                                                          |
| 2    | cargo fmt --all --check                                    | pass                                                                                                                                                                                                                                                             |
| 2    | cargo clippy --all-targets --all-features, warnings denied | pass                                                                                                                                                                                                                                                             |
| 2    | cargo test --all-features                                  | pass, 83 tests (lib 68, 1 ignored; ipc 12; pipeline 2; shell 1)                                                                                                                                                                                                  |
| 3    | pnpm tauri build                                           | pass, installer ResSwap_0.1.0_x64-setup.exe 2,310,169 bytes; release exe 10,040,832 bytes                                                                                                                                                                        |
| 3    | dist mock marker check                                     | pass, no RESSWAP_MOCK_CORE in dist                                                                                                                                                                                                                               |
| 3    | mt.exe manifest check on the release exe                   | pass, Common-Controls 6.0.0.0 and asInvoker                                                                                                                                                                                                                      |
| 4    | pnpm e2e:mock                                              | pass, 9 tests                                                                                                                                                                                                                                                    |
| 5    | pnpm e2e:app (phase P0)                                    | pass, 5 tests (4 checks, then first paint)                                                                                                                                                                                                                       |
| 6    | pnpm budgets P0                                            | exit 2, not run or waiting: installer pass (2,310,169 bytes, at most 10,485,760); first-paint-debug pass (473 ms, under 1,000; runs 473, 415, 390, 409 and 463 ms; measure-P0-firstpaint-3.json); idle-open missing (a release-exe reading the maintainer takes) |

Scans: the security, network, rule A6, pin and hygiene scans printed only their expected lines; the traceability check for P0 printed nothing.

Review: one review, 13 findings (2 major, 11 minor), all fixed before this run; the first-paint method changed as D-017 records.

Hardware: none this phase.

## 2026-11-14 P1 gate, run 1

On commit 463032d, with the P1 work uncommitted. Windows build 26200, WebView2 157.0.4341.46, Node 26.12.1, Rust 1.99.1.

| Step | Command                                                    | Result                                                                                                                                       |
| ---- | ---------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| 1    | pnpm typecheck                                             | pass                                                                                                                                         |
| 1    | pnpm lint                                                  | pass                                                                                                                                         |
| 1    | pnpm format:check                                          | pass                                                                                                                                         |
| 1    | pnpm test                                                  | pass, 8 files, 82 tests                                                                                                                      |
| 2    | cargo fmt --all --check                                    | pass                                                                                                                                         |
| 2    | cargo clippy --all-targets --all-features, warnings denied | pass                                                                                                                                         |
| 2    | cargo test --all-features                                  | pass, 250 tests (lib 192, 4 ignored; ipc 16; pipeline 41; shell 1)                                                                           |
| 3    | pnpm tauri build                                           | pass, installer ResSwap_0.1.0_x64-setup.exe 2,601,134 bytes; release exe 11,078,144 bytes                                                    |
| 3    | dist mock marker check                                     | pass, no RESSWAP_MOCK_CORE in dist                                                                                                           |
| 3    | mt.exe manifest check on the release exe                   | pass, Common-Controls 6.0.0.0 and asInvoker                                                                                                  |
| 4    | pnpm e2e:mock                                              | pass, 69 tests                                                                                                                               |
| 5    | pnpm e2e:app (phase P1)                                    | pass, 9 tests                                                                                                                                |
| 6    | pnpm budgets P1                                            | exit 2, waiting: installer pass; first-paint-debug 414 ms, pass; mode-switch missing (a measurement the maintainer takes in the HW-P1 round) |

Screenshots and measurements: docs/results/2026-11-14/.

## 2026-11-14 P1 gate, run 2 (rerun after the review fixes)

On commit 463032d, with the P1 work uncommitted. Windows build 26200, WebView2 157.0.4341.46, Node 26.12.1, Rust 1.99.1. The fixes touched files every step builds or tests, so all six steps ran again.

| Step | Command                                                    | Result                                                                                                                                       |
| ---- | ---------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| 1    | pnpm typecheck                                             | pass                                                                                                                                         |
| 1    | pnpm lint                                                  | pass                                                                                                                                         |
| 1    | pnpm format:check                                          | pass                                                                                                                                         |
| 1    | pnpm test                                                  | pass, 8 files, 82 tests                                                                                                                      |
| 2    | cargo fmt --all --check                                    | pass                                                                                                                                         |
| 2    | cargo clippy --all-targets --all-features, warnings denied | pass                                                                                                                                         |
| 2    | cargo test --all-features                                  | pass, 260 tests (lib 193, 4 ignored; ipc 17; pipeline 49; shell 1)                                                                           |
| 3    | pnpm tauri build                                           | pass, installer ResSwap_0.1.0_x64-setup.exe 2,609,246 bytes; release exe 11,103,232 bytes                                                    |
| 3    | dist mock marker check                                     | pass, no RESSWAP_MOCK_CORE in dist                                                                                                           |
| 3    | mt.exe manifest check on the release exe                   | pass, Common-Controls 6.0.0.0 and asInvoker                                                                                                  |
| 3    | resswap-diag left out of the bundle                        | pass, installer.nsi does not name it and target\release holds no resswap-diag.exe                                                            |
| 4    | pnpm e2e:mock                                              | pass, 69 tests                                                                                                                               |
| 5    | pnpm e2e:app (phase P1)                                    | pass, 9 tests                                                                                                                                |
| 6    | pnpm budgets P1                                            | exit 2, waiting: installer pass; first-paint-debug 691 ms, pass; mode-switch missing (a measurement the maintainer takes in the HW-P1 round) |

Screenshots and measurements: docs/results/2026-11-14/.

## 2026-11-14 P1 fix for monitors without a device path, gate run 1

On commit 2693a2b, with the fix uncommitted: a monitor whose device is disabled in Device Manager reports no device path, and every command error is now written to the log. Windows build 26200, WebView2 157.0.4341.46, Node 26.12.1, Rust 1.99.1.

| Step | Command                                                    | Result                                                                                                                                       |
| ---- | ---------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| 1    | pnpm typecheck                                             | pass                                                                                                                                         |
| 1    | pnpm lint                                                  | pass                                                                                                                                         |
| 1    | pnpm format:check                                          | pass                                                                                                                                         |
| 1    | pnpm test                                                  | pass, 9 files, 85 tests                                                                                                                      |
| 2    | cargo fmt --all --check                                    | pass                                                                                                                                         |
| 2    | cargo clippy --all-targets --all-features, warnings denied | pass                                                                                                                                         |
| 2    | cargo test --all-features                                  | pass, 264 tests (lib 196, 4 ignored; ipc 17; pipeline 50; shell 1)                                                                           |
| 3    | pnpm tauri build                                           | pass, installer ResSwap_0.1.0_x64-setup.exe 2,615,152 bytes; release exe 11,133,952 bytes                                                    |
| 3    | dist mock marker check                                     | pass, no RESSWAP_MOCK_CORE in dist                                                                                                           |
| 3    | mt.exe manifest check on the release exe                   | pass, Common-Controls 6.0.0.0 and asInvoker                                                                                                  |
| 3    | resswap-diag left out of the bundle                        | pass, installer.nsi does not name it and target\release holds no resswap-diag.exe                                                            |
| 4    | pnpm e2e:mock                                              | pass, 69 tests                                                                                                                               |
| 5    | pnpm e2e:app (phase P1)                                    | pass, 9 tests                                                                                                                                |
| 6    | pnpm budgets P1                                            | exit 2, waiting: installer pass; first-paint-debug 525 ms, pass; mode-switch missing (a measurement the maintainer takes in the HW-P1 round) |

Screenshots and measurements: docs/results/2026-11-14/.

## 2026-11-14 P1 fix for monitors without a device path, gate run 2 (rerun after the review fixes)

On commit 2693a2b, with the fix and the review fixes uncommitted. Windows build 26200, WebView2 157.0.4341.46, Node 26.12.1, Rust 1.99.1. The review fixes touched files every step builds or tests, so all six steps ran again. Step 1 first failed one test, whose list of scenario IDs lacked the new monitors-without-path scenario; the list was updated and step 1 then passed.

| Step | Command                                                    | Result                                                                                                                                       |
| ---- | ---------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| 1    | pnpm typecheck                                             | pass                                                                                                                                         |
| 1    | pnpm lint                                                  | pass                                                                                                                                         |
| 1    | pnpm format:check                                          | pass                                                                                                                                         |
| 1    | pnpm test                                                  | pass, 9 files, 87 tests                                                                                                                      |
| 2    | cargo fmt --all --check                                    | pass                                                                                                                                         |
| 2    | cargo clippy --all-targets --all-features, warnings denied | pass                                                                                                                                         |
| 2    | cargo test --all-features                                  | pass, 265 tests (lib 197, 4 ignored; ipc 17; pipeline 50; shell 1)                                                                           |
| 3    | pnpm tauri build                                           | pass, installer ResSwap_0.1.0_x64-setup.exe 2,613,967 bytes; release exe 11,125,760 bytes                                                    |
| 3    | dist mock marker check                                     | pass, no RESSWAP_MOCK_CORE in dist                                                                                                           |
| 3    | mt.exe manifest check on the release exe                   | pass, Common-Controls 6.0.0.0 and asInvoker                                                                                                  |
| 3    | resswap-diag left out of the bundle                        | pass, installer.nsi does not name it and target\release holds no resswap-diag.exe                                                            |
| 4    | pnpm e2e:mock                                              | pass, 69 tests                                                                                                                               |
| 5    | pnpm e2e:app (phase P1)                                    | pass, 9 tests                                                                                                                                |
| 6    | pnpm budgets P1                                            | exit 2, waiting: installer pass; first-paint-debug 438 ms, pass; mode-switch missing (a measurement the maintainer takes in the HW-P1 round) |

Screenshots and measurements: docs/results/2026-11-14/.

## 2026-11-14 P1 fix batch 2 (recovery reports, put-back scaling, gone monitors), gate run 3

On commit 2693a2b, with both fix batches uncommitted. Windows build 26200, WebView2 157.0.4341.46, Node 26.12.1, Rust 1.99.1. All six steps ran, because the batch touches files every step builds or tests.

| Step | Command                                                    | Result                                                                                                                                       |
| ---- | ---------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| 1    | pnpm typecheck                                             | pass                                                                                                                                         |
| 1    | pnpm lint                                                  | pass                                                                                                                                         |
| 1    | pnpm format:check                                          | pass                                                                                                                                         |
| 1    | pnpm test                                                  | pass, 9 files, 91 tests                                                                                                                      |
| 2    | cargo fmt --all --check                                    | pass                                                                                                                                         |
| 2    | cargo clippy --all-targets --all-features, warnings denied | pass                                                                                                                                         |
| 2    | cargo test --all-features                                  | pass, 272 tests (lib 199, 4 ignored; ipc 17; pipeline 55; shell 1)                                                                           |
| 3    | pnpm tauri build                                           | pass, installer ResSwap_0.1.0_x64-setup.exe 2,615,317 bytes; release exe 11,128,320 bytes                                                    |
| 3    | dist mock marker check                                     | pass, no RESSWAP_MOCK_CORE in dist                                                                                                           |
| 3    | mt.exe manifest check on the release exe                   | pass, Common-Controls 6.0.0.0 and asInvoker                                                                                                  |
| 3    | resswap-diag left out of the bundle                        | pass, installer.nsi does not name it and target\release holds no resswap-diag.exe                                                            |
| 4    | pnpm e2e:mock                                              | pass, 69 tests                                                                                                                               |
| 5    | pnpm e2e:app (phase P1)                                    | pass, 9 tests                                                                                                                                |
| 6    | pnpm budgets P1                                            | exit 2, waiting: installer pass; first-paint-debug 551 ms, pass; mode-switch missing (a measurement the maintainer takes in the HW-P1 round) |

Screenshots and measurements: docs/results/2026-11-14/.

## 2026-11-14 P1 fix batch 2, gate run 4 (rerun after the review fixes)

On commit 2693a2b, with both fix batches and the review fixes uncommitted. Windows build 26200, WebView2 157.0.4341.46, Node 26.12.1, Rust 1.99.1. The review fixes touched files every step builds or tests, so all six steps ran again.

| Step | Command                                                    | Result                                                                                                                                       |
| ---- | ---------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| 1    | pnpm typecheck                                             | pass                                                                                                                                         |
| 1    | pnpm lint                                                  | pass                                                                                                                                         |
| 1    | pnpm format:check                                          | pass                                                                                                                                         |
| 1    | pnpm test                                                  | pass, 9 files, 92 tests                                                                                                                      |
| 2    | cargo fmt --all --check                                    | pass                                                                                                                                         |
| 2    | cargo clippy --all-targets --all-features, warnings denied | pass                                                                                                                                         |
| 2    | cargo test --all-features                                  | pass, 279 tests (lib 200, 4 ignored; ipc 17; pipeline 61; shell 1)                                                                           |
| 3    | pnpm tauri build                                           | pass, installer ResSwap_0.1.0_x64-setup.exe 2,615,707 bytes; release exe 11,134,464 bytes                                                    |
| 3    | dist mock marker check                                     | pass, no RESSWAP_MOCK_CORE in dist                                                                                                           |
| 3    | mt.exe manifest check on the release exe                   | pass, Common-Controls 6.0.0.0 and asInvoker                                                                                                  |
| 3    | resswap-diag left out of the bundle                        | pass, installer.nsi does not name it and target\release holds no resswap-diag.exe                                                            |
| 4    | pnpm e2e:mock                                              | pass, 69 tests                                                                                                                               |
| 5    | pnpm e2e:app (phase P1)                                    | pass, 9 tests                                                                                                                                |
| 6    | pnpm budgets P1                                            | exit 2, waiting: installer pass; first-paint-debug 675 ms, pass; mode-switch missing (a measurement the maintainer takes in the HW-P1 round) |

Screenshots and measurements: docs/results/2026-11-14/.
