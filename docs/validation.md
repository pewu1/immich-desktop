# Validation and limitations

## Evidence

| Check | Status |
| --- | --- |
| Earlier existing-file drag, Intel macOS 15.7.3 | User verified Chrome received the fixture with correct bytes/metadata |
| Modern `NSFilePromiseProvider`-only deferred drop | **Failed**: user reported Chrome rejected it or no file appeared |
| Chrome promised-content compatibility adapter | User reported a real Immich JPEG received through `DataTransfer.files` with filename, JPEG MIME and 2,730,861-byte size; source hash comparison not supplied |
| Five-second synthetic Chrome drop | Failed / stalled in user testing; it exceeds the observed synchronous browser-request tolerance |
| Finder desktop / Firefox with generic-item text fallback | Failed in user testing: a text file / only `text/plain`. Revised code writes a genuine AppKit provider, removes the text flavor and attaches compatibility bytes afterward; physical retest pending |
| macOS Rust + Objective-C build | `cargo check` passed; debug app bundle built |
| Rust policy/cache/preview tests | 12 passed, including real loopback HTTP failure tests |
| JavaScript gesture/fixture tests | 8 passed |
| Off-screen native formats and delegate test | Browser payload/metadata, no text fallback, no non-Chromium rendering, original-file delegate write and reference cleanup checked |
| Windows native C++ adapter | Implemented; **not compiled or tested here**, no Windows SDK/host |
| Apple Silicon build/drop | Not tested; Intel macOS host only |
| Live Immich replacement flow | Not tested against a supplied server/account |

The user's real-photo Chrome result establishes that the compatibility path can deliver an ordinary JPEG `File`; it does not establish a source-hash match or arbitrary download latency. The five-second synthetic test and the earlier text-fallback desktop/Firefox results are failures. The revised AppKit provider needs a physical desktop/Firefox retest.

The command-line `--receiver` probe did not complete even with Apple's unmodified provider (`--baseline`). Consequently the default native check tests promise recognition plus the actual writing delegate, not a completed cross-process `NSFilePromiseReceiver` transaction. Use the physical Finder test for that boundary.

This environment has no browser-control tool and macOS denied automated pointer-event access. Real native drag tests require the user's physical gesture. The installed Chrome framework is version 155.0.8059.85; capture the actual receiver user agent when reporting a test.

## Native acceptance

1. Close previous app builds and launch the new bundle. Serve the receiver as described in README and open it in a separate Chrome window.
2. Leave **Force a fresh original** checked and use **Normal — no artificial delay**. One gesture must produce one item in `DataTransfer.files`, named `drag-test.png`, MIME `image/png`, 39,564 bytes, with both match checks true. Repeat with the one-second mode. The five-second mode reproduces the known browser-timeout boundary and must not be counted as supported early-drop delivery.
3. Uncheck the fresh option and drag again. It must reuse the latest original from disk immediately. Repeat with Escape cancellation and retry. Drop the fixture onto the desktop: it must be a real PNG with the same bytes, never a text/HTML clipping. Repeat into Firefox and inspect `DataTransfer.files`.
4. Check the displayed thumbnail follows the pointer. Test fast/slow movement and release before preparation finishes; no late native drag may begin after release.
5. Record OS/browser versions and receiver JSON. Repeat on Windows 11 and both macOS architectures. On Windows run both applications at normal privilege.

Chrome on macOS requests promised-content bytes synchronously on drag entry, so it may pause while downloading. Its path caps content at 256 MiB and the adapter aborts its wait after 30 seconds. The app supplies a read-only disk mapping, while Chrome and the OS pasteboard copy the content independently. Do not describe that browser path as an asynchronous disk stream or promise support for arbitrary-size videos. macOS apps that fulfill the modern promise use background disk copying; Windows uses virtual-file streams.

## Immich and cache acceptance

1. Configure the origin and API key; log in and browse timeline, albums, search and viewer/video screens. Restart to verify session persistence.
2. Drag an uncached JPEG with a known source hash once and drop before download finishes. Verify exact filename/size/hash. Repeat from cache. Repeat for PNG, HEIC, Unicode filenames, a large original within the browser limit and a small video.
3. Drag A, then B: A must immediately stop being reusable. Its generation disappears from disk once the last active reader/mapping finishes. Retrying B must reuse its download or completed file. A -> B -> A must not resurrect an obsolete generation.
4. Start a slow transfer, switch assets, close its source window and let the receiver finish. Transfer leases must prevent premature deletion. Closing the last app window must defer process exit only while an accepted native transfer remains active.
5. Test deleted/inaccessible assets, revoked keys, network interruption and insufficient disk space. Errors must not deliver partial files. Failed preparation must be retryable; superseded unneeded downloads must stop.
6. Restart after a completed drag and after an interrupted download. Only the latest valid original is restored, and orphaned/legacy cache generations are removed. OS deletion failures are retried on startup.
7. Inspect app memory during large downloads. There must be no whole-original heap buffer or RAM cache; macOS Chrome delivery may have temporary file-backed mapped pages and external browser/pasteboard copies.
8. Verify ordinary clicks, buttons, selection modifiers, context menus, scrolling, navigation, zoom, touch, video controls and uploads into Immich remain functional.

## Security and outstanding checks

- Different origins, ports, subdomains and frames must not inherit commands. Setup, shell, generic HTTP/filesystem commands remain denied remotely.
- Reject malformed UUIDs, path traversal, arbitrary URLs/paths and malformed/oversized PNG previews. These backend validation cases have automated coverage.
- Confirm API redirects never forward credentials. Backend tests cover auth/404 errors, traversal filenames, deduplication, truncation, readers, eviction, restart and cancellation.
- Private-CA trust, disk exhaustion, Windows COM lifetime/Chrome delivery, browser compatibility timeout behavior and adversarial webview ACL enforcement need manual platform checks.

Not implemented: multi-file gestures, viewer-image dragging, percentage progress, popup/custom-scheme OAuth, subpath servers, signing/notarization or Linux support.
