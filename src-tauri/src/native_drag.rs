//! Deferred native delivery. Only preview pixels and paths cross the native boundary.
use crate::cache::{emit, Activity, Events, Transfer};
use base64::Engine;
use std::{
    ffi::{c_char, c_void, CStr, CString},
    io::Cursor,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tauri::WebviewWindow;
static DRAGGING: AtomicBool = AtomicBool::new(false);

pub struct Preview {
    png: Vec<u8>,
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}
impl Preview {
    pub fn parse(data: &str) -> Result<Self, String> {
        if data.len() > 400_000 {
            return Err("Thumbnail preview exceeds limit".into());
        }
        let base64 = data
            .strip_prefix("data:image/png;base64,")
            .ok_or("Expected a PNG thumbnail preview")?;
        Self::decode(
            base64::engine::general_purpose::STANDARD
                .decode(base64)
                .map_err(|_| "Invalid thumbnail encoding")?,
        )
    }
    pub fn fixture() -> Result<Self, String> {
        Self::decode(include_bytes!("../../fixtures/drag-test.png").to_vec())
    }
    fn decode(bytes: Vec<u8>) -> Result<Self, String> {
        let mut decoder = png::Decoder::new(Cursor::new(&bytes));
        decoder.set_limits(png::Limits { bytes: 1024 * 1024 });
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = decoder.read_info().map_err(|_| "Invalid thumbnail PNG")?;
        let (width, height) = (reader.info().width, reader.info().height);
        if width == 0 || height == 0 || width > 256 || height > 256 {
            return Err("Thumbnail must be at most 256 x 256".into());
        }
        let mut pixels = vec![
            0;
            reader
                .output_buffer_size()
                .ok_or("Thumbnail exceeds decode limit")?
        ];
        let info = reader
            .next_frame(&mut pixels)
            .map_err(|_| "Cannot decode thumbnail")?;
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        use png::ColorType::*;
        for pixel in pixels[..info.buffer_size()].chunks(info.color_type.samples()) {
            match info.color_type {
                Rgb => rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]),
                Rgba => rgba.extend_from_slice(pixel),
                Grayscale => rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], 255]),
                GrayscaleAlpha => rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]),
                _ => return Err("Unsupported thumbnail encoding".into()),
            }
        }
        drop(reader);
        Ok(Self {
            png: bytes,
            rgba,
            width,
            height,
        })
    }
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Callbacks {
    retain: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    release: unsafe extern "C" fn(*mut c_void),
    copy_file: unsafe extern "C" fn(*mut c_void, *const c_char) -> i32,
    read_file: unsafe extern "C" fn(*mut c_void, u64, *mut u8, usize, *mut usize) -> i32,
    file_size: unsafe extern "C" fn(*mut c_void, *mut u64) -> i32,
    ended: unsafe extern "C" fn(*mut c_void, i32),
    file_path: unsafe extern "C" fn(*mut c_void, *mut c_char, usize) -> i32,
    failed: unsafe extern "C" fn(*mut c_void, *const c_char),
    ready: unsafe extern "C" fn(*mut c_void) -> i32,
}
#[cfg(any(target_os = "macos", target_os = "windows"))]
extern "C" {
    fn immich_start_drag(
        window: *mut c_void,
        name: *const c_char,
        png: *const u8,
        png_len: usize,
        rgba: *const u8,
        width: u32,
        height: u32,
        context: *mut c_void,
        callbacks: Callbacks,
    ) -> i32;
    fn immich_mouse_down() -> i32;
    // Takes the context only on success; finished is called exactly once.
    fn immich_start_files_drag(
        window: *mut c_void,
        paths: *const *const c_char,
        count: usize,
        png: *const u8,
        png_len: usize,
        rgba: *const u8,
        width: u32,
        height: u32,
        context: *mut c_void,
        finished: unsafe extern "C" fn(*mut c_void, i32),
    ) -> i32;
}
struct FileSession {
    _activity: Activity,
    events: Events,
}
unsafe extern "C" fn files_finished(context: *mut c_void, dropped: i32) {
    let session = Box::from_raw(context.cast::<FileSession>());
    DRAGGING.store(false, Ordering::Release);
    emit(
        &session.events,
        if dropped != 0 { "dropped" } else { "cancelled" },
        "Group drag ended; outgoing files remain available until configuration reset",
    );
}
pub fn start_files(
    window: WebviewWindow,
    paths: Vec<PathBuf>,
    preview: Preview,
    events: Events,
    activity: Activity,
) -> Result<(), String> {
    if paths.is_empty()
        || paths
            .iter()
            .any(|path| !path.is_absolute() || !path.is_file())
    {
        return Err("Group originals are not ready".into());
    }
    let names = paths
        .iter()
        .map(|path| {
            path.to_str()
                .ok_or("Invalid outgoing path")
                .and_then(|path| CString::new(path).map_err(|_| "Invalid outgoing path"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    #[cfg(target_os = "macos")]
    let handle = window
        .ns_window()
        .map_err(|_| "Native window unavailable")?;
    #[cfg(target_os = "windows")]
    let handle = window.hwnd().map_err(|_| "Native window unavailable")?.0 as *mut c_void;
    if !mouse_down() {
        return Err("Mouse released before group drag started".into());
    }
    if DRAGGING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("Another native drag is active".into());
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let pointers = names.iter().map(|name| name.as_ptr()).collect::<Vec<_>>();
        emit(&events, "dragging", "Dragging selected originals");
        let context = Box::into_raw(Box::new(FileSession {
            _activity: activity,
            events,
        }))
        .cast();
        let ok = unsafe {
            immich_start_files_drag(
                handle,
                pointers.as_ptr(),
                pointers.len(),
                preview.png.as_ptr(),
                preview.png.len(),
                preview.rgba.as_ptr(),
                preview.width,
                preview.height,
                context,
                files_finished,
            )
        };
        if ok == 0 {
            unsafe {
                drop(Box::from_raw(context.cast::<FileSession>()));
            }
            DRAGGING.store(false, Ordering::Release);
            return Err("Native group drag could not start".into());
        }
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (window, names, preview, events, activity);
        DRAGGING.store(false, Ordering::Release);
        Err("macOS and Windows only".into())
    }
}
unsafe extern "C" fn retain(context: *mut c_void) -> *mut c_void {
    Arc::increment_strong_count(context as *const Transfer);
    context
}
unsafe extern "C" fn release(context: *mut c_void) {
    Arc::decrement_strong_count(context as *const Transfer);
}
fn callback(context: *mut c_void, action: impl FnOnce(&Transfer) -> Result<(), String>) -> i32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let transfer = unsafe { &*(context as *const Transfer) };
        match action(transfer) {
            Ok(()) => 1,
            Err(error) => {
                emit(&transfer.events, "error", error);
                0
            }
        }
    }))
    .unwrap_or(0)
}
unsafe extern "C" fn copy_file(context: *mut c_void, destination: *const c_char) -> i32 {
    callback(context, |transfer| {
        let path = CStr::from_ptr(destination)
            .to_str()
            .map_err(|_| "Invalid receiver path")?;
        transfer.copy_to(Path::new(path))?;
        emit(
            &transfer.events,
            "delivered",
            "Original delivered to the receiver",
        );
        Ok(())
    })
}
unsafe extern "C" fn read_file(
    context: *mut c_void,
    offset: u64,
    buffer: *mut u8,
    count: usize,
    read: *mut usize,
) -> i32 {
    callback(context, |transfer| {
        *read = transfer.read_at(offset, std::slice::from_raw_parts_mut(buffer, count))?;
        Ok(())
    })
}
unsafe extern "C" fn file_size(context: *mut c_void, size: *mut u64) -> i32 {
    callback(context, |transfer| {
        *size = transfer.wait()?.size;
        Ok(())
    })
}
unsafe extern "C" fn ended(context: *mut c_void, dropped: i32) {
    DRAGGING.store(false, Ordering::Release);
    let _ = callback(context, |transfer| {
        emit(
            &transfer.events,
            if dropped != 0 { "dropped" } else { "cancelled" },
            if dropped != 0 {
                "Dropped. Delivering the original when ready…"
            } else {
                "Drag cancelled; the latest original stays available for retry"
            },
        );
        Ok(())
    });
}
unsafe extern "C" fn file_path(context: *mut c_void, output: *mut c_char, capacity: usize) -> i32 {
    callback(context, |transfer| {
        let ready = transfer.wait()?;
        let path = ready
            .path
            .to_str()
            .ok_or("Invalid native cache path")?
            .as_bytes();
        if path.len() >= capacity {
            return Err("Native cache path exceeds limit".into());
        }
        std::ptr::copy_nonoverlapping(path.as_ptr(), output.cast(), path.len());
        *output.add(path.len()) = 0;
        Ok(())
    })
}
unsafe extern "C" fn failed(context: *mut c_void, message: *const c_char) {
    let _ = callback(context, |transfer| {
        emit(
            &transfer.events,
            "error",
            CStr::from_ptr(message).to_string_lossy().into_owned(),
        );
        Ok(())
    });
}
unsafe extern "C" fn ready(context: *mut c_void) -> i32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        (&*(context as *const Transfer)).readiness()
    }))
    .unwrap_or(-1)
}
pub fn mouse_down() -> bool {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    unsafe {
        immich_mouse_down() != 0
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        false
    }
}
pub fn start(
    window: WebviewWindow,
    transfer: Arc<Transfer>,
    preview: Preview,
) -> Result<(), String> {
    if !mouse_down() {
        return Err("Drag released before preparation finished".into());
    }
    if DRAGGING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("Another native drag is active".into());
    }
    #[cfg(target_os = "macos")]
    let handle = window.ns_window().map_err(|_| {
        DRAGGING.store(false, Ordering::Release);
        "Native window unavailable"
    })?;
    #[cfg(target_os = "windows")]
    let handle = window
        .hwnd()
        .map_err(|_| {
            DRAGGING.store(false, Ordering::Release);
            "Native window unavailable"
        })?
        .0 as *mut c_void;
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let name = CString::new(transfer.name()).map_err(|_| "Invalid original filename")?;
        emit(
            &transfer.events,
            "dragging",
            "Dragging original; download continues in the background…",
        );
        let context = Arc::into_raw(transfer) as *mut c_void;
        let ok = unsafe {
            immich_start_drag(
                handle,
                name.as_ptr(),
                preview.png.as_ptr(),
                preview.png.len(),
                preview.rgba.as_ptr(),
                preview.width,
                preview.height,
                context,
                Callbacks {
                    retain,
                    release,
                    copy_file,
                    read_file,
                    file_size,
                    ended,
                    file_path,
                    failed,
                    ready,
                },
            )
        };
        if ok == 0 {
            DRAGGING.store(false, Ordering::Release);
            return Err("Native deferred drag could not start".into());
        }
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (window, transfer, preview);
        DRAGGING.store(false, Ordering::Release);
        Err("macOS and Windows only".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_actual_png_previews_and_rejects_malformed_or_oversized_images() {
        let fixture = Preview::fixture().unwrap();
        assert_eq!(
            (fixture.width, fixture.height, fixture.rgba.len()),
            (160, 120, 160 * 120 * 4)
        );
        let data = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&fixture.png)
        );
        assert!(Preview::parse(&data).is_ok());
        assert!(Preview::parse("https://example.org/image.png").is_err());
        assert!(Preview::parse("data:image/png;base64,!!").is_err());
        assert!(Preview::parse(&"A".repeat(400_001)).is_err());
        assert!(Preview::decode(fixture.png[..30].to_vec()).is_err());
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, 257, 1);
        encoder.set_color(png::ColorType::Rgb);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&vec![0; 257 * 3]).unwrap();
        drop(writer);
        assert!(Preview::decode(bytes).is_err());
    }
}
