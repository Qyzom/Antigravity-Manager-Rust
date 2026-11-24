# AGENTS.md — AMR Architecture & Maintenance Guide

## Overview & Release History
Below is the definitive record of changes and protocol advancements implemented across all versions:

### Version Changelog (English)
- **v1.0.0 (Latest / Production LTS)**:
  - **Milestone 1.0.0 Production Release**: First official major production release marking complete API stability, zero telemetry, and long-term maintenance readiness.
  - **Complete Removal of Unsupported TTS & Image Gen Modalities**: Fully purged legacy TTS routes (`/v1/audio/speech`), synthetic fallback layers, and image generation endpoints (`/v1/images/generations`). Upstream Cloud Code Pa restricts image generation to Enterprise tier licenses (`#3501`) and does not offer stateless REST TTS. AMR now strictly exposes 100% stable, authentic upstream capabilities.
  - **Authentic Speech-to-Text Engine (`gemini-3.5-transcribe`)**: Native speech recognition powered by Gemini multimodal audio processing. Transparent OpenAI Whisper SDK compatibility (`whisper-1` and `stt-1` aliases auto-routed to `gemini-3.5-transcribe`).
  - **Deep Codebase Optimization & Legacy Spaghetti Purge**: Stripped 800+ lines of dead modal code, unused Slint properties, redundant translation keys, and unreferenced fallback branches. Slimmer GUI footprint and zero runtime allocations for test playgrounds.
  - **Relocated Master API Key & Streamlined Models Catalog**: Master API Key management placed directly below Network Service Binding for instant access. Clean model catalog focused on verified chat, search, and speech recognition.
  - **Diagnostic Engine Overhaul**: Simplified diagnostic modal with instant HTTP 200 OK verification across chat, search grounding, and speech transcription models.
  - **Window Ergonomics & Responsiveness**: Enforced responsive 1140x740 Slint canvas without horizontal scrollbars, flexible wrapping for internationalized labels, and dynamic tray integration.

- **v0.1.8**:
  - **Complete Removal of Unsupported TTS & Image Gen Modalities**: Fully purged legacy TTS routes (`/v1/audio/speech`, `/audio/speech`), `tts-1` catalog entries, and image generation endpoints (`/v1/images/generations`). Verified via upstream reverse-engineering that Cloud Code Pa does not offer stateless REST TTS and restricts image generation to Enterprise tier licenses (`SUBSCRIPTION_REQUIRED #3501`). AMR now strictly exposes 100% functional upstream features without artificial or unstable layers.
  - **Authentic Speech-to-Text (STT) Engine (`gemini-3.5-transcribe`)**: Replaced OpenAI legacy `whisper-1` catalog entry with authentic Antigravity internal model `gemini-3.5-transcribe` (provider `google`), backed by Google Gemini multimodal audio processing. Added transparent alias fallback for OpenAI SDK clients requesting `whisper-1` or `stt-1`.
  - **Relocated Master API Key & Clean Models Catalog**: Moved Master API Key management card right underneath Network Service Binding in the Proxy Layer tab for immediate visibility. Model catalog streamlined to verified chat, search, and speech recognition (`gemini-3.5-transcribe`).
  - **Multimodal Diagnostic Runners (TTS, STT, Image Gen)**: Overhauled diagnostic engine in `run_diagnostic_check` with dedicated test handlers for speech synthesis (`/v1/audio/speech`), speech recognition, and image generation, ensuring authentic 200 OK diagnostic checks.
  - **Refined Slint UI & Window Responsiveness**: Increased standard window dimensions to 1140x740 with strict `content-width: self.visible-width` to eliminate awkward horizontal scrollbars. Fixed clipped preference label layouts in Settings tab with flexible text wrapping and zero min-width constraints.
  - **Streamlined MCP Surface**: Removed legacy decorative badges and placeholder logos from MCP Master Card in favor of clean native indicators.

