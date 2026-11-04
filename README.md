# tingyi (聽譯)

[English](README.md) | [繁體中文](README.zh-TW.md)

A lightweight Windows desktop app that listens to what your PC is playing (English, Japanese or Korean speech), recognizes it locally in real time and shows Traditional Chinese (Taiwan) subtitles in a floating overlay. It runs fully offline by default and needs no API key.

It also ships a Chrome extension for bilingual web page translation that uses the same local translation model:

- Translate web pages in place, with per-site auto translation (e.g. Reddit, X).
- Translate YouTube captions and show only the translation.
- For YouTube videos **without** captions, tingyi listens to the browser audio and draws live subtitles inside the player.

## Install (end users)

1. Download from [Releases](../../releases):
   - `tingyi_<version>_x64-setup.exe` — the Windows installer
   - `tingyi-extension-<version>.zip` — the unpacked Chrome extension
2. Run the installer, then open tingyi once (it registers itself for the Chrome extension).
3. In tingyi's main window, download the translation models if they are missing (llama.cpp and Hy-MT2 1.8B under **Translation model**).
4. Extract the extension zip, open `chrome://extensions`, turn on **Developer mode**, click **Load unpacked** and choose the extracted folder.
5. Open any page and use the extension popup (or `Alt+A`) to translate. The popup also has per-site settings: auto translate this site, translate YouTube captions, and use tingyi when a video has no captions.

Notes:

- When the extension needs translation, it starts tingyi in the background automatically. The main window has a **Web translation** card that shows the status and lets you start or stop it.
- "Use tingyi when a video has no captions" captures the whole browser's audio, so sound from other tabs is recognized too.
- Subtitles target Traditional Chinese (Taiwan) by default. Under **Translation model** you can change the target language and how Simplified Chinese output is converted.

## Architecture

```
Audio source (WASAPI loopback: all system audio, or one app's process tree like OBS; switchable while running)
  → 16 kHz mono → Silero VAD → loudness normalization
  → ASR: SenseVoice-Small int8 (sherpa-onnx, CPU; also drives the live preview)
         or dedicated models (CPU: ReazonSpeech k2 v2 for Japanese, Parakeet TDT v2 for English)
         or Whisper large-v3-turbo (whisper-server sidecar, CUDA, optional)
  → cleanup: repetition suppression + user replacement table
  → sentence completeness check per language; unfinished segments are merged with the next one
  → Hy-MT2 (llama-server sidecar, Vulkan) streaming translation + Simplified → Traditional (Taiwan) post-processing
  → Tauri 2 events → React main window (history, metrics) + overlay window (topmost, transparent, click-through)

Web translation (Chrome extension `web/extension`)
  → native messaging host (the same tingyi.exe, `com.jerry.tingyi.host`) reads runtime.json and wakes tingyi in the background
  → shares one llama-server with subtitles (-np 4: slot 0 subtitles, 1 refine, 2–3 web; API key protected)
  → YouTube without captions: connectNative → tingyi named pipe (current user only) → subtitles in a closed shadow root inside the player
```

| Path | Contents |
|---|---|
| `crates/tingyi-core` | Pure Rust core: audio capture and sources, VAD, ASR, language routing, model download, cleanup, CER/WER eval, MT prompts, post-processing, pipeline, metrics, llama-server management, shared translation server |
| `crates/tingyi-bench` | Benchmark CLI: `devices` / `capture-probe` / `vad` / `asr` / `asr-file` / `asr-eval` / `mt` / `live` / `serve` / `download-model` |
| `src-tauri` | Tauri 2 backend: commands, state, event forwarding; native messaging host, runtime.json, tray, named pipe for YouTube live subtitles |
| `src` | React 19 + Tailwind 4 frontend: `App.tsx` main window, `Overlay.tsx` floating subtitles |
| `web` | Web translation extension (merged from agent-translator via git subtree): `extension/` (Chrome MV3), `packages/core`, `bridge/` (optional CLI-agent MCP server, not wired into tingyi); separate pnpm workspace |
| `scripts/download-models.ps1` | Downloads models and llama-server into the data directory (pinned Hugging Face commits, SHA256 verified) |
| `data/` (gitignored) | Models, llama-server, test audio |

## Development

Requirements: Windows 10/11, Rust 1.92+ (msvc), Node 24, pnpm 10, WebView2, a Vulkan-capable GPU (the default models need about 2.5 GB of VRAM).

```powershell
pnpm install
powershell -ExecutionPolicy Bypass -File scripts\download-models.ps1 -With7B -With1_8B_Q4
pnpm tauri dev
```

Data directory resolution: `TINGYI_DATA_DIR` → `data/` in the repo (if it exists) → `%LOCALAPPDATA%\tingyi`.

Extension:

```powershell
pnpm --dir web install
pnpm --dir web --filter @atx/extension build   # output: web/extension/dist (load unpacked)
```

Tests:

```powershell
cargo test --workspace
pnpm --dir web test
pnpm build
```

## Licenses

- Code: MIT
- Models: Hy-MT2 (Tencent, Apache-2.0), SenseVoice-Small (Apache-2.0, converted by sherpa-onnx), Silero VAD (MIT), llama.cpp (MIT), ReazonSpeech k2 v2 (Apache-2.0), Parakeet TDT 0.6B v2 (NVIDIA, CC-BY-4.0, attribution required)
- Bundled [tao](https://github.com/tauri-apps/tao) 0.37.1 (Apache-2.0) in `third_party/tao-0.37.1/`, with one assert patched (see `TINGYI-PATCH.md` there)
- [opencc-js](https://github.com/nk2028/opencc-js) (MIT / Apache-2.0) for Simplified → Traditional conversion in the extension
- Evaluation sets are for local measurement only and are not distributed with this repo (ReazonSpeech test, JSUT basic5000, Google FLEURS)
