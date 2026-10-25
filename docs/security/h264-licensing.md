# H.264 licensing review (M10)

The [M10 council](../council/2026-10-23-initial-roadmap.md#m10-gpu--h264) made a licensing review a
**blocking condition** for enabling H.264 in release builds. This is that review. It isn't legal advice. It
records how Farsight uses H.264 and what each part relies on, so that the user can decide (U-14) knowing
the facts.

## How Farsight touches H.264

| Part | What it does | Codec implementation | Who ships it |
|---|---|---|---|
| Viewer: decode | Shows Open H.264 rectangles | Cisco's prebuilt **OpenH264** | Cisco. The user downloads it with `--install-openh264`, and Farsight checks its SHA-256 |
| Agent: software encode | Encodes when no GPU encoder exists | Cisco's prebuilt **OpenH264** | Cisco, downloaded the same way |
| Agent: AMD/Intel encode | Encodes on the GPU | The GPU's encoder, driven through **VA-API** (Mesa or Intel's driver) | The OS distribution or driver vendor |
| Agent: NVIDIA encode | Encodes on the GPU | **NVENC** in the NVIDIA driver | NVIDIA |
| `farsight-hwenc` | Writes the SPS, PPS and slice headers, and converts colours | Farsight code (MIT); **no** prediction, transform or entropy coding | Farsight |

## Findings
1. **OpenH264 (U-12).** Cisco licenses H.264 for its binary and pays the royalties. This covers the binary
   only when the end user downloads it from Cisco.
   - Farsight never builds, bundles or redistributes the codec. It downloads Cisco's build on the user's
     request, from `ciscobinary.openh264.org`, and checks the build's SHA-256.
   - It shows Cisco's required notice ("OpenH264 Video Codec provided by Cisco Systems, Inc.") wherever
     H.264 is offered: in `--install-openh264`, in the logs, and in the README credits.
   - Cisco's binary licence (BSD 2-clause) allows this use.
2. **Hardware encoders.** The encoding (motion search, transforms, CABAC) is done by the GPU's fixed-function
   encoder, under the GPU vendor's and the driver distributor's arrangements.
   - This isn't uniform across distributions. Fedora and openSUSE build Mesa without H.264 VA-API encode for
     patent reasons. Ubuntu and Pop!_OS ship it enabled.
   - Farsight doesn't change which codecs the user's driver offers. If the driver has no H.264 encoder,
     Farsight falls back to OpenH264, or to lossless encodings.
3. **Farsight's own code** writes bitstream *syntax*: headers whose fields are dictated by the standard. It
   implements none of the coding tools that H.264 patents cover. The headers were written from ITU-T H.264
   §7.3, not taken from another implementation (clean-room, ADR-0011).
4. **Distribution.** Farsight's binaries contain no H.264 codec. The GPU paths load the system's own libraries
   at run time ([ADR-0021](../adr/0021-unsafe-confined-to-gpu-sys.md)), and OpenH264 is a separate download.

## Conclusion and what's left for the user
- No licensing blocker was found for **opt-in** H.264 that relies on Cisco's binary and on the user's own
  GPU driver, as built today.
- Enabling the `h264`, `vaapi`, `nvenc` and `dmabuf` features in release builds still waits for the user's
  go-ahead after the hand checks (U-14,
  [charter](../testing/charters/2026-10-25-gpu-h264.md)). When it happens, the README's credits must keep
  Cisco's notice.
- If Farsight is ever packaged for a distribution that excludes H.264, such as Fedora or Flathub with
  codec restrictions, that package should build without these features.
