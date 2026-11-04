# AGENTS.md — Aroxy Architecture & Maintenance Guide

## Overview
Aroxy is an ultra-lightweight, high-performance cross-platform AI protocol proxy and emulator written in pure Rust (Tokio, Axum, reqwest) with a native Slint GUI (using `backend-winit` and `renderer-femtovg`).

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
   - Both suffix-based clones (`(low)`, `(medium)`, `(high)`, `(no-thinking)`) and native protocol parameters (`reasoning_effort`, `thinking`) must be supported concurrently in `src/proxy/router.rs`.

## Release Packaging & Artifacts
The release workflow (`.github/workflows/release.yml`) builds exactly two standalone release artifacts:
- `Aroxy-0.0.3-x86_64.AppImage` (Linux)
- `Aroxy-0.0.3-x86_64.exe` (Windows)

Do not add unnecessary packaging formats or complex installer wrappers.

## Maintenance Policy
Aroxy prioritizes stability and minimal disruption. Releases occur on significant protocol shifts or when a solid bundle of improvements has accumulated.