- **v0.1.7**:
  - **Antigravity v44.9.1 TTS & Live Mode Integration**: Native support for all 9 official Antigravity voices (`Fola`, `Arlo`, `Cleo`, `Finn`, `Gero`, `Lora`, `Neno`, `Olin`, `Tari`), seamless bidirectional mapping for Gemini Native (`Zephyr`, `Puck`, `Charon`...) and OpenAI voice aliases (`alloy`, `echo`, `ash`...), and automatic fallback pairing.
  - **Ultra-Fast Live TTS Models**: Integrated lightweight `gemini-3.8-flash-lite-tts` for sub-200ms Live voice latency alongside `gemini-3.8-flash-tts` and `gemini-3.1-flash-tts-preview`, plus broadcasted 27-voice capability catalog in `GET /v1/models`.
  - **Hardened MCP Web Fetching & SSRF Defense**: Comprehensive protection against SSRF in `fetch_web_page` (rejects loopback, internal, private, and documentation IP ranges with DNS pre-resolution and redirect validation), strict 2 MiB response cap, and Unicode-safe HTML tag stripper.
  - **Atomic Configuration Persistence**: Named temporary file staging (`tempfile`) and atomic file replacement preventing configuration truncation or corruption during simultaneous writes.
  - **Instant Proxy Port & Host Hot-Reload**: Rebinds Axum HTTP listener immediately on port or host change without stopping or manually restarting the proxy daemon.
  - **Comprehensive 429 & Dispatcher Logging**: Structured pre-dispatch logging across OpenAI (`/v1/chat/completions`), Claude (`/v1/messages`), and Google Gemini (`/v1beta/models/*`) recording exact error messages, request payloads, and remaining cooldown seconds (`retry in Xs`).
  - **MCP Search Results Limit & Enhanced Web Tools**: Added customizable search results limit selector (`mcp_search_max_results`: 3, 5, 10, 15) for `google_web_search`, intelligent URL auto-prefixing (`https://`), and friendly query validation for `fetch_web_page`.
  - **Streamlined MCP & Settings UI**: Stripped fake logo, active badge, tools label, and test button from the MCP card; fixed horizontal layout stretching and viewport cut-offs in Settings preferences.
  - **Interactive GitHub Update Dispatcher**: Persistent update notification button that directly opens the latest GitHub release in the default browser, with optional automatic startup check.

- **v0.1.6**:
  - **Models & MCP Master Control**: Unified Models tab with MCP Master Card, live status indicator, client configuration exporter, and interactive MCP tool testing modal (`google_web_search`, `fetch_web_page`).
  - **Model Status & Diagnostics Modal**: Replaced legacy test playground with instant diagnostic dialog displaying model metadata, per-account availability breakdown, last error, and a single "Run Diagnostics" button.
  - **Clean Model IDs & Hard Proxy Blocking**: Stripped `-disabled` suffix from model identifiers and enforced hard HTTP 403 proxy-level blocking for disabled models across all endpoints (`/v1/chat/completions`, `/v1/messages`, `/v1beta/*`, `/v1/audio/*`, `/v1/images/*`, `/v1/search`).
  - **Default Thinking Grouping**: Enabled base model grouping by default with inverted toggle (*«Разгруппировать модели по уровню мышления»* / Ungroup thinking models), routing reasoning effort parameters exclusively to base models.
  - **Decomposed Architecture Settings**: Completely removed separate About tab, embedding AMR system info card into Settings. Distinct modular sections: Proxy Layer, Spoofing, and Settings.
  - **Standardized Error Normalizer**: Dynamic error handler with localized explanations (RU/EN/ZH) prefixed with `AMR: Spoof error — ...` / `AMR: Ошибка спуфинга — ...`.
  - **Pure Claude & Gemini Quotas**: Eliminated confusing OpenAI/GPT quota labels, focusing strictly on Claude and Gemini quotas with active models count summary.
  - **Account Editor Enhancements**: Fixed subpixel rendering blur (`round(...) / 2px * 1px`), added live proxy connection test button with latency display.
  - **Emoji-Free Action Notifications**: Stripped all checkmark and cross emojis from user-facing action states, preserving clean localized text.

