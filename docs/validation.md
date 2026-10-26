# Validation and limitations

## Evidence

| Check | Status |
| --- | --- |
| Earlier existing-file drag, Intel macOS 15.7.3 | User verified Chrome received the fixture with correct bytes/metadata |
| Modern `NSFilePromiseProvider`-only deferred drop | **Failed**: user reported Chrome rejected it or no file appeared |
| Chrome promised-content compatibility adapter | User reported a real Immich JPEG received through `DataTransfer.files` with filename, JPEG MIME and 2,730,861-byte size; source hash comparison not supplied |
| Five-second synthetic Chrome drop | Failed / stalled in user testing; it exceeds the observed synchronous browser-request tolerance |
| Earlier generic-item text fallback | Failed in user testing: Finder created a text file and Firefox received only `text/plain` |
| Current native provider: Finder / Chrome | User confirmed copying to Finder and dragging into Chrome work after the provider fix; no new fixture hash results supplied |
| Current native provider: Firefox on macOS | **Failed** in the user's retest; currently unsupported |
| macOS Rust + Objective-C build | `cargo check` passed; release app and Intel DMG built; installation remains manual |
| Rust policy/cache/preview/group tests | 20 passed: group concurrency, exact bytes, retained-group reuse, reset, cancellation, filename collisions, no-overwrite protection, partial cleanup and real loopback HTTP failures |
| JavaScript gesture/fixture/setup tests | 26 passed: long-hold selection regression, late clicks, Escape cancellation, complete offscreen selections, unchecked items, direct viewer media/background presses with `pointer-events-auto`, nested overlays/links/controls, zoom and editor controls |
| Off-screen native formats and delegate test | Passed: multi-file URL payload/bytes, exactly-once group completion, single-file browser payload/metadata, no text fallback, original-file delegate write and reference cleanup |
| Native macOS login-clear callback | Passed against an isolated nonpersistent WebKit store: session cookie present before reset and absent after completion; no user profile touched |
| Windows native C++ adapter | Implemented; **not compiled or tested here**, no Windows SDK/host |
| Apple Silicon build/drop | Not tested; Intel macOS host only |
| Live Immich replacement flow | Not tested against a supplied server/account |
| GitHub Actions build/release matrix | Configured for pushes to `main`: native Intel/Apple Silicon macOS DMGs and Windows x64 MSI/NSIS installers; first hosted run and release publication pending |
| Viewer and group drops into Finder / Chrome | Implemented; physical cross-application tests pending, including compatibility with the deployed Immich selection-manager export |

The user's real-photo Chrome result establishes that the compatibility path can deliver an ordinary JPEG `File`; it does not establish a source-hash match or arbitrary download latency. The latest physical retest confirms the revised AppKit provider works with Finder and Chrome. Firefox still fails. The five-second synthetic test and the earlier text-fallback results remain failures.

The command-line `--receiver` probe did not complete even with Apple's unmodified provider (`--baseline`). Consequently the default native check tests promise recognition plus the actual writing delegate, not a completed cross-process `NSFilePromiseReceiver` transaction. Use the physical Finder test for that boundary.

This environment has no browser-control tool and macOS denied automated pointer-event access. Real native drag tests require the user's physical gesture. The installed Chrome framework is version 155.0.8059.85; capture the actual receiver user agent when reporting a test.

## Native acceptance

1. Close previous app builds and launch a debug bundle. Open **Development → Native drag lab**. Serve the receiver as described in README and open it in a separate Chrome window.
2. Leave **Force a fresh original** checked and use **Normal — no artificial delay**. One gesture must produce one item in `DataTransfer.files`, named `drag-test.png`, MIME `image/png`, 39,564 bytes, with both match checks true. Repeat with the one-second mode. The five-second mode reproduces the known browser-timeout boundary and must not be counted as supported early-drop delivery.
3. Uncheck the fresh option and drag again. It must reuse the latest original from disk immediately. Repeat with Escape cancellation and retry. Drop the fixture onto the desktop: it must be a real PNG with the same bytes, never a text/HTML clipping. Repeat into Firefox and inspect `DataTransfer.files`.
4. Check the displayed thumbnail follows the pointer. Test fast/slow movement and release before preparation finishes; no late native drag may begin after release.
5. Record OS/browser versions and receiver JSON. Repeat on Windows 11 and both macOS architectures. On Windows run both applications at normal privilege.

Chrome on macOS requests promised-content bytes synchronously on drag entry, so it may pause while downloading. Its path caps content at 256 MiB and the adapter aborts its wait after 30 seconds. The app supplies a read-only disk mapping, while Chrome and the OS pasteboard copy the content independently. Do not describe that browser path as an asynchronous disk stream or promise support for arbitrary-size videos. macOS apps that fulfill the modern promise use background disk copying; Windows uses virtual-file streams.

## Immich and cache acceptance

