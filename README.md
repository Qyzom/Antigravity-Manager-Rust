<div align="center">

# Aroxy

**Ultra-lightweight API emulator for Antigravity supporting OpenAI, Claude & Gemini protocols.**  
*Native Linux & Windows • Pure Rust • Slint GUI • Zero Telemetry*

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2021_Edition-orange.svg)](https://www.rust-lang.org/)
[![Slint](https://img.shields.io/badge/UI-Slint_1.9-blueviolet.svg)](https://slint.dev/)
[![Platform](https://img.shields.io/badge/Platform-Linux%20%7C%20Windows-brightgreen.svg)]()
[![Release](https://img.shields.io/github/v/release/Qyzom/Aroxy?color=teal)](https://github.com/Qyzom/Aroxy/releases)

</div>

---

### Motivation & Origins

Existing tools for transforming Antigravity into standard API endpoints are almost exclusively bloated Chinese Electron wrappers packed with background analytics, ads, unvetted telemetry, and excessive multi-account overhead.

**Aroxy** was built from scratch to fix this. It is a clean-room, native Rust application designed to turn Antigravity into a rock-solid local API proxy without tracking, promotional junk, or Chromium memory consumption:

- **100% Original Codebase:** Written completely from scratch on pure **Tokio**, **Axum**, and **Slint**. Zero upstream code was used or adapted.
- **Zero Webview & Zero Telemetry:** No Electron, no Tauri, no React, no Node.js. No analytics or phone-home requests under any circumstances.
- **Engineered for Reliability:** Features lock-free concurrency and automatic SSE heartbeat frames (`: ping\n\n`) to prevent connection timeouts when querying deep-thinking models (Claude 3.7 Sonnet, o1, Gemini 2.5).

---

### Comparison

| Feature | Typical Electron Wrappers | Aroxy |
| :--- | :---: | :---: |
| **Tech Stack** | Electron / Tauri + React + Node.js | **Pure Rust (Tokio + Axum + Slint)** |
| **Memory Footprint** | ~250 – 500 MB RAM | **~18 MB RAM** |
| **Startup Time** | 3 – 8 seconds | **< 80 ms** |
| **Telemetry & Ads** | Trackers / Phone-home / Ads | **Zero (Completely offline & local)** |
| **Binary Format** | Multi-file package / Webview runtime | **Portable AppImage & single .exe** |
| **Headless Mode** | ❌ No native CLI daemon | **✔ Yes (`--headless`)** |
| **Thinking Stability** | ❌ Timeouts on long reasoning phases | **✔ Automatic keepalive SSE heartbeats** |
| **Reasoning Effort** | Fixed model names | **✔ Dual-mode (Virtual clones + native parameters)** |
| **License** | Restrictive / Copyleft | **✔ Permissive MIT License** |

---

### Features

- **Multi-Protocol Emulation:** Native `/v1/chat/completions`, `/v1/models`, and Anthropic `/v1/messages` compatibility with real-time SSE stream translation.
- **Dual-Mode Reasoning Effort:**
  - **Virtual Clones:** Automatically exposes `(low)`, `(medium)`, `(high)`, and `(no-thinking)` model variants for clients without reasoning sliders (e.g., `claude-3-7-sonnet (medium)`).
  - **Native Protocol Control:** Transparently honors native OpenAI `reasoning_effort` and Anthropic `thinking.budget_tokens`.
- **Model Catalog Filtering:** Enable or disable specific models broadcasted via `/v1/models` directly in the UI.
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

Aroxy is maintained with stability as its first priority. It may be updated less frequently than rapidly churning upstream tools, but **it will not be abandoned**. 

Updates will be published when:
1. AI providers make significant changes to their wire protocols.
2. A meaningful accumulation of bug fixes, model additions, or performance improvements is ready for release.

---

### License

Distributed under the **MIT License**. See [LICENSE](LICENSE) for details.  
Copyright © 2026 Qyzom.
