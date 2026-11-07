# AGENTS.md — AMR Architecture & Maintenance Guide

## Overview
AMR is an ultra-lightweight, high-performance cross-platform AI protocol proxy and emulator written in pure Rust (Tokio, Axum, reqwest) with a native Slint GUI (using `backend-winit` and `renderer-femtovg`).

## Core Architecture & Module Layout
```
src/
├── main.rs          # Application entry point, CLI handling, Slint UI event binding & timers
├── lib.rs           # Library exports for unit and integration testing
├── cli.rs           # Clap CLI parser (--headless, --port, --config, --start-minimized)
├── config.rs        # App configuration, quota calculus, account state, model catalogs
├── i18n.rs          # TranslationBundle & multi-language loader (EN, RU, ZH)
├── autostart.rs     # XDG Desktop (Linux) & Registry (Windows) auto-start manager
├── sys_metrics.rs   # Low-overhead OS process RSS / memory metrics collector
├── tray/            # System tray integration (muda / tray-icon) with dynamic localization
└── proxy/
    ├── server.rs    # Axum HTTP proxy router (/v1/chat/completions, /v1/messages, /v1/models)
    ├── balancer.rs  # Quota-aware load balancing, session stickiness & KV cache retention
    ├── router.rs    # Suffix table & native reasoning effort parser, model name translation
    ├── antigravity.rs # Upstream OAuth token refresh, Cloud Code Pa API mapping, device profiles
    ├── fingerprint.rs # Cryptographic machine ID, device ID & hardware fingerprint generators
    ├── stats.rs     # Lock-free atomic metrics counters & persistent disk logging (logs.jsonl)
    ├── stream.rs    # SSE chunk transformer, heartbeat injector (: ping\n\n), reasoning parser
    └── mappers/     # OpenAI <-> Claude protocol request & response schema transformers
```

## Core Philosophy & Design Principles
1. **Zero Bloat & Zero Telemetry:**
   - No analytics, tracking, ads, or phone-home requests are allowed under any circumstances.
   - Do NOT introduce WebViews, Chromium, Electron, or Node.js dependencies.
2. **License Compliance:**
   - The project is licensed under **MIT License**.
   - Preserves standard permissive open-source usage with attribution retained.
3. **Lock-Free Concurrency:**
   - Never hold standard blocking locks across async `.await` boundaries.
   - Use `tokio::sync`, atomic counters (`AtomicU64`, `AtomicBool`), or `parking_lot::RwLock` for synchronous operations only.
4. **Thinking Stream Resilience:**
   - All SSE streams must maintain heartbeat/ping frames (`: ping\n\n`) when reasoning models (Claude 3.7, o1, o3, Gemini 2.5) take extended time before first token arrival.
   - Always map and recover reasoning tokens (`reasoning_content` in OpenAI chunks; `thinking` blocks in Claude).
5. **Dual-Mode Reasoning:**
   - Both suffix-based clones (`(low)`, `(medium)`, `(high)`, `(no-thinking)`) and native protocol parameters (`reasoning_effort`, `thinking`) are parsed via the declarative `REASONING_SUFFIXES` table in `src/proxy/router.rs`.
6. **Strict Internationalization (i18n):**
   - Every single user-visible string in the UI, system tray, notifications, test runner, and status indicators must be declared in `i18n/{en,ru,zh}.json` and mirrored in `src/i18n.rs` (`TranslationBundle`) and `ui/app.slint` (`I18nData`).
   - Hardcoded English UI strings are prohibited.
7. **Cleanliness & Linter Standard:**
   - Zero clippy warnings (`cargo clippy --message-format=short`).
   - All 15+ router, mapper, balancer, and model tests must pass at all times (`cargo test`).

## Release Packaging & Artifacts
The release workflow (`.github/workflows/release.yml`) builds exactly two standalone release artifacts:
- `AMR-0.1.3-x86_64.AppImage` (Linux)
- `AMR-0.1.3-x86_64.exe` (Windows)

Do not add unnecessary packaging formats or complex installer wrappers.

## Maintenance Policy
AMR prioritizes stability, sub-millisecond dispatch performance, and zero bloat. Releases occur on significant protocol shifts or when a solid bundle of improvements has accumulated.
