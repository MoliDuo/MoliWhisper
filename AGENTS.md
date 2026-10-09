# AGENTS.md

<!-- moli-rules:start -->
## Moli rules (copied verbatim from MoliSpec; do not edit)

These rules apply to every Moli repository. The full standards live in the private repo `MoliDuo/MoliSpec` (`standards/`).

**Naming**
- Product name `MoliFoo` (repo, package, file and identifier names, no spaces). User-facing name is `Moli Foo` (one space): window titles, app name, UI text, README title, Release titles.

**Deploy and CI**
- Deploy only after CI passes. Merging to `main` deploys to production (server apps), so run the check entry (`npm run check` or the stack's equivalent) locally before opening the PR, and watch CI and the deploy after it merges.
- Keep the CI names fixed: workflows `ci` / `deploy` / `release` / `codeql`; jobs `check`, `gitleaks`, `build`, `integration`, `ci-gate`.
- Never delete or skip tests, or loosen lint rules, to make a check pass.

**Git**
- Conventional Commits (`feat(scope): subject`).
- Never push to `main` directly. Every change, however small, goes on a new branch and is merged through a PR with auto-merge on. Before starting, update local `main` (`git switch main && git pull`) and branch from it; if a push is rejected or the branch is behind, pull the latest `main` and merge or rebase it in.
- Never force-push `main`. Roll back with `git revert`.

**Secrets and private information**
- Never commit secrets, `.env` files, keys, or internal information (server addresses, hostnames, Tailscale addresses, personal emails). Use obviously fake values in tests and examples (`test-token`, `example.com`, `192.0.2.1`).
- Never print secret values in logs, chat, or commits. Never store secrets in the OS keychain. Runtime secrets live in the server `.env` (mode 600); build and release secrets live in GitHub organization secrets.
- Do not copy a shared (organization-level) secret into repository-level secrets unless the administrator has said so.

**Login, data, config**
- Sign-in is Authelia only. Do not build your own accounts, passwords or registration pages.
- Database and settings schemas only add; never delete or rename an existing field in one step. Migrations must keep the previous app version working.
- Clients are offline-first and the server is authoritative. Settings are read in the order defined in the config standard; do not invent a second source.
- Server apps expose `GET /healthz` returning `{"ok": true, "version": "<commit sha>"}`, run as non-root, take config from environment variables, and publish no host ports.

**Working with the user**
- Do only what was asked. Do not publish, delete, or change shared settings (GitHub org, server, DNS) without being asked.
- Reply to the user in Chinese, briefly.
<!-- moli-rules:end -->

## About this project

Moli Whisper is a macOS (Apple Silicon, 13+) menu bar dictation app: press the hotkey (right ⌥ by
default), speak, and the text is pasted where the cursor is. Speech recognition is Qwen real-time ASR
on DashScope; the optional "organize" step rewrites speech into written text with DeepSeek. Both use
API keys the user enters in Settings. Forked from `lilong7676/doubao-murmur` (MIT). Design and the
reasons behind it: `docs/architecture.md`.

## Run and test

- Node 24 (`.nvmrc`) and pnpm; Rust is pinned in `rust-toolchain.toml`. Builds need macOS (the app
  links AppKit and CoreAudio).
- Install: `pnpm install --frozen-lockfile`.
- Check (same as CI): `pnpm run check` — Prettier, `tsc` (UI and release scripts), the release-script
  tests (`node --test`), the Vite build, `cargo fmt --check`, `cargo clippy -D warnings`,
  `cargo test --workspace`. `pnpm run format` fixes the formatting.
- Run: `./scripts/dev-app.sh` builds a signed debug `.app` and opens it (use it for hotkeys, pasting
  and the microphone); `./scripts/dev.sh` is `tauri dev` for UI work only. `./scripts/install.sh`
  builds a release `.app` into `/Applications`. Logs: `./scripts/logs.sh`.
- Local builds sign with the "Moli Self-Signed Code Signing" certificate when it is in the keychain, so
  the Accessibility and microphone grants survive rebuilds; without it they are ad-hoc signed.
- Release (MoliSpec 006, `release.mode: manual`): `scripts/set-version.sh X.Y.Z` (it only edits
  `version` in `package.json`, the one place the version is written), merge `chore(release): vX.Y.Z`
  through a PR, then tag that commit of `main` `vX.Y.Z` and push the tag. `.github/workflows/release.yml`
  builds, signs, drafts, uploads, checks `main` has not moved, publishes and verifies the live
  `latest.json`. "Run workflow" on `release` is a dry run with a throwaway update key.

## Layout

- `crates/moli-core`: platform-independent Rust core (Qwen ASR client, DeepSeek organizer, session
  state machine, audio DSP, hotkey matching). No Tauri; most tests live here.
- `src-tauri`: the Tauri app (tray, windows, updater, macOS hotkey hook, paste, permissions).
- `ui`: the overlay and settings pages (Vite, plain TypeScript).
- `scripts`: local build/install helpers; `scripts/release`: the release pipeline's feed, signature
  check and notes (Node, tested).

## Do not touch

- `plugins.updater.pubkey` in `src-tauri/tauri.conf.json`: replacing it cuts every installed copy off
  from updates (MoliSpec 007 §7.5.8).
- The bundle identifier `com.moliduo.moliwhisper` and `productName` `MoliWhisper`: changing either
  resets every user's Accessibility and microphone grants or moves their settings.
- The signing certificate: release builds must be signed with the shared "Moli Self-Signed Code
  Signing" certificate (fingerprint in `.github/workflows/release.yml`), never ad-hoc.
