# ADR-0021: `unsafe` is confined to `farsight-gpu-sys`

- **Status:** Accepted
- **Date:** 2026-10-25
- **Deciders:** user (U-13, U-15); [M10 council](../council/2026-10-23-initial-roadmap.md#m10-gpu--h264)
- **Requirements:** SRV-11, ENC-8

## Context
Farsight sets `unsafe_code = "forbid"` across the workspace. M10 adds three things that safe Rust can't reach:

- **DMA-BUF capture.** The CPU must bracket its reads of a GPU buffer with `DMA_BUF_IOCTL_SYNC` and map it.
- **H.264 on AMD and Intel GPUs.** This goes through VA-API (libva).
- **H.264 on NVIDIA GPUs.** This goes through NVENC (`libnvidia-encode`, plus a CUDA context).

Two existing crates looked like they would avoid our own `unsafe`, but neither works:

- **ChromeOS's `cros-codecs`** writes no packed slice headers. Mesa's AMD driver takes the NAL unit type
  and `nal_ref_idc` only from those headers. Without them it emits invalid streams (verified on an RX 9070
  with Mesa 25.1; see [P-51](../decisions/provisional.md)).
- **`nvidia-video-codec-sdk`** links the NVIDIA libraries when the program is built, so the agent would not
  start on a computer without an NVIDIA driver.

The council's security condition still holds: GPU drivers are a large C attack surface, so these paths stay
optional and off by default.

## Decision
- **One crate holds every `unsafe` block.** It is `farsight-gpu-sys`, and nothing else in the workspace may
  use `unsafe`.
  - It sets `unsafe_code = "deny"` in place of the workspace's `forbid`, and allows `unsafe` only on
    individual blocks.
  - Clippy's `undocumented_unsafe_blocks` and `multiple_unsafe_ops_per_block` are set to deny. Every
    block therefore performs exactly one unsafe operation and carries a `// SAFETY:` comment.
- **The crate exposes safe APIs and is feature-gated:**
  - `read_dmabuf` and `sync`: DMA-BUF access, always built.
  - `va`: VA-API H.264.
  - `nvenc`: NVENC H.264.
- **Native libraries are loaded at run time** with `libloading`: `libva-drm.so.2`, `libcuda.so.1` and
  `libnvidia-encode.so.1`. Nothing links against them, so a missing driver means "no GPU encoder", never
  "won't start".
- **FFI declarations are generated** by `scripts/gen-va-bindings.sh` and `scripts/gen-nvenc-bindings.sh`, and
  committed.
  - Sources are libva 2.22 (MIT) and FFmpeg's nv-codec-headers 12.0 (MIT), each pinned by SHA-256.
  - Builds therefore need neither the headers nor clang.
  - The generated modules are exempt from lints; the wrappers around them are not.
- **Everything that can be safe lives outside the crate,** in `farsight-hwenc`. That covers:
  - the H.264 headers (SPS, PPS and slice headers, written from ITU-T H.264);
  - colour conversion;
  - keyframe planning;
  - device choice and the thread per encoder.
  This code is unit-tested without a GPU.
- **DMA-BUF reads copy the bytes out** of the mapping with a raw copy. The producer may still write the
  buffer, so no `&[u8]` is ever formed over it. The sync ioctl runs before the mapping, which refuses anything
  that isn't a DMA-BUF.
- **`scripts/check-layers.sh` fails if any other crate leaves out `[lints] workspace = true`.** This keeps the
  boundary enforced by a machine, not by memory.
- **Each encoder runs on its own thread.** VA-API and CUDA handles are not thread-safe, so the wrapper types
  are not `Send`.
- **Releases build none of these features** until the user has tested them by hand (U-14). The agent picks
  NVENC first, then VA-API, then OpenH264 in software, and falls back to lossless encodings whenever an
  encoder fails.

## Consequences
- The audit surface is one directory, about 1,100 hand-written lines (plus generated declarations), and the
  reviewers know where to look.
- The stream format belongs to us. Every GPU receives the same Main-profile, CABAC, one-reference,
  no-reordering stream, so viewers show each frame as soon as it arrives.
- Bindings must be regenerated when moving to newer libva or NVENC APIs, which is a deliberate step.
- CI compiles and unit-tests everything, but has no GPU. GPU checks are hand checks
  ([charter](../testing/charters/2026-10-25-gpu-h264.md)) plus `farsight-agent --check-h264`.

## Alternatives considered
- **`cros-codecs` over `cros-libva`:** no packed slice headers, so the stream is broken on Mesa radeonsi.
  It also needs libva-dev 2.16 or newer and clang at build time.
- **`nvidia-video-codec-sdk`:** links NVIDIA's libraries at build time, and depends on `cudarc` and the CUDA
  toolkit.
- **FFmpeg (`ffmpeg-next`):** a large C dependency with LGPL/GPL build variants and its own H.264
  licensing questions.
- **Vulkan Video encode:** one API would cover AMD and NVIDIA, but driver support for H.264 encode is still
  uneven (RADV has only recently enabled it). Deferred ([P-54](../decisions/provisional.md)).
- **No GPU encoding:** rejected by the user (U-15).
