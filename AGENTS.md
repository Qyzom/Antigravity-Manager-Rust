# AGENTS.md — AMR Architecture & Maintenance Guide

## Overview & Release History
Below is the definitive record of changes and protocol advancements implemented across all versions:

### Version Changelog (English)
- **v0.1.6 (Latest)**:
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
  - **Window & Tray Integration**: Fixed window minimize-to-tray lifecycle on Linux/Windows and preserved dynamic tray context actions.
  - **Prompt Caching & Telemetry**: Integrated real prompt caching ratios, request latency counters, and token tracking.
  - **UI / Model Explorer**: Added model ID copy chip, grey thinking badges, and sanitized upstream headers.
  - **Windows Dependencies**: Added `Win32_System_ProcessStatus` and `Win32_System_Threading` features to `windows-sys`.

- **v0.1.3**:
  - **Protocol Alignment**: Eliminated problematic upstream endpoints causing synthetic 429 errors and aligned request schemas to native Pa API parameters.

- **v0.1.2**:
  - **Windows Subsystem**: Configured `#![windows_subsystem = "windows"]` to suppress unwanted background console windows.
  - **OAuth & State**: Fixed URL escaping and token keeper persistence across application reboots.
  - **UI Quota**: Fixed rendering when quota reaches 0%.

- **v0.1.1**:
  - **Packaging**: Streamlined AppImage, DEB, RPM, and EXE build scripts with cross-distro compatibility.

- **v0.1.0**:
  - **Multi-Architecture CI**: Introduced standalone 64-bit ARM (`aarch64` Linux & `arm64` Windows) build matrix.

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
