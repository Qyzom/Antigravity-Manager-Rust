<div align="center">

# Aroxy

**Ultra-lightweight, high-performance AI API proxy & protocol emulator.**  
*Native Linux & Windows • Pure Rust • Slint GUI • Zero Telemetry*

[![License: CC BY-NC-SA 4.0](https://img.shields.io/badge/License-CC_BY--NC--SA_4.0-lightgrey.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2021_Edition-orange.svg)](https://www.rust-lang.org/)
[![Slint](https://img.shields.io/badge/UI-Slint_1.9-blueviolet.svg)](https://slint.dev/)
[![Platform](https://img.shields.io/badge/Platform-Linux%20%7C%20Windows-brightgreen.svg)]()
[![Release](https://img.shields.io/github/v/release/Qyzom/Aroxy?color=teal)](https://github.com/Qyzom/Aroxy/releases)

</div>

---

### Motivation & Background

Existing tooling in the Antigravity ecosystem for transforming AI agents into standard OpenAI/Claude API endpoints is unfortunately dominated by heavy Chinese Electron wrappers bundled with third-party analytics, ads, unvetted telemetry, and excessive multi-account overhead.

**Aroxy** was built out of frustration with that status quo. The goal was simple: create an ultra-fast, clean, and transparent native application that does one job reliably without tracking, background ads, or massive Chromium memory bloat.

Protocol mapping concepts and core ideas are derived from [Antigravity-Manager](https://github.com/lbjlaq/Antigravity-Manager) by lbjlaq. Aroxy is a complete architectural reimagining and codebase purge:
- **Telemetry Stripped:** 100% of remote analytics, update checkers, Chinese API callbacks, and promotional services have been eliminated.
- **Webview Discarded:** Zero Electron, zero Tauri v2, zero React, and zero Node.js. The interface has been completely rewritten from scratch in native **Rust + Slint**.
- **Bloat Removed:** Complex multi-account swappers, auto-patchers, and embedded cloudflared binaries were discarded in favor of a lean, focused local proxy.
- **Engine Hardened:** Implemented lock-free streaming via Tokio/Axum, fixing upstream concurrency deadlocks and eliminating timeout dropouts on deep-thinking models (#3816/#3817).

---

### Comparison

| Feature | Upstream / Typical Electron Wrappers | Aroxy |
| :--- | :---: | :---: |
| **Tech Stack** | Electron / Tauri v2 + React + Node.js | **Pure Rust (Tokio + Axum + Slint)** |
| **Memory Footprint** | ~250 – 500 MB RAM | **~18 MB RAM** |
| **Startup Time** | 3 – 8 seconds | **< 80 ms** |
| **Third-Party Telemetry & Ads** | Present / Phone-home trackers | **Zero (Completely offline & local)** |
| **Single Portable Binary** | ❌ Multi-file package / Webview runtime | **✔ Portable AppImage & single .exe** |
| **Headless Daemon Mode** | ❌ No native CLI daemon | **✔ Yes (`--headless`)** |
| **Thinking Model Stability** | ❌ Timeouts on 10s+ reasoning phases | **✔ Automatic keepalive SSE heartbeats** |
| **Reasoning Effort Support** | Rigid model definitions | **✔ Dual-mode (Virtual clones + native parameters)** |

---

### Features

- **Multi-Protocol Emulation:** Emulates `/v1/chat/completions`, `/v1/models`, and Anthropic `/v1/messages` with streaming SSE translation.
- **Dual-Mode Reasoning Effort:**
  - **Virtual Clones:** Automatically broadcasts `(low)`, `(medium)`, `(high)`, and `(no-thinking)` model variants for clients without native reasoning controls (e.g., `claude-3-7-sonnet (medium)`).
  - **Native Protocol Control:** Transparently honors OpenAI `reasoning_effort` and Anthropic `thinking.budget_tokens`.
- **Model Catalog Filtering:** Enable or disable specific models from being broadcasted via `/v1/models` directly from the UI.
- **Anti-Detection Fingerprinting:** Selectable client profiles (`Claude Code 2.x`, `Cursor Pro`, `Antigravity CLI`, `OpenCode`) with authentic User-Agent and protocol headers.
- **Modern Dark UI:** Minimalist monochrome design inspired by modern desktop interfaces.
- **Background Tray Daemon:** Minimize to system tray on close with zero resource waste.

---

### Release Binaries

Each release provides exactly two portable, standalone artifacts:
- **Linux:** `Aroxy-0.0.1-x86_64.AppImage`
- **Windows:** `Aroxy-0.0.1-x86_64.exe`

---

### Usage

```bash
# Launch GUI
./aroxy

# Run in background / headless mode (e.g. servers, Docker)
./aroxy --headless --port 8045

# Specify custom listening port
./aroxy --port 9000
```

#### Client Configuration

**Claude Code CLI:**
```bash
export ANTHROPIC_BASE_URL="http://127.0.0.1:8045"
export ANTHROPIC_API_KEY="dummy"
claude
```

**Cursor / Aider / OpenAI SDK:**
```bash
export OPENAI_BASE_URL="http://127.0.0.1:8045/v1"
export OPENAI_API_KEY="dummy"
```

---

### Maintenance & Update Policy

Aroxy is maintained with stability as its first priority. It may be updated less frequently than upstream projects, but **it will not be abandoned**. 

Updates will be published when:
1. Upstream AI providers make significant changes to their authentication or wire protocols.
2. A meaningful accumulation of bug fixes, model additions, or performance improvements is ready for release.

---

### License & Attribution

This project is licensed under the **Creative Commons Attribution-NonCommercial-ShareAlike 4.0 International License (CC BY-NC-SA 4.0)**, preserving the upstream licensing terms of [Antigravity-Manager](https://github.com/lbjlaq/Antigravity-Manager) by lbjlaq.

Non-commercial use, modification, and redistribution are permitted under the terms of CC BY-NC-SA 4.0 with attribution retained.
