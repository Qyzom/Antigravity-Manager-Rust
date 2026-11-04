<div align="center">

# Aroxy

**Ultra-lightweight high-performance AI API proxy, protocol emulator & quota dispatcher.**  
*Native Linux & Windows • Pure Rust • Slint GUI • Zero Telemetry*

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-2021_Edition-orange.svg)](https://www.rust-lang.org/)
[![Slint](https://img.shields.io/badge/UI-Slint_1.9-blueviolet.svg)](https://slint.dev/)
[![Platform](https://img.shields.io/badge/Platform-Linux%20%7C%20Windows-brightgreen.svg)]()
[![Release](https://img.shields.io/github/v/release/Qyzom/Aroxy?color=teal)](https://github.com/Qyzom/Aroxy/releases)

</div>

---

## ⚡ Overview & Core Architecture

**Aroxy** is an ultra-lightweight, high-concurrency local AI protocol gateway written in pure Rust. It transforms multiple upstream AI accounts into universal standard endpoints compatible with **OpenAI** (`/v1/chat/completions`), **Anthropic Claude** (`/v1/messages`), and `/v1/models`.

Unlike heavy Electron- or WebKit-based wrappers that consume hundreds of megabytes of RAM and run complex browser engines just for proxying, Aroxy is built from the ground up on **Tokio**, **Axum**, and a native **Slint** interface (FemtoVG/OpenGL), delivering sub-millisecond routing and idling at just **~22 MB of RAM**.

```
┌────────────────────────────────────────────────────────┐
│             Clients (Cursor, VS Code, Claude Code)     │
└───────────────────────────┬────────────────────────────┘
                            │ Standard OpenAI / Claude REST & SSE
                            ▼
┌────────────────────────────────────────────────────────┐
│                   Aroxy Core Proxy                     │
│  - Lock-free Tokio / Axum Routing                      │
│  - Session Affinity & Prompt Cache Retention           │
│  - Automatic SSE Heartbeats (: ping\n\n)               │
│  - 429 Failover & Quota Protection Threshold           │
└───────┬───────────────────────────────┬────────────────┘
        │ Device Profile #1             │ Device Profile #2
        │ SOCKS5 / HTTP Proxy #1        │ Direct / Proxy #2
        ▼                               ▼
┌──────────────────────┐        ┌──────────────────────┐
│  Google Upstream #1  │        │  Google Upstream #2  │
│  (Token Keeper Auto) │        │  (Token Keeper Auto) │
└──────────────────────┘        └──────────────────────┘
```

---

## 🛡️ Upstream Protections & Anti-Detection Architecture

Aroxy implements enterprise-grade device isolation and protocol hygiene identical to and exceeding upstream client tooling:

### 1. Persistent OAuth 2.0 Web Flow & Background Token Keeper
- **Official Credentials & Offline Consent:** Authorizes accounts via official Client ID/Secret parameters with `access_type=offline` and `prompt=consent`, ensuring Google yields both short-lived access tokens (`ya29...`) and permanent `refresh_token`s (`1//...`).
- **Proactive Token Keeper:** Access tokens naturally expire after 3,600 seconds (1 hour). Aroxy automatically checks token validity on every request and **proactively refreshes tokens 5 minutes prior to expiration**. Accounts never drop into `ERROR` states and never return spurious 401s to IDE clients.

### 2. Hardware Device & Fingerprint Isolation
Every upstream account is paired with an independent, persistent `DeviceProfile`:
- **Unique Machine Identifiers:** Distinct `X-Client-Id` (UUID v4) and `X-Machine-Id` (cryptographic SHA-256 machine hash) per account.
- **Official Protocol Headers:** Emits the official language hub User-Agent (`antigravity/hub/2.23.1 (aidev_client; os_type=linux; arch=amd64; cl=998881661)`) and runtime marker `gl-rust/1.0.0`.
- **Request Metadata:** Request bodies are decorated with authentic `"userAgent": "antigravity"`, per-request `"requestId"`, and isolated `"sessionId"`, preventing Google anti-abuse heuristics from linking distinct accounts.

### 3. Per-Account Network Proxy Pooling
- Each upstream account can be routed through its own dedicated proxy (`http://`, `https://`, `socks5://`, or direct connection).
- Prevents multi-account IP clustering and regional geolocation mismatches.

### 4. Thinking Stream Resilience & Heartbeat Frames
- Reasoning models (Claude 3.7 Sonnet, Gemini 2.5 Pro / Flash) can spend 30–90 seconds generating internal reasoning tokens before emitting the first user-visible byte.
- Aroxy automatically injects lightweight SSE heartbeat frames (`: ping\n\n`) into active streams to prevent client network timeouts (e.g. Cursor 30s timeout drop).
- Full mapping of `reasoning_content` (OpenAI format) and `thinking` delta blocks (Anthropic format).

---

## 📊 Technical Comparison

| Metric / Feature | Typical Webview Wrappers | **Aroxy (v0.0.3)** |
| :--- | :---: | :---: |
| **GUI Framework** | Electron / Tauri (Chromium / WebKit) | **Slint Native (FemtoVG / OpenGL)** |
| **Process Memory** | ~250 – 500 MB RAM | **~22 MB RAM** |
| **Startup Time** | 3.5 – 8.0 seconds | **< 60 milliseconds** |
| **Concurrency Model** | Node.js single-thread IPC | **Tokio Lock-Free Concurrency** |
| **Telemetry & Trackers** | Frequent phone-home telemetry | **Zero (100% Offline & Local)** |
| **Token Refresh** | Often manual or fragile | **Proactive Background Token Keeper** |
| **Device Isolation** | Often shares single machine-id | **Cryptographically isolated profiles** |
| **Per-Account Proxies** | Global proxy only | **Granular per-account SOCKS5/HTTP** |
| **License** | Restrictive / Commercial Copyleft | **Permissive MIT License** |

---

## 🚀 Getting Started

### Standalone Release Binaries

Aroxy produces exactly two standalone portable binaries without external dependencies:
- **Linux:** `Aroxy-0.0.3-x86_64.AppImage`
- **Windows:** `Aroxy-0.0.3-x86_64.exe`

### Running Aroxy

```bash
# Launch native GUI workstation
./Aroxy-0.0.3-x86_64.AppImage

# Run in headless daemon mode (servers, CI, Docker)
./Aroxy-0.0.3-x86_64.AppImage --headless --port 8045

# Specify custom listen port
./Aroxy-0.0.3-x86_64.AppImage --port 9000
```

### Client Integration Examples

#### 1. Claude Code CLI
```bash
export ANTHROPIC_BASE_URL="http://127.0.0.1:8045"
export ANTHROPIC_API_KEY="sk-aroxy-..."
claude
```

#### 2. Cursor / VS Code Cline / OpenAI SDK
- **Base URL:** `http://127.0.0.1:8045/v1`
- **API Key:** `sk-aroxy-...` (or any dummy key if authorization is disabled)
- **Model:** `claude-sonnet-5-5`, `gemini-3.8-flash`, etc.

---

## ⚖️ Legal Disclaimer & Terms of Use

> [!IMPORTANT]
> **READ CAREFULLY BEFORE USING THIS SOFTWARE.**

1. **Educational & Research Purpose Only:**  
   This software is provided strictly for educational, academic, and reverse-engineering research purposes to study protocol interoperability, local proxy routing architectures, and memory-efficient native GUI design. It is not intended for commercial exploitation.

2. **Provided "AS IS" Without Warranty:**  
   The software is provided under the terms of the MIT License "AS IS", without warranty of any kind, express or implied, including but not limited to the warranties of merchantability, fitness for a particular purpose, and non-infringement. In no event shall the authors or copyright holders be liable for any claim, damages, or other liability.

3. **User Responsibility & Third-Party Terms of Service:**  
   Users are solely responsible for complying with all applicable local laws, regulations, and third-party Terms of Service (including, but not limited to, Google Cloud, Anthropic, and OpenAI). The authors do not endorse, encourage, or facilitate any violation of service terms, quota abuse, or unauthorized access.

4. **Trademarks & Attribution:**  
   All product names, logos, brands, and registered trademarks mentioned within this repository or application (including "Google", "Antigravity", "Anthropic", "Claude", and "OpenAI") are the property of their respective owners. Their mention is solely for technical identification and interoperability description.

---

## 📜 License

Distributed under the permissive **MIT License**. See [LICENSE](LICENSE) for details.  
Copyright © 2026 Qyzom.