- **v0.1.5**:
  - **Triple Protocol Gateway**: Native unified support for OpenAI API (`/v1/*`), Anthropic Claude API (`/v1/messages`), and Google Gemini Native API (`/v1beta/models/*:generateContent` / `streamGenerateContent`).
  - **Embedded Model Context Protocol (MCP) Server**: Built-in `/mcp` & `/v1/mcp` HTTP endpoint providing `google_web_search` and `fetch_web_page` tools for Claude Code, Cursor, Cline, Windsurf, and AI agents.
  - **Claude Streaming Thinking Auto-Heal**: Automatic `signature_delta` and synthetic thinking signature injection preventing Claude Code CLI and Anthropic clients from failing on empty thought signatures or out-of-order blocks.
  - **Zero-Hardcode Dynamic Model Discovery**: Removed legacy static `gemini-2.5-*` models; AMR now dynamically discovers and loads active recommended models (`agentModelSorts`) and multimodal capabilities directly from upstream accounts.
  - **Antigravity 2.29.1 Client Emulation**: Updated upstream hub version and CL metadata (`antigravity/hub/2.29.1`, `CL 1015941558`) to match latest Antigravity desktop release (`44.5.1`).
  - **Upstream Image, STT, & TTS Protocol Fixes**: Switched `/v1/images/generations`, `/v1/audio/transcriptions`, and `/v1/audio/speech` to non-streaming `v1internal:generateContent` (`Accept: application/json`) with automatic OAuth token refresh, aspect ratio support, PCM-to-WAV wrapping, and authentic voice mapping.
  - **Web Search (`POST /v1/search` & `googleSearch` Grounding)**: Reverse-engineered Antigravity `GetWebSearchResults` (`requestType: "web_search"`, `googleSearch` tool, `groundingMetadata` sources) and exposed `POST /v1/search`, the `web-search` catalog model, and OpenAI `web_search` tool mapping in `/v1/chat/completions`.
  - **Metadata-Driven Version Placeholders & Packaging**: Synchronized `Cargo.toml` (`0.1.5`), Slint UI version, metainfo XML/desktop stamping in `build_appimage.sh` / `build_deb.sh` / `build_rpm.sh`, `README.md` placeholders, and dynamic changelog extraction in `.github/workflows/release.yml`.

- **v0.1.4**:
  - **Window & System Tray Integration**: Fixed window minimize-to-tray lifecycle on Linux/Windows and preserved dynamic tray context actions.
  - **Prompt Caching & Telemetry**: Integrated real prompt caching ratios, request latency counters, and token tracking.
  - **Audio STT & TTS Protocol**: Exposed `/v1/audio/transcriptions` and `/v1/audio/speech` with authentic voice mapping and model cataloging.
  - **UI / Model Explorer**: Added model ID copy chip, grey thinking badges, and sanitized upstream headers.
  - **DeepSeek Reasoning Metadata**: Integrated DeepSeek harness reasoning slider, metadata injection, and upstream response formatting.
  - **Windows Dependencies**: Added `Win32_System_ProcessStatus` and `Win32_System_Threading` features to `windows-sys`.

- **v0.1.3**:
  - **Protocol Alignment**: Eliminated problematic upstream endpoints causing synthetic 429 errors and aligned request schemas to native Pa API parameters.
  - **Session Stickiness & KV Cache Retention**: Improved session affinity routing to maximize upstream prompt caching hit ratios across conversational turns.
  - **Resilient TCP Listener**: Added exponential retry backoff on server startup to handle transient socket `TIME_WAIT` states gracefully.

