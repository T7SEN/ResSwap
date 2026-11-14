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
