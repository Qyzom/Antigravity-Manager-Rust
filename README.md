<div align="center">

<img src="assets/icons/logo_256.png" alt="AMR Logo" width="110" />

# AMR (Antigravity Manager Rust)

**Ultra-lightweight, high-performance AI protocol proxy & multi-account quota manager.**  
*Native Linux & Windows • Pure Rust • Hardware-Accelerated Slint GUI • Zero Telemetry*

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2021_Edition-orange.svg)](https://www.rust-lang.org/)
[![Slint](https://img.shields.io/badge/UI-Slint_1.9-blueviolet.svg)](https://slint.dev/)
[![Platform](https://img.shields.io/badge/Platform-Linux%20%7C%20Windows-brightgreen.svg)]()
[![Release](https://img.shields.io/github/v/release/Qyzom/Antigravity-Manager-Rust?color=teal)](https://github.com/Qyzom/Antigravity-Manager-Rust/releases)

<br/>

<img src="assets/screenshots/dashboard.png" alt="AMR Dashboard" width="780"/>

<br/><br/>

<img src="assets/screenshots/accounts.png" width="380"/>
<img src="assets/screenshots/models.png" width="380"/>

<br/>

<img src="assets/screenshots/balancer.png" width="380"/>
<img src="assets/screenshots/logs.png" width="380"/>

<br/>

<img src="assets/screenshots/stats.png" width="380"/>
<img src="assets/screenshots/settings.png" width="380"/>

</div>

---

### Overview & Philosophy

**AMR (Antigravity Manager Rust)** is an ultra-fast, cross-platform AI protocol gateway and account balancer built from the ground up in pure Rust. It unifies multiple upstream AI accounts into standard local endpoints compatible with **OpenAI** (`/v1/chat/completions`, `/v1/models`, `/v1/usage`), **Anthropic Claude** (`/v1/messages`), and developer tools like Cursor, Claude Code, NextChat, and DeepSeek Harness.

Unlike heavy Electron wrappers or WebKit-based shells that spawn full Chromium/WebKit browser engines consuming hundreds of megabytes of RAM just to route HTTP requests, AMR is engineered with:
- **Zero Bloat & Zero Telemetry:** Completely free of trackers, ads, analytics, Sentry, OpenTelemetry, or phone-home requests.
- **Microscopic Memory Footprint:** Idles at **~22 MB RSS** with the full hardware-accelerated GUI and **< 3 MB** in headless CLI daemon mode.
- **Direct Native Rendering:** Powered by **Slint 1.9** on top of FemtoVG / OpenGL with sub-millisecond event dispatch and zero web runtime dependencies.
- **Stream Resilience & Pacing:** Automatic SSE heartbeat frames (`: ping\n\n`) to prevent client timeouts during deep reasoning phases (Claude 3.7 Thinking, o1/o3, Gemini 2.5), combined with smooth micro-chunk stream pacing.

---

### Key Features

- **Multi-Protocol Emulation:** Native translation between OpenAI (`/v1/chat/completions`), Claude (`/v1/messages`), and Google Cloud Code PA protocols with full reasoning support.
- **Dual-Mode Thinking & Reasoning:** Supports both declarative model suffixes (`-low`, `-medium`, `-high`, `(low)`) and native protocol parameters (`reasoning_effort`, `thinking` budget).
- **Quota-Aware Load Balancing:** Real-time quota tracking with automatic 429 failover, sticky session affinity for prompt KV cache reuse, and configurable emergency reserve thresholds.
- **OpenAI Billing & Usage Emulation:** Compatible with tools and web UIs querying `/v1/dashboard/billing/subscription`, `/v1/dashboard/billing/usage`, and `/v1/usage`.
- **Anti-Fingerprint Protection:** Cryptographic device ID and machine GUID isolation per upstream account, paired with randomized jitter timing (+-15%) to evade anti-abuse heuristics.
- **Traffic Inspector & Persistent Logging:** Live searchable HTTP log inspector with token usage breakdowns (Prompt, Completion, Thinking), latency metrics, and payload viewer.
- **Strict Internationalization (i18n):** Native support for English, Russian, and Chinese across all UI views, system tray menus, and OS notifications.

---

### Performance Comparison

| Benchmark / Specification | AntigravityManager (Electron) | Antigravity-Manager (Tauri) | AMR (Antigravity Manager Rust) |
| :--- | :---: | :---: | :---: |
| **Language & Engine** | TypeScript / Electron (Chromium + Node.js) | Rust + Tauri (WebKitGTK Web Engine) | **Pure Rust (Tokio + Axum + Slint)** |
| **GUI Memory Footprint (RSS)** | ~220 – 380 MB | ~120 – 180 MB | **~22 MB** |
| **Headless / Daemon RAM** | ❌ Not available | ❌ Not available | **~2.5 MB** |
| **Cold Startup Latency** | 3.5 – 5.0 s | 1.5 – 2.2 s | **< 60 ms** |
| **Binary / AppImage Size** | ~95 MB | ~45 MB (+ WebKitGTK runtime) | **~12 MB (Standalone)** |
| **External Web Engine Required** | Yes (Bundled Chromium) | Yes (System WebKitGTK) | **❌ None (Native OpenGL / FemtoVG)** |
| **Telemetry & Trackers** | Sentry, Clarity, OpenTelemetry | None | **Zero Bloat / Zero Telemetry** |
| **License** | MIT License | CC-BY-NC-SA-4.0 (Non-Commercial) | **MIT License (Permissive Open-Source)** |
| **SSE Heartbeat Injection (`: ping\n\n`)** | ❌ Raw upstream SSE | ❌ Raw upstream SSE | **✔ Native Auto-Injection** |
| **Stream Micro-Chunk Pacing** | ❌ Bursty | ❌ Bursty | **✔ Configurable Sub-Token Pacing** |
| **OpenAI Billing Emulation (`/v1/usage`)** | ❌ None | ❌ None | **✔ Built-in Emulation** |
| **Dual-Mode Reasoning Effort** | ⚠️ Limited | ⚠️ Suffix Only | **✔ Suffixes + Native Protocol** |

> *\* Real-world benchmarks performed on **Arch Linux** under identical test conditions.*  
> *\* Software versions tested: AMR `v0.1.0` vs AntigravityManager `v0.30.0` (Draculabo) vs Antigravity-Manager `v5.0.0` (lbjlaq).*

---

### Quick Start

#### Linux (AppImage)

```bash
# Download latest standalone AppImage
chmod +x AMR-0.1.0-x86_64.AppImage

# Launch Graphical UI:
./AMR-0.1.0-x86_64.AppImage

# Run in headless mode as a background proxy server:
./AMR-0.1.0-x86_64.AppImage --headless --port 8045

# Start minimized to system tray:
./AMR-0.1.0-x86_64.AppImage --start-minimized
```

#### Windows

Download `AMR-0.1.0-x86_64.exe` from the [**Releases**](https://github.com/Qyzom/Antigravity-Manager-Rust/releases/latest) page and run it. AMR automatically integrates with the Windows Notification Area (System Tray) and supports automatic startup via the Settings menu.

---

### CLI Command Options

```text
AMR (Antigravity Manager Rust) — Ultra-lightweight AI API Proxy & Emulator

Usage: amr [OPTIONS]

Options:
      --headless             Run proxy in headless mode without graphical UI
  -p, --port <PORT>          Override local listening port [default: 8045]
      --start-minimized      Start minimized directly to system tray
  -c, --config <PATH>        Custom config file path
      --take-screenshots <DIR> Capture UI screenshots in English and exit
  -h, --help                 Print help
  -V, --version              Print version
```

---

### Client Integration Guide

AMR runs on `http://127.0.0.1:8045` by default. You can configure any AI client or IDE with standard OpenAI or Anthropic protocols:

#### 1. Claude Code CLI
```bash
export ANTHROPIC_BASE_URL="http://127.0.0.1:8045"
export ANTHROPIC_API_KEY="sk-amr-local-proxy"
claude
```

#### 2. Cursor IDE
- **Settings** -> **Models** -> **OpenAI API Key**: `sk-amr-local-proxy`
- **Override OpenAI Base URL**: `http://127.0.0.1:8045/v1`
- Supported models: `claude-3-7-sonnet`, `claude-3-7-sonnet-high`, `gemini-2.5-pro`, `gemini-2.5-flash`

#### 3. DeepSeek Harness / NextChat / OpenWebUI
- **Base URL**: `http://127.0.0.1:8045/v1`
- **API Key**: `sk-amr-local-proxy`

#### 4. Python OpenAI SDK
```python
from openai import OpenAI

client = OpenAI(
    base_url="http://127.0.0.1:8045/v1",
    api_key="sk-amr-local-proxy",
)

response = client.chat.completions.create(
    model="claude-3-7-sonnet",
    messages=[{"role": "user", "content": "Explain quantum computing in three sentences."}],
    stream=True,
)

for chunk in response:
    content = chunk.choices[0].delta.content or ""
    print(content, end="", flush=True)
```

---

### Architecture & Source Tree

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
    ├── server.rs    # Axum HTTP proxy router (/v1/chat/completions, /v1/messages, /v1/models, /v1/usage)
    ├── balancer.rs  # Quota-aware load balancing, session stickiness & KV cache retention
    ├── router.rs    # Suffix table & native reasoning effort parser, model name translation
    ├── antigravity.rs # Upstream OAuth token refresh, Cloud Code Pa API mapping, device profiles
    ├── fingerprint.rs # Cryptographic machine ID, device ID & hardware fingerprint generators
    ├── stats.rs     # Lock-free atomic metrics counters & persistent disk logging (logs.jsonl)
    ├── stream.rs    # SSE chunk transformer, heartbeat injector (: ping\n\n), reasoning parser
    └── mappers/     # OpenAI <-> Claude protocol request & response schema transformers
```

---

### Building from Source

#### Prerequisites
- Rust 1.75+ toolchain (`cargo`, `rustc`)
- Linux dependencies: `libfontconfig1-dev`, `libxkbcommon-dev` (if building native GUI on Linux)

```bash
# Clone the repository
git clone https://github.com/Qyzom/Antigravity-Manager-Rust.git
cd Antigravity-Manager-Rust

# Run test suite (18 unit & integration tests)
cargo test

# Build optimized release binary
cargo build --release
```

The resulting binary will be located at `target/release/amr`.

---

### License

AMR is distributed under the permissive **MIT License**.  
See [LICENSE](LICENSE) for details.