- **v0.1.2**:
  - **Windows Background Console Suppression**: Configured `#![windows_subsystem = "windows"]` to suppress unwanted background console popups.
  - **OAuth & State Keeper Persistence**: Fixed URL parameter escaping and token keeper persistence across system reboots.
  - **Zero-Quota UI Stabilization**: Fixed progress and graph rendering when account quota drops to 0%.
  - **Emergency Quota Protection Threshold**: Added configurable account reserve cutoff to avoid complete account exhaustion.

- **v0.1.1**:
  - **Packaging & Distro Compatibility**: Streamlined AppImage, DEB, RPM, and EXE build scripts with broad Linux distro compatibility.
  - **Headless Daemon CLI Mode**: Added `--headless`, `--port`, `--config`, and `--start-minimized` flags for server and terminal environments.
  - **Cross-Architecture Build Matrix**: Unified automated build pipeline across Linux and Windows 64-bit architectures.

- **v0.1.0**:
  - **Multi-Architecture Release**: Introduced standalone 64-bit ARM (`aarch64` Linux & `arm64` Windows) build matrix alongside Intel/AMD `x86_64`.
  - **Account Quarantine & Cooldown**: Intelligent exponential backoff quarantine for failing upstream accounts to prevent cascading errors.
  - **Hardware Isolation & Steganographic Spoofing**: Distinct machine GUIDs, client IDs, and hardware fingerprint profiles per upstream account.
  - **Pure Rust Native Architecture**: Sub-millisecond latency, zero telemetry, and minimal memory footprint (~22 MB RSS).

- **v0.0.4**:
  - **Antifraud & Identity**: Implemented randomized jitter refresh (±15%), hardware fingerprint generators, and comprehensive trilingual i18n (EN, RU, ZH).

- **v0.0.3**:
  - **Security & Storage**: Added persistent SQLite token cache, device isolation, and per-account proxy support.

- **v0.0.2**:
  - **Models**: Enriched `/v1/models` broadcast with reasoning capabilities and reasoning clone toggle.

- **v0.0.1**:
  - **Core**: Initial release of pure Rust AI API proxy and protocol emulator under MIT License.

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
    ├── audio.rs     # Audio STT (/v1/audio/transcriptions), TTS (/v1/audio/speech), Images (/v1/images/generations)
    ├── mcp.rs       # Model Context Protocol (MCP) server & direct tool execution (search & fetch)
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
The release workflow (`.github/workflows/release.yml`) builds standalone release packages for both Intel/AMD 64-bit (`x86_64`) and ARM 64-bit (`ARM64` / `aarch64`):
- **Linux — Intel/AMD 64-bit (x86_64):**
  - AppImage: `AMR-*-x86_64.AppImage` (Universal portable binary)
  - DEB: `AMR_*_amd64.deb` (Ubuntu / Debian / Mint)
  - RPM: `AMR-*-1.x86_64.rpm` (Fedora / RHEL / openSUSE)
- **Linux — ARM 64-bit (ARM64 / aarch64):**
  - AppImage: `AMR-*-aarch64.AppImage` (ARM64 portable binary for Raspberry Pi / Apple Silicon VMs)
  - DEB: `AMR_*_arm64.deb` (Ubuntu / Debian ARM64)
  - RPM: `AMR-*-1.aarch64.rpm` (Fedora / RHEL ARM64)
- **Windows — Intel/AMD 64-bit (x86_64):**
  - Portable EXE: `AMR-*-x86_64.exe` (Standalone single-file GUI binary)
- **Windows — ARM 64-bit (ARM64):**
  - Portable EXE: `AMR-*-arm64.exe` (Native ARM64 binary for Snapdragon X Elite / Surface Pro)

Do not add unnecessary packaging formats or complex installer wrappers.

## Maintenance Policy
AMR prioritizes stability, sub-millisecond dispatch performance, and zero bloat. Releases occur on significant protocol shifts or when a solid bundle of improvements has accumulated.
