# Performance baselines (M8)

Measured 2026-10-24 on an AMD Ryzen 9 9950X3D2 (one core used), Rust 1.99.1, release profile, with
`cargo bench -p farsight-codec -p farsight-server`. Every release also attaches its own benchmark run
(`benchmarks-<version>.txt`, GitHub-hosted runner, slower and noisier) so regressions are visible over time.

## One 1920×1080 frame, cut into 256×256 pieces

"Desktop" is windows, title bars and lines of text on a flat background; "photo" is a smooth picture with noise
in every pixel. Times are per whole frame, single-threaded.

| Content | Encoding | Encode | Decode | Size |
|---|---|---|---|---|
| desktop | Raw | 7.3 ms | 7.5 ms | 8100 KiB |
| desktop | Hextile | 14.8 ms | 1.5 ms | 285 KiB |
| desktop | ZRLE | 8.5 ms | 1.8 ms | 7 KiB |
| desktop | Tight | 10.5 ms | 1.7 ms | 14 KiB |
| desktop | Tight JPEG (q8) | 17.2 ms | 5.0 ms | 509 KiB |
| photo | Raw | 6.7 ms | 7.5 ms | 8100 KiB |
| photo | Hextile | 43.2 ms | 7.7 ms | 8107 KiB |
| photo | ZRLE | 51.1 ms | 20.5 ms | 2748 KiB |
| photo | Tight | 41.8 ms | 16.0 ms | 2824 KiB |
| photo | Tight JPEG (q8) | 19.2 ms | 7.2 ms | 752 KiB |

What the agent does with this (automatic selection, ENC-9): flat and text areas go lossless (Tight or ZRLE, 1–2 %
of raw), photo-like areas go to JPEG when the viewer asked for a quality level (3.7× smaller and 2× faster than
lossless Tight here), and solid areas are a few bytes.

## Agent → viewer, in memory

The whole Farsight path without network or screen capture: a change on the desktop until the viewer has decoded it
(`agent-to-viewer` in `crates/farsight-server/benches/pipeline.rs`).

| Change | Time | Frames per second (CPU bound) |
|---|---|---|
| 16×16 (typing a character), 1080p desktop | 0.43 ms* | — |
| Whole 1080p desktop (switching windows) | 6.9 ms | ~145 |

\* Includes copying the 8 MiB frame into the desktop model each iteration, so the protocol's own share is smaller.

## Flow control

With continuous updates the agent keeps at most two updates unacknowledged (fences, M8), so a slow viewer or link
gets fewer, fresher frames instead of a growing backlog (`continuous_updates_flow_without_requests_within_the_congestion_window`).
