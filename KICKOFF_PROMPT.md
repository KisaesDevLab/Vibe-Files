# Kickoff prompt for Claude Code — Vibe Files

Paste this as the first message in a fresh Claude Code session opened in the new, empty `vibe-files` repo that contains only these plan files, a `LICENSE` (PolyForm Internal Use 1.0.0), and `.gitignore`.

---

You are building **Vibe Files**, a Rust/egui Windows document management system for CPA firms. The repo currently holds only the plan: `CLAUDE.md`, `PHASES.md`, `STATE.md`, `QUESTIONS.md`, and this file.

Start by reading, in order: `CLAUDE.md` (all of it), `STATE.md`, `QUESTIONS.md`, then **P00** in `PHASES.md`. Do not skim.

Then:

1. Confirm back to me, in five lines or fewer, the three things you must not do (per CLAUDE.md principles) and the current phase.
2. The Filer Desktop source is already decided: the most recent `KisaesDevLab/Vibe-Filer-Desktop` `master` (`ea4e726`, v0.2.4, or newer if pushed since). All document moves, the sentinel, and the folder tree are based on it (CLAUDE.md §Document moves). Practice Manager is out of scope: do not ask for it or read it. No prerequisites are outstanding.
3. Execute P00 step by step. One conventional commit per step. `cargo test --workspace` and `cargo clippy -- -D warnings` must be green at every commit.
4. When a `QUESTIONS.md` item has a default, take the default, record it under "Assumptions taken" in `STATE.md`, and continue. When it has no default, stop and ask.
5. At the end of the session, update `STATE.md`: phase log row, exact next step, assumptions, debt. Then stop.

Rules that override everything else:
- Files in the cabinet stay plain files. The catalog is an index.
- Every mutation goes through `vf-core::tx::Transaction`.
- Never put real client data in fixtures, tests, logs, or commits. The fixture generator makes synthetic PDFs with checksum-invalid EINs/SSNs.
- Do not vendor or add dependencies beyond those named in `CLAUDE.md` §Stack without recording why in `STATE.md` debt.
- Do not start P01 until every P00 "Done when" box is checked.

Begin with step 1.
