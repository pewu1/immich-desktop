# Feasibility and source review

Reviewed 2026-10-25. These are source-level findings, not cross-application acceptance results.

| Question | Evidence / decision |
| --- | --- |
| Remote Immich UI | Tauri 2 `WebviewWindowBuilder` accepts `WebviewUrl::External`. Windows uses WebView2, macOS uses WKWebView. No reverse proxy or replacement timeline is needed. |
| Injection | `initialization_script` runs at document start on desktop. Windows also injects into subframes, so the script must reject `window.top !== window` and check the configured origin. |
| Asset identity | Current Immich `Thumbnail.svelte` exposes `data-asset={asset.id}` on the thumbnail root. Use this explicit ID, never guess from a thumbnail URL. This is upstream DOM, not a public integration contract. |
| Real file drag | `drag` 2.1.1 accepts `DragItem::Files(Vec<PathBuf>)` with absolute existing paths and raw-window-handle 0.6, compatible with Tauri 2 handles. Run it on Tauri's main thread. |
| macOS | The library creates `NSURL` file pasteboard writers, `NSDraggingItem`s and a native `NSDraggingSession`. |
| Windows | The library uses OLE `DoDragDrop` and `CF_HDROP` with filesystem paths. This is separate platform code behind one Rust API. |
| Alternative | `tauri-plugin-dragout` 0.1.1 offers macOS file promises only. It does not cover Windows. Prefer existing, completely downloaded files for this MVP. |
| Chrome | A native file drop is the appropriate mechanism for ordinary `DataTransfer.files`. Only the receiver test with a real OS gesture can establish interoperability, filename, MIME and byte integrity. |
| Authentication | Immich API supports `x-api-key`; webview cookies do not automatically become reqwest cookies. Prefer an independently scoped API key in the OS credential store. |
| API | The current OpenAPI spec calls the original endpoint `downloadAsset`, `GET /api/assets/{id}/original`, permission `asset.download`. Metadata is `GET /api/assets/{id}`, permission `asset.read`. Do not request edited output. |
| Narrow IPC | Tauri supports runtime capabilities with explicit remote URL patterns and window labels. Declare application commands in `AppManifest`; custom application commands otherwise have different ACL defaults from plugin commands. Never grant filesystem/HTTP/shell plugins to the remote page. |

Immich source inspected at revision `256eb55dbe92ac77e02794442f9487e88ffed2b1` (main, 2026-10-24). A deployed server can differ.

Sources:

- [CrabNebula drag-rs](https://github.com/crabnebula-dev/drag-rs), including platform implementations and the Tauri plugin's main-thread scheduling pattern.
- [Published drag 2.1.1 source](https://docs.rs/crate/drag/2.1.1/source/).
- [dragout 0.1.1 README](https://docs.rs/crate/tauri-plugin-dragout/0.1.1/source/README.md).
- [Tauri webview builder](https://docs.rs/tauri/2.12.5/tauri/webview/struct.WebviewWindowBuilder.html).
- [Tauri capabilities](https://v2.tauri.app/security/capabilities/).
- [Tauri app command ACL manifest](https://docs.rs/tauri-build/latest/tauri_build/struct.AppManifest.html).
- [Immich thumbnail source](https://github.com/immich-app/immich/blob/256eb55dbe92ac77e02794442f9487e88ffed2b1/web/src/lib/components/assets/thumbnail/Thumbnail.svelte).
- [Immich OpenAPI specification](https://github.com/immich-app/immich/blob/256eb55dbe92ac77e02794442f9487e88ffed2b1/open-api/immich-openapi-specs.json).
- [Immich API documentation](https://api.immich.app/).

HTML5 `DataTransfer.items.add(File)`, image/URL drags and Chromium `DownloadURL` are not substitutes for this native file path implementation. No synthetic browser drop is an acceptance test for the native integration.

## Deferred-drag follow-up

The first `NSFilePromiseProvider`-only implementation was rejected in the user's real Chrome test. Chromium's current macOS `PopulateDropDataFromPasteboard` reads real file URLs or promised-content bytes; it does not fulfill a modern AppKit file promise on this path. Promised content is read on drag entry and capped at 256 MiB. Its content-disposition pasteboard type restores the original filename. The replacement adapter publishes a read-only disk-backed mapping for that compatibility representation, alongside the normal modern file promise. Windows instead uses asynchronous virtual-file streams. The original `drag` 2.1.1 adapter has been replaced because it only handled ready filesystem files for this use case.

- [Chromium macOS drop data extraction](https://github.com/chromium/chromium/blob/main/content/browser/web_contents/web_drag_dest_mac.mm)
- [Chromium drag-entry timing](https://github.com/chromium/chromium/blob/main/content/app_shim_remote_cocoa/web_contents_view_cocoa.mm)
- [Chromium file URL extraction](https://github.com/chromium/chromium/blob/main/ui/base/clipboard/clipboard_util_mac.mm)
- [Chromium filename metadata](https://github.com/chromium/chromium/blob/main/ui/base/clipboard/clipboard_constants_mac.mm)
- [Apple file-promise example](https://developer.apple.com/documentation/appkit/supporting-drag-and-drop-through-file-promises)
- [Windows asynchronous data-object interface](https://learn.microsoft.com/en-us/windows/win32/api/shldisp/nn-shldisp-idataobjectasynccapability)
