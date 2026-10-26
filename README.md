# Immich Desktop

A small Rust / Tauri 2 wrapper for the existing Immich web UI, with native outbound file dragging. The server URL is configured in the local setup window. The app does not replace or proxy Immich's frontend.

**Status: experimental.** User testing confirms that the current build copies photos to Finder and delivers files to Chrome on Intel macOS. An earlier Chrome result also recorded a real Immich JPEG through `DataTransfer.files`, with its filename, JPEG MIME type and file size; no source-hash comparison was supplied. Slow uncached transfers can stall or time out in Chrome. **Firefox drops do not work in the current macOS build.** Windows and Apple Silicon have not been validated. See [validation status](docs/validation.md).

## Compile and run on macOS

The app targets macOS 11 or later, on Intel and Apple Silicon. Build it on a Mac with a supported Xcode command-line toolchain.

1. Install [Node.js 22 or newer](https://nodejs.org/en/download/) and the Xcode command-line tools. In Terminal:

   ```sh
   xcode-select --install
   ```

   Finish the system installer before continuing. If the tools are already installed, macOS will say so.

2. Install the stable Rust toolchain using [rustup](https://rustup.rs/):

   ```sh
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

   Follow the installer, then open a new Terminal window so Rust is on `PATH`. Check the tools:

   ```sh
   rustc --version
   cargo --version
   node --version
   npm --version
   ```

3. Download the source and install the pinned JavaScript dependencies:

   ```sh
   git clone https://github.com/pewu1/ImmichDesktop.git
   cd ImmichDesktop
   npm ci
   ```

   If you already have this checkout, open Terminal in its root directory and run `npm ci`.

4. Compile and launch the development app:

   ```sh
   npm run dev
   ```

   Wait for Rust compilation; the first run downloads dependencies and can take several minutes. The local setup window opens when compilation finishes. Keep this terminal open while developing. Use **Connect Immich** below to configure the server and API key.

5. To create and open a standalone development `.app`:

   ```sh
   npm run build -- --debug --bundles app
   open "src-tauri/target/debug/bundle/macos/Immich Desktop.app"
   ```

   Quit any other Immich Desktop instance before opening the new bundle. Only one instance may use the cache at a time.

6. Build the optimized release app and macOS disk image:

   ```sh
   npm run build
   ```

   The `.app` is under `src-tauri/target/release/bundle/macos/`; the `.dmg` is under `src-tauri/target/release/bundle/dmg/`. The build finishes without installing or launching the app. Open the DMG yourself, drag **Immich Desktop** into **Applications**, eject the disk image, then launch the installed app. Quit any existing instance before replacing it. Distribution signing/notarization is not configured.

   For an app-only build, use `npm run build -- --bundles app`. Debug and explicit architecture options remain available; omit `--bundles app` to produce the DMG for those builds too.

By default, the app builds for the Mac running the command. To select an architecture or create a universal app, install both Rust targets:

```sh
rustup target add x86_64-apple-darwin aarch64-apple-darwin
npm run build -- --target x86_64-apple-darwin --bundles app
npm run build -- --target aarch64-apple-darwin --bundles app
npm run build -- --target universal-apple-darwin --bundles app
```

Choose the relevant build command; you do not need to run all three. Explicit-target builds place their output under `src-tauri/target/<target>/release/bundle/`.

## Compile and run on Windows 11

Use a Windows machine for these steps. The following commands target Windows x64.

1. Install [Git for Windows](https://gitforwindows.org/), [Node.js 22 or newer](https://nodejs.org/en/download/) and [Rust via rustup](https://rustup.rs/).
2. Install [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/), selecting **Desktop development with C++**, the MSVC x64/x86 compiler and a Windows SDK. These compile both Rust's native dependencies and the C++ drag adapter.
3. Ensure the [Microsoft Edge WebView2 Evergreen Runtime](https://developer.microsoft.com/en-us/microsoft-edge/webview2/) is installed. Install Chrome for the upload-zone test.
4. Open a **new PowerShell window** after installing the tools. Select the MSVC Rust toolchain and check the installation:

   ```powershell
   rustup default stable-x86_64-pc-windows-msvc
   rustc --version
   cargo --version
   node --version
   npm.cmd --version
   ```

5. Download the source and install dependencies:

   ```powershell
   git clone https://github.com/pewu1/ImmichDesktop.git
   cd ImmichDesktop
   npm.cmd ci
   ```

6. Compile and start the development app:

   ```powershell
   npm.cmd run dev
   ```

   Wait for the first Rust build to finish, then configure the server and API key in the setup window. `npm.cmd` works in PowerShell without changing its script-execution policy.

7. To compile a standalone debug executable without an installer:

   ```powershell
   npm.cmd run build -- --debug --no-bundle
   .\src-tauri\target\debug\immich-desktop.exe
   ```

8. To build optimized Windows installers:

   ```powershell
   npm.cmd run build
   ```

   Find the generated installer files under `src-tauri\target\release\bundle\` (typically `nsis\` and `msi\`). Run the installer, then launch **Immich Desktop** from the Start menu. Signing is not configured. Quit an existing instance before launching a newly built executable.

For an explicit target, use `npm.cmd run build -- --target x86_64-pc-windows-msvc`; its outputs are under `src-tauri\target\x86_64-pc-windows-msvc\release\bundle\`. Windows compilation and these launch commands have not been exercised in this session.

Tauri serves the plain files in `ui/`; there is no separate frontend build server to start. Platform prerequisites are also described in the [Tauri setup guide](https://v2.tauri.app/start/prerequisites/).

## First: native drag lab

Run a development build (`npm run dev` or a debug bundle), then choose **Development → Native drag lab** from the application menu. The configuration window contains only connection settings; release builds do not show the development menu.

In another terminal, from this repository:

```sh
npm run test:drop-zone
```

Open <http://127.0.0.1:4173/tests/drop-zone.html> in **Google Chrome**, beside the Tauri window. Use **Normal — no artificial delay** and drag the test image into the zone. Expect one item in `DataTransfer.files`: `drag-test.png`, `image/png`, 39,564 bytes, with `bytesMatch: true` and `fixtureMetadataMatches: true`. Uncheck **Force a fresh original** and repeat to test retry caching.

The delay selector also offers a one-second deferred-transfer check and a five-second timeout stress test. The five-second case stalled/failed in the tested Chrome configuration; it is not a supported-flow demonstration. A desktop drop must create the actual PNG, never a text or HTML clipping. The user confirmed Finder and Chrome work after the native-provider fix. Firefox still fails and is currently unsupported on macOS; use Chrome for the upload-zone test.

The expected hash is shown in the lab and `fixtures/fixture.json`. The receiver computes SHA-256 from the received file. URL/HTML drops without files fail. Nothing is uploaded externally.

On Windows without a `python3` alias, serve the page with:

```powershell
py -3 -m http.server 4173 --bind 127.0.0.1 --directory .
```

## Connect Immich

1. Enter your server origin, for example `https://photos.example.com` or `https://photos.example.com:2283`. Subpaths are not supported. HTTP is allowed only for loopback development.
2. Create an Immich API key with **asset.read** and **asset.download** permissions and enter it in local setup. It is stored in macOS Keychain / Windows Credential Manager, separately from web login. It is never returned to the remote UI. Leave the key blank to reuse one saved for that origin.
3. Choose **Save and connect**, then log in normally. Subsequent launches open Immich directly without displaying setup. The app retains the persistent webview profile, including login cookies and browser storage, and reuses the API key from the OS credential store. It does not store your Immich password separately. A session can still expire or be revoked by the server; persistence needs a live restart test.
4. Drag a loaded photo or video thumbnail, or the media surface of an opened photo/video. Zoomed photos retain mouse panning; video controls remain usable. A single item's preview supplies the native drag image while Rust prepares the original in the background. Hover prepares only filename metadata and, when needed, a video poster. Release or Escape cancels preparation; it must not start a late native session.
5. Check several photos/videos in the timeline or an album, then drag a checked thumbnail to export the complete selection, including offscreen items. Keep the mouse held while the originals prepare; group dragging starts when every file is ready. Dragging an unchecked item exports only that item. Dragging never changes the checkboxes, including long holds, cancellation and errors. Duplicate filenames within a group receive numbered suffixes.

Full-selection integration reads Immich's existing selection manager from the deployed web modules. If an Immich version does not expose a compatible manager, checked-item dragging is skipped rather than exporting an incomplete selection. Single unchecked items and the viewer do not require that adapter. Groups are limited to 10,000 items per gesture.

Single-item Chrome drops on macOS use a synchronous promised-content compatibility path: they may pause on drag entry while the original downloads, and the OS/browser can give up before the app's 30-second wait limit. This path is capped at 256 MiB. Rust streams originals to disk; the native compatibility layer supplies a read-only file mapping. Groups use completed, retained files with one native file URL per item on macOS and a `CF_HDROP` file list on Windows. Groups do not use the single-item browser-content representation. Cross-application delivery still needs physical platform testing.

Use the same account for the API key and web login. The key may not access all shared assets visible in the session. Do not enter keys in chat or commit them to the repository.

Use **Immich Desktop → Reset configuration and login…** in the macOS menu bar or Windows window menu to change servers or erase local login data. Confirm **Reset and restart** to remove the saved server URL, API key, all app-webview cookies/storage and the cached original. This does not delete server photos or sign you out of other browsers. Reset waits for browser-data deletion to finish before restarting into setup, and refuses to interrupt an active file transfer. If interrupted, the next launch offers reset again instead of reopening the previous session. If a saved API key becomes unavailable, setup opens so you can enter it again.

**Immich Desktop → About Immich Desktop** shows the app icon, version, GPL-3.0-only license and GitHub link. External links open in your system browser. The supplied flower-monitor artwork was prepared with transparent outer edges and exported as PNG, macOS ICNS and Windows ICO; the editable master is `src-tauri/icons/app-icon-source.png`.

The injected drag handler adds no status toast or overlay to Immich. The development drag lab retains its diagnostic status display.

The main Immich window remembers its size, position, maximized state and fullscreen state when the app closes, and restores them on the next launch. A disconnected monitor falls back to placement on an available display. Window state is stored separately from connection settings, so resetting login/configuration preserves the window layout.

For a private development CA, trust it in the OS store used by both the webview and Rust's native TLS client. There is no TLS-validation bypass. Configure the final HTTPS origin; API redirects are refused. HTTPS OAuth pages may load but receive no native privileges. Popup/custom-scheme login flows need testing.

## Troubleshooting compilation and launch

- **`cargo`, `rustc`, `node` or `npm` is missing:** finish installation and open a new terminal. Run the version checks above before building.
- **macOS compiler/SDK missing:** finish `xcode-select --install`; `xcode-select -p` should report the installed developer-tools directory.
- **Windows `link.exe` or a C++ compiler is missing:** modify the Build Tools installation to include the C++ workload and Windows SDK. Retry from the Visual Studio **Developer PowerShell** or **x64 Native Tools Command Prompt** if the ordinary terminal cannot find the compiler.
- **Dependencies cannot download:** check connectivity and proxy settings. Both `npm ci` and the first Rust build need registry access.
- **Another instance is using the cache:** quit the previous Immich Desktop app and let active transfers finish before opening another build.
- **TLS or certificate error:** use the final HTTPS server origin and a certificate trusted by the OS. Install your development CA through the OS trust store; do not disable TLS verification.
- **An upload receives no files or a text clipping:** check [current validation and limitations](docs/validation.md). A successful compilation does not establish destination compatibility.

## Checks

```sh
npm test
cargo test --manifest-path src-tauri/Cargo.toml
cargo check --manifest-path src-tauri/Cargo.toml
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
```

On macOS, run `npm run test:native:macos` to check lazy browser formats, fixture bytes, filename metadata, absence of a text fallback, the original-file writing delegate and native reference cleanup. It uses a private pasteboard and does not synthesize pointer events. It also checks that hovering Chromium does not release the original before a later Finder drop, and that the production login-clear callback follows deletion of a cookie in an isolated temporary WebKit store. It does not clear your saved login. This is not a full Finder/Firefox/Chrome end-to-end test or a live Immich login/reset test.

The optional `--receiver` and `--baseline` native probes attempt a real `NSFilePromiseReceiver` transaction. Neither completed in the command-line environment, including Apple's unmodified baseline provider; they are diagnostic probes, not passing acceptance tests. Actual cross-application drops still need manual verification.

- [Architecture and security](docs/architecture.md)
- [Feasibility and primary sources](docs/research.md)
- [Manual acceptance procedure and limitations](docs/validation.md)

Thumbnail, viewer and multiple-selection dragging are implemented. HEIC/videos follow the original-byte path, but browser MIME recognition and destination acceptance need testing. Percentage progress, bulk prefetch and Linux are not implemented. Thumbnail detection requires Immich's `data-asset` attribute; full-selection dragging additionally depends on a compatible upstream selection manager.

Originals stream to disk; complete originals are never retained in RAM. The single-item retry cache keeps only the latest original and delays eviction until active readers finish. Completed outgoing groups are stored separately and retained across later drags and restarts, so a receiver's file URLs stay valid. Repeating the same group reuses those files. **Reset configuration and login…** removes retained groups as well as the retry cache; interrupted group staging is removed on startup. Receiver-owned copies are outside this app's cache.
