# Aroxy ⚡

[![Release Build](https://github.com/Qyzom/Aroxy/actions/workflows/release.yml/badge.svg)](https://github.com/Qyzom/Aroxy/actions/workflows/release.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](https://opensource.org/licenses/MIT)

> **Ultra-lightweight, high-performance cross-platform AI protocol proxy & emulator written in Rust + Slint.**

Aroxy strips away the bloat of Electron, Tauri, WebViews, and telemetry from heavy AI proxy managers, delivering a pure native single binary (~20 MB RAM footprint) with instantaneous startup, system tray support, and rock-solid protocol emulation for **OpenAI v1**, **Anthropic Claude**, and **Google Gemini**.

---

## ✨ Features

- **🚀 Ultra-lightweight & Native:** Built purely in Rust with the Slint GUI toolkit (`femtovg` renderer). Zero Chromium, zero Node.js, zero WebViews.
- **🔄 Multi-Protocol Emulation:**
  - Full **OpenAI v1** endpoint emulation (`/v1/chat/completions`, `/v1/models`).
  - Native **Anthropic Messages** emulation (`/v1/messages`).
  - Google Gemini format mapping.
- **🧠 Thinking / Reasoning Recovery:** Full support for thinking models (Claude 3.7 Sonnet reasoning tokens, OpenAI `reasoning_content`, and Gemini thoughts) without timeout cliff disconnects.
- **🛡️ Anti-Detection Fingerprinting:** Built-in spoofing profiles (`Claude Code 2.x`, `Cursor Pro`, `Antigravity CLI`, `OpenCode`) to bypass bot detection.
- **📊 Real-time Traffic Inspector:** Lock-free in-memory request logger, latency tracker (TTFT), and token statistics.
- **🪟 Cross-Platform & Portable:**
  - Runs in background with native system tray daemon (`tray-icon` + `muda`).
  - Supports `--headless` mode for headless Linux servers / Docker.
  - Builds portable Windows `.exe` and Linux AppImage.

---

## ⚡ Quick Start

### Build & Run
```bash
# Clone the repository
git clone https://github.com/Qyzom/Aroxy.git
cd Aroxy

# Run with GUI
cargo run --release

# Or run in headless daemon mode
cargo run --release -- --headless --port 8045
```

### Configure Clients

#### Claude Code CLI
```bash
export ANTHROPIC_BASE_URL="http://127.0.0.1:8045"
export ANTHROPIC_API_KEY="dummy-or-your-key"
claude
```

#### Cursor / Aider / OpenAI SDK
```bash
export OPENAI_BASE_URL="http://127.0.0.1:8045/v1"
export OPENAI_API_KEY="dummy-or-your-key"
```

---

## 🏆 GitHub Achievements Integration

Aroxy is configured with commit metadata to help unlock developer badges on GitHub:
- **Pair Extraordinaire:** Co-authored commits with [`@qwezzxxx`](https://github.com/qwezzxxx).
- **Pull Shark / YOLO:** Branch pull requests merged to `main`.
- **Quickdraw:** Close an issue / PR within 5 minutes.

---

## 📄 License

MIT License © 2026 Qyzom & Aroxy Contributors.