1. Configure the origin and API key; **Save and connect** must open Immich and hide setup. Log in and browse timeline, albums, search and viewer/video screens. Restart: only Immich should appear, with the session retained subject to server expiration/revocation. Verify configuration contains no drag-test controls and real-photo dragging adds no status toast.
2. Drag an uncached JPEG with a known source hash once and drop before download finishes. Verify exact filename/size/hash. Repeat from cache. Repeat for PNG, HEIC, Unicode filenames, a large original within the browser limit and a small video.
3. Drag A, then B: A must immediately stop being reusable. Its generation disappears from disk once the last active reader/mapping finishes. Retrying B must reuse its download or completed file. A -> B -> A must not resurrect an obsolete generation.
4. Start a slow transfer, switch assets, close its source window and let the receiver finish. Transfer leases must prevent premature deletion. Closing the last app window must defer process exit only while an accepted native transfer remains active.
5. Test deleted/inaccessible assets, revoked keys, network interruption and insufficient disk space. Errors must not deliver partial files. Failed preparation must be retryable; superseded unneeded downloads must stop.
6. Restart after a completed drag and after an interrupted download. Only the latest valid original is restored, and orphaned/legacy cache generations are removed. OS deletion failures are retried on startup.
7. Inspect app memory during large downloads. There must be no whole-original heap buffer or RAM cache; macOS Chrome delivery may have temporary file-backed mapped pages and external browser/pasteboard copies.
8. Verify ordinary clicks, buttons, selection modifiers, context menus, scrolling, navigation, zoom, touch, video controls and uploads into Immich remain functional.
9. Open About from the native menu: verify the icon, GPL license (including bundled full text), GitHub link and external-browser launch. Check the app icon in the macOS Dock and Windows taskbar/window.
10. Choose **Reset configuration and login…** and confirm. It must refuse active transfers, otherwise remove local settings/key/cache and await browser-data deletion, restart to setup, and require login after reconnecting. Check IndexedDB/local storage/cookies as well as the visible login screen. Repeat after interrupting reset: startup must offer reset rather than the old session. Test WebView2 deletion errors and unavailable Keychain credentials. These live lifecycle checks remain pending on both target platforms; compilation and cache tests do not prove browser-session deletion.

## Viewer, group and installation acceptance

1. Drag an unchecked photo and video after holding the mouse stationary for more than 350 ms. Continue the gesture beyond 600 ms, then drop or cancel. Neither item may become checked; a subsequent ordinary click must still work. Repeat while other items are checked and after a preparation failure.
2. Open a photo/video from the timeline and from an album. For portrait and landscape photos, start separate drags directly on the image and on the surrounding black background, including in fullscreen. Drop into Finder and Chrome and compare filename, size and SHA-256 with the original; Chrome must receive one real file through `DataTransfer.files`. Repeat for video surfaces. Verify zoomed-photo panning, crop editing, interactive face/OCR overlays, playback controls, ordinary clicks and touch interactions remain usable. Physical verification of the direct-image press regression remains pending.
3. Check a mixed photo/video group, including items from months scrolled out of the DOM, and drag a checked thumbnail. Hold until preparation completes. Every selected original must arrive exactly once. Drag an unchecked thumbnail instead: only that item must arrive, and all existing checkboxes must stay unchanged.
4. Repeat with duplicate filenames, Unicode filenames, uncached originals and a previously prepared group. Release, press Escape, navigate, or close the source during preparation: no late native session or partial file may be delivered. Escape during preparation must not clear the checked selection.
5. Drag a different group and restart the app: previously exported file URLs must remain valid, completed groups must be reusable, and interrupted staging must disappear. Explicit reset must remove retained groups as well as the single-item retry cache. Group preparation/native sessions must block reset while active.
6. Run `npm run build` on macOS. Confirm both the app bundle and DMG exist, with no automatic installation or app launch. Open the DMG manually, copy the app into Applications, eject the image and launch the installed app. Repeat on Apple Silicon when a suitable host is available.
7. Resize and move the main Immich window, quit via the native menu and reopen: verify its previous size and position. Repeat using the close button, fullscreen, maximized and minimized windows. Reopening must display the main window and restore fullscreen/maximized state when saved. Disconnect its previous display before reopening and verify the window remains accessible. Setup and menu dialogs must not unexpectedly open or inherit Immich's layout; login/configuration reset must preserve the layout. These live window-manager checks remain manual on macOS and Windows.

Automated tests use modeled Immich DOM/module state and private native pasteboards. They do not establish live upstream module compatibility or a physical Finder/Chrome handoff. Group file URLs use a different path from the deferred single-item browser compatibility representation; previous single-item Firefox failures remain unchanged.

## Security and outstanding checks

- Different origins, ports, subdomains and frames must not inherit commands. Setup, shell, generic HTTP/filesystem commands remain denied remotely.
- Reject malformed UUIDs, path traversal, arbitrary URLs/paths and malformed/oversized PNG previews. These backend validation cases have automated coverage.
- Confirm API redirects never forward credentials. Backend tests cover auth/404 errors, traversal filenames, deduplication, truncation, readers, eviction, restart and cancellation.
- Private-CA trust, disk exhaustion, Windows COM lifetime/Chrome delivery, browser compatibility timeout behavior and adversarial webview ACL enforcement need manual platform checks.

Not implemented: percentage progress, popup/custom-scheme OAuth, subpath servers, signing/notarization or Linux support.
