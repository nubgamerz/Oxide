//! Plugin window management
//!
//! This module provides platform-specific window creation and management
//! for VST3 plugin GUIs.

use crate::error::{Error, Result};
use crate::plugin::Plugin;
use std::sync::{Arc, Mutex};

#[cfg(target_os = "macos")]
use objc2::{rc::Retained, MainThreadMarker, MainThreadOnly};
#[cfg(target_os = "macos")]
use objc2_app_kit::{NSApplication, NSBackingStoreType, NSView, NSWindow, NSWindowStyleMask};
#[cfg(target_os = "macos")]
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

#[cfg(target_os = "windows")]
use winapi::{
    shared::minwindef::{LPARAM, LRESULT, UINT, WPARAM},
    shared::windef::{HWND, RECT},
    um::libloaderapi::GetModuleHandleW,
    um::winuser::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetWindowLongW, LoadCursorW,
        RegisterClassExW, SetWindowLongW, SetWindowPos, ShowWindow, UpdateWindow, CS_HREDRAW,
        CS_VREDRAW, CW_USEDEFAULT, GWL_STYLE, IDC_ARROW, SIZE_MINIMIZED, SWP_FRAMECHANGED,
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_SHOW, WM_CLOSE, WM_DPICHANGED,
        WM_ENTERSIZEMOVE, WM_EXITSIZEMOVE, WM_SIZE,
        WNDCLASSEXW, WS_MAXIMIZEBOX, WS_OVERLAPPEDWINDOW, WS_THICKFRAME,
    },
};

#[cfg(any(test, target_os = "windows"))]
fn dpi_scale_factor(dpi: u32) -> Option<f32> {
    (dpi > 0).then_some(dpi as f32 / 96.0)
}

/// Editor windows whose window procedure has seen `WM_CLOSE`, keyed by `HWND` (as `usize`,
/// since `HWND` is a raw pointer and not `Send`).
///
/// `DefWindowProcW` answers `WM_CLOSE` by destroying the window, which would leave the plugin's
/// `IPlugView` attached to a freed `HWND`. [`plugin_window_proc`] records the request here
/// instead, so the host can tear the editor down in the right order.
#[cfg(target_os = "windows")]
fn close_requests() -> &'static Mutex<std::collections::HashSet<usize>> {
    static REQUESTS: std::sync::OnceLock<Mutex<std::collections::HashSet<usize>>> =
        std::sync::OnceLock::new();
    REQUESTS.get_or_init(Mutex::default)
}

/// Latest pending DPI for each editor window. The window procedure cannot call into the plugin:
/// `setContentScaleFactor` may synchronously enter arbitrary plugin/UI code, while callers often
/// hold the plugin mutex when Windows dispatches a message. The procedure records only the
/// newest DPI and [`PluginWindow::service_platform_events`] applies it after the callback returns.
#[cfg(target_os = "windows")]
fn dpi_changes() -> &'static Mutex<std::collections::HashMap<usize, u32>> {
    static CHANGES: std::sync::OnceLock<Mutex<std::collections::HashMap<usize, u32>>> =
        std::sync::OnceLock::new();
    CHANGES.get_or_init(Mutex::default)
}

/// Latest client size the user dragged each editor window to (vendored addition). As with DPI,
/// the window procedure only records it: [`PluginWindow::service_platform_events`] hands it to
/// the plug-in's view (`checkSizeConstraint`, `onSize`) after the callback returns, and snaps
/// the window to the size the view accepts.
#[cfg(target_os = "windows")]
fn size_changes() -> &'static Mutex<std::collections::HashMap<usize, (i32, i32)>> {
    static CHANGES: std::sync::OnceLock<Mutex<std::collections::HashMap<usize, (i32, i32)>>> =
        std::sync::OnceLock::new();
    CHANGES.get_or_init(Mutex::default)
}

/// Editor windows whose edge the user is dragging right now (between `WM_ENTERSIZEMOVE` and
/// `WM_EXITSIZEMOVE`): their editor follows the drag, but the window snaps to the size the
/// editor took only when the drag ends, so an editor with its own proportions doesn't fight
/// the mouse (vendored addition).
#[cfg(target_os = "windows")]
fn dragging() -> &'static Mutex<std::collections::HashSet<usize>> {
    static DRAGGING: std::sync::OnceLock<Mutex<std::collections::HashSet<usize>>> =
        std::sync::OnceLock::new();
    DRAGGING.get_or_init(Mutex::default)
}

#[cfg(target_os = "windows")]
fn dpi_from_wparam(wparam: WPARAM) -> Option<u32> {
    // WM_DPICHANGED packs the new X DPI into LOWORD and Y DPI into HIWORD. VST3 has one content
    // scale, and Windows normally reports both equally, so use X as Microsoft recommends.
    let dpi = (wparam & 0xffff) as u32;
    (dpi > 0).then_some(dpi)
}

/// Window procedure for plugin editor windows: record control-plane work and defer plugin calls
/// until after Windows returns from the callback.
///
/// # Safety
///
/// Called by the OS with the arguments of a window procedure; all it does with them is forward
/// them to `DefWindowProcW`.
#[cfg(target_os = "windows")]
unsafe extern "system" fn plugin_window_proc(
    hwnd: HWND,
    msg: UINT,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_CLOSE {
        if let Ok(mut requests) = close_requests().lock() {
            requests.insert(hwnd as usize);
        }
        return 0;
    }
    if msg == WM_DPICHANGED {
        // The suggested rectangle is valid only for this callback and must be applied
        // synchronously. Do that before taking any host mutex: SetWindowPos may dispatch more
        // window messages reentrantly.
        if let Some(suggested) = (lparam as *const RECT).as_ref() {
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                suggested.left,
                suggested.top,
                suggested.right - suggested.left,
                suggested.bottom - suggested.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        if let Some(dpi) = dpi_from_wparam(wparam) {
            if let Ok(mut changes) = dpi_changes().lock() {
                changes.insert(hwnd as usize, dpi);
            }
        }
        return 0;
    }
    if msg == WM_ENTERSIZEMOVE {
        if let Ok(mut d) = dragging().lock() {
            d.insert(hwnd as usize);
        }
    }
    if msg == WM_EXITSIZEMOVE {
        if let Ok(mut d) = dragging().lock() {
            d.remove(&(hwnd as usize));
        }
        // The drag's last size, once more, now that the window may snap to the editor.
        let mut rect: RECT = std::mem::zeroed();
        if GetClientRect(hwnd, &mut rect) != 0 && rect.right > 0 && rect.bottom > 0 {
            if let Ok(mut changes) = size_changes().lock() {
                changes.insert(hwnd as usize, (rect.right, rect.bottom));
            }
        }
    }
    if msg == WM_SIZE && wparam != SIZE_MINIMIZED as WPARAM {
        // The new client size: LOWORD width, HIWORD height.
        let (width, height) = ((lparam & 0xffff) as i32, ((lparam >> 16) & 0xffff) as i32);
        if width > 0 && height > 0 {
            if let Ok(mut changes) = size_changes().lock() {
                changes.insert(hwnd as usize, (width, height));
            }
        }
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// An X11 window (connection + window id) backing a plugin editor on Linux.
///
/// Ported from the khremeviuc1004 fork's XCB implementation.
#[cfg(target_os = "linux")]
struct XcbWindowState {
    connection: xcb::Connection,
    window: xcb::x::Window,
}

/// A plugin window that manages the native window and plugin editor lifecycle
pub struct PluginWindow {
    plugin: Arc<Mutex<Plugin>>,
    #[cfg(target_os = "macos")]
    native_window: Option<Retained<NSWindow>>,
    /// The view the plugin attached its editor into. Kept so a plugin-initiated resize can
    /// grow the container along with the window.
    #[cfg(target_os = "macos")]
    container_view: Option<Retained<NSView>>,
    #[cfg(target_os = "windows")]
    native_window: Option<HWND>,
    /// The attached editor's view, to resize it when the user drags the window without the
    /// plug-in's lock (vendored addition; dropped before the view is detached).
    #[cfg(target_os = "windows")]
    view: Option<crate::plugin::ViewHandle>,
    /// The host frame's slot for this window's handle (vendored addition): set while the
    /// editor is attached, so `resizeView` resizes the window at once.
    #[cfg(target_os = "windows")]
    frame_window: Option<Arc<std::sync::atomic::AtomicUsize>>,
    /// The host frame's slot for the size the editor asks for (vendored addition): read each
    /// frame without the plug-in's lock.
    #[cfg(target_os = "windows")]
    resize_requests: Option<Arc<Mutex<Option<(i32, i32)>>>>,
    #[cfg(target_os = "linux")]
    native_window: Option<XcbWindowState>,
    #[cfg(target_os = "android")]
    native_window: Option<()>,
}

impl PluginWindow {
    /// Create a new plugin window for the given plugin
    pub fn new(plugin: Arc<Mutex<Plugin>>) -> Self {
        Self {
            plugin,
            #[cfg(any(
                target_os = "macos",
                target_os = "windows",
                target_os = "linux",
                target_os = "android"
            ))]
            native_window: None,
            #[cfg(target_os = "windows")]
            view: None,
            #[cfg(target_os = "windows")]
            frame_window: None,
            #[cfg(target_os = "windows")]
            resize_requests: None,
            #[cfg(target_os = "macos")]
            container_view: None,
        }
    }

    /// Open the plugin window
    pub fn open(&mut self) -> Result<()> {
        // Check if plugin has editor
        let has_editor = self
            .plugin
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .has_editor();
        if !has_editor {
            return Err(Error::Other(
                "Plugin does not have a GUI editor".to_string(),
            ));
        }

        // Close existing window if any. Keyed on the handle rather than `is_open()`, which also
        // reports `false` for a window the user already dismissed — that one still needs closing.
        if self.native_window.is_some() {
            self.close();
        }

        // Get plugin info for window title
        let plugin_info = self
            .plugin
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .info()
            .clone();

        // Windows: make and attach the view without holding the plug-in's lock, so the audio
        // thread keeps processing (a vendored change; see VENDORED.md). Plug-ins that can't
        // (process isolation) take the path below.
        #[cfg(target_os = "windows")]
        {
            let setup = self
                .plugin
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .begin_editor();
            if let Ok(setup) = setup {
                return self.open_unlocked(setup, &plugin_info.name);
            }
        }

        // Try to get editor size
        let (width, height) = self
            .plugin
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get_editor_size()
            .unwrap_or((800, 600));

        // Create native window
        #[cfg(target_os = "macos")]
        {
            // AppKit objects must be created on the main thread.
            let mtm = MainThreadMarker::new().ok_or_else(|| {
                Error::Other("plugin editor window must be opened on the main thread".to_string())
            })?;

            let frame = NSRect::new(
                NSPoint::new(100.0, 100.0),
                NSSize::new(width as f64, height as f64),
            );
            let style = NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Miniaturizable;

            // SAFETY: standard AppKit window/view construction on the main thread.
            let window = unsafe {
                NSWindow::initWithContentRect_styleMask_backing_defer(
                    NSWindow::alloc(mtm),
                    frame,
                    style,
                    NSBackingStoreType::Buffered,
                    false,
                )
            };

            // Programmatic NSWindows default to `releasedWhenClosed = YES`; closing one would
            // then release it while our `Retained<NSWindow>` also releases on drop — a
            // double-free that crashes on close. We own the lifetime, so opt out.
            // SAFETY: standard AppKit setter on the main thread.
            unsafe { window.setReleasedWhenClosed(false) };

            let title = NSString::from_str(&format!("{} - VST3", plugin_info.name));
            window.setTitle(&title);

            // A container view, sized to the editor, that the plugin attaches its view into.
            let container_frame = NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(width as f64, height as f64),
            );
            let container_view = NSView::initWithFrame(NSView::alloc(mtm), container_frame);
            if let Some(content_view) = window.contentView() {
                content_view.addSubview(&container_view);
            }

            // Hand the plugin the container NSView to embed its editor in.
            // SAFETY: `container_view` is a live NSView this `PluginWindow` keeps alive (via
            // the retained window it was added to) for as long as the editor is attached —
            // `close()` detaches the editor before dropping the window.
            let window_handle = unsafe {
                crate::plugin::WindowHandle::from_nsview(
                    Retained::as_ptr(&container_view) as *mut std::ffi::c_void
                )
            };
            self.plugin
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .open_editor(window_handle)?;

            // Match the window to the editor size, then show and center it.
            window.setContentSize(container_frame.size);
            window.makeKeyAndOrderFront(None);
            window.center();

            self.native_window = Some(window);
            self.container_view = Some(container_view);
        }

        #[cfg(target_os = "windows")]
        {
            unsafe {
                use std::mem;
                use std::ptr;

                // Register window class if not already registered
                let class_name = "VST3PluginWindow\0".encode_utf16().collect::<Vec<u16>>();
                let mut wc: WNDCLASSEXW = mem::zeroed();
                wc.cbSize = mem::size_of::<WNDCLASSEXW>() as UINT;
                wc.style = CS_HREDRAW | CS_VREDRAW;
                wc.lpfnWndProc = Some(plugin_window_proc);
                wc.hInstance = GetModuleHandleW(ptr::null());
                wc.hCursor = LoadCursorW(ptr::null_mut(), IDC_ARROW);
                wc.lpszClassName = class_name.as_ptr();

                // Try to register, ignore if already registered
                RegisterClassExW(&wc);

                // Create window
                let window_title = format!("{} - VST3\0", plugin_info.name);
                let window_name = window_title.encode_utf16().collect::<Vec<u16>>();

                // Calculate window size including borders
                let mut rect = RECT {
                    left: 0,
                    top: 0,
                    right: width,
                    bottom: height,
                };

                winapi::um::winuser::AdjustWindowRectEx(
                    &mut rect,
                    WS_OVERLAPPEDWINDOW,
                    0, // No menu
                    0, // No extended style
                );

                let window_width = rect.right - rect.left;
                let window_height = rect.bottom - rect.top;

                let hwnd = CreateWindowExW(
                    0,
                    class_name.as_ptr(),
                    window_name.as_ptr(),
                    WS_OVERLAPPEDWINDOW,
                    CW_USEDEFAULT,
                    CW_USEDEFAULT,
                    window_width,
                    window_height,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    GetModuleHandleW(ptr::null()),
                    ptr::null_mut(),
                );

                if hwnd.is_null() {
                    return Err(Error::Other("Failed to create native window".to_string()));
                }

                // Try to open plugin editor.
                // SAFETY: `hwnd` was just created above and null-checked; it is destroyed only
                // after the editor is detached (in `close()`, or the error arm below).
                let window_handle =
                    crate::plugin::WindowHandle::from_hwnd(hwnd as *mut std::ffi::c_void);
                let mut plugin = self.plugin.lock().unwrap_or_else(|p| p.into_inner());
                let dpi = winapi::um::winuser::GetDpiForWindow(hwnd);
                if let Some(scale_factor) = dpi_scale_factor(dpi) {
                    if let Err(error) = plugin.set_editor_scale_factor(scale_factor) {
                        drop(plugin);
                        DestroyWindow(hwnd);
                        return Err(error);
                    }
                }
                match plugin.open_editor(window_handle) {
                    Ok(()) => {
                        drop(plugin);
                        ShowWindow(hwnd, SW_SHOW);
                        UpdateWindow(hwnd);
                        self.native_window = Some(hwnd);
                    }
                    Err(e) => {
                        drop(plugin);
                        DestroyWindow(hwnd);
                        return Err(e);
                    }
                }
            }
        }

        #[cfg(target_os = "linux")]
        {
            use xcb::Xid;

            // Create an X11 window via XCB and embed the plugin editor into it using the
            // VST3 X11EmbedWindowID platform type (handled in plugin_impl::open_editor).
            let (connection, screen_number) = xcb::Connection::connect(None)
                .map_err(|e| Error::Other(format!("Failed to connect to X server: {e}")))?;
            let setup = connection.get_setup();
            let screen = setup
                .roots()
                .nth(screen_number as usize)
                .ok_or_else(|| Error::Other("No X11 screen found".to_string()))?;
            let window = connection.generate_id();

            connection
                .send_and_check_request(&xcb::x::CreateWindow {
                    depth: xcb::x::COPY_FROM_PARENT as u8,
                    wid: window,
                    parent: screen.root(),
                    x: 0,
                    y: 0,
                    width: width as u16,
                    height: height as u16,
                    border_width: 0,
                    class: xcb::x::WindowClass::InputOutput,
                    visual: screen.root_visual(),
                    value_list: &[
                        xcb::x::Cw::BackPixel(screen.white_pixel()),
                        xcb::x::Cw::EventMask(
                            xcb::x::EventMask::EXPOSURE | xcb::x::EventMask::KEY_PRESS,
                        ),
                    ],
                })
                .map_err(|e| Error::Other(format!("Failed to create X11 window: {e}")))?;

            // Window title.
            let title = format!("{} - VST3", plugin_info.name);
            connection.send_request(&xcb::x::ChangeProperty {
                mode: xcb::x::PropMode::Replace,
                window,
                property: xcb::x::ATOM_WM_NAME,
                r#type: xcb::x::ATOM_STRING,
                data: title.as_bytes(),
            });

            // Show the window, then attach the plugin editor to its X11 id.
            connection.send_request(&xcb::x::MapWindow { window });
            let _ = connection.flush();

            let handle = crate::plugin::WindowHandle::from_x11(window.resource_id());
            self.plugin
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .open_editor(handle)?;

            self.native_window = Some(XcbWindowState { connection, window });
        }

        #[cfg(target_os = "android")]
        {
            return Err(Error::Other(
                "PluginWindow::open() is not supported on Android".to_string(),
            ));
        }

        Ok(())
    }

    /// Open the editor from `setup`: the view is created, sized and attached with the plug-in's
    /// lock released, and only handed back under it. Its window takes the view's own size, so
    /// no throwaway view is made to ask for it.
    #[cfg(target_os = "windows")]
    fn open_unlocked(&mut self, setup: crate::plugin::EditorSetup, name: &str) -> Result<()> {
        let setup_window_slot = setup.window_slot();
        let resize_requests = setup.resize_requests();
        let view = setup.create()?;
        let (width, height) = view.size();
        unsafe {
            use std::mem;
            use std::ptr;

            let class_name = "VST3PluginWindow\0".encode_utf16().collect::<Vec<u16>>();
            let mut wc: WNDCLASSEXW = mem::zeroed();
            wc.cbSize = mem::size_of::<WNDCLASSEXW>() as UINT;
            wc.style = CS_HREDRAW | CS_VREDRAW;
            wc.lpfnWndProc = Some(plugin_window_proc);
            wc.hInstance = GetModuleHandleW(ptr::null());
            wc.hCursor = LoadCursorW(ptr::null_mut(), IDC_ARROW);
            wc.lpszClassName = class_name.as_ptr();
            // Registering again fails harmlessly.
            RegisterClassExW(&wc);

            let window_title = format!("{name} - VST3\0");
            let window_name = window_title.encode_utf16().collect::<Vec<u16>>();
            let mut rect = RECT {
                left: 0,
                top: 0,
                right: width,
                bottom: height,
            };
            winapi::um::winuser::AdjustWindowRectEx(&mut rect, WS_OVERLAPPEDWINDOW, 0, 0);
            let hwnd = CreateWindowExW(
                0,
                class_name.as_ptr(),
                window_name.as_ptr(),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                rect.right - rect.left,
                rect.bottom - rect.top,
                ptr::null_mut(),
                ptr::null_mut(),
                GetModuleHandleW(ptr::null()),
                ptr::null_mut(),
            );
            if hwnd.is_null() {
                view.discard();
                return Err(Error::Other("Failed to create native window".to_string()));
            }
            // SAFETY: `hwnd` was just created and null-checked; it is destroyed only after the
            // editor is detached (in `close()`, or the error arms below).
            let window_handle =
                crate::plugin::WindowHandle::from_hwnd(hwnd as *mut std::ffi::c_void);
            // An editor may resize itself while attaching: the frame resizes this window then.
            let slot = setup_window_slot.clone();
            slot.store(hwnd as usize, std::sync::atomic::Ordering::Release);
            let scale = dpi_scale_factor(winapi::um::winuser::GetDpiForWindow(hwnd));
            let attached = match view.attach(window_handle, scale) {
                Ok(attached) => attached,
                Err(e) => {
                    slot.store(0, std::sync::atomic::Ordering::Release);
                    DestroyWindow(hwnd);
                    return Err(e);
                }
            };
            // The size it gives once attached, if it gave none (or another) before.
            let fitted = attached.size().filter(|&s| s != (width, height));
            let handle = attached.handle();
            // A fixed-size editor gets a window that can't be resized either, or dragging its
            // edge would only open black space beside the editor.
            if !attached.can_resize() {
                let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
                SetWindowLongW(
                    hwnd,
                    GWL_STYLE,
                    (style & !(WS_THICKFRAME | WS_MAXIMIZEBOX)) as i32,
                );
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED | SWP_NOSIZE,
                );
            }
            let adopted = self
                .plugin
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .adopt_editor(attached);
            if let Err(e) = adopted {
                slot.store(0, std::sync::atomic::Ordering::Release);
                DestroyWindow(hwnd);
                return Err(e);
            }
            self.frame_window = Some(slot);
            self.resize_requests = Some(resize_requests);
            self.native_window = Some(hwnd);
            self.view = Some(handle);
            // Sized to the editor in the window's own style (a fixed one has a thinner frame).
            let (w, h) = fitted.unwrap_or((width, height));
            self.resize_native_window(w, h);
            // What creating and sizing the window recorded isn't the user's.
            if let Ok(mut changes) = size_changes().lock() {
                changes.remove(&(hwnd as usize));
            }
            ShowWindow(hwnd, SW_SHOW);
            UpdateWindow(hwnd);
        }
        Ok(())
    }

    /// Pump the editor's window-level traffic that has to cross into the plugin.
    ///
    /// **Call this from your UI event loop, once per frame, while the window is open.** Two
    /// things depend on it:
    ///
    /// - **Plugin-initiated resizes.** A resizable editor (VSTGUI zoom, a "big view" toggle)
    ///   asks the host to resize through `IPlugFrame::resizeView`. This applies the request to
    ///   the native window, so the editor is not clipped by a window that never grew.
    /// - **Windows DPI changes.** `WM_DPICHANGED` applies its suggested native rectangle
    ///   immediately in the window procedure and queues the new content scale for here; making
    ///   the COM call outside the window procedure prevents reentrant deadlocks on the plugin
    ///   mutex.
    ///
    /// Deliberately non-blocking: an editor opened without the plug-in's lock (Windows) is
    /// serviced without it; otherwise, if the plugin mutex is held elsewhere (the audio callback
    /// holds it per block) this returns without doing anything, and the pending work is picked
    /// up on the next call.
    ///
    /// [`Self::is_open`] and [`Self::closed_by_user`] do *not* run this — they stay lock-free
    /// so they are safe to call while holding the plugin lock.
    pub fn service_platform_events(&self) -> Result<()> {
        if self.native_window.is_none() {
            return Ok(());
        }
        // The user dragged the window: the editor takes that size if it can (or the nearest it
        // allows), and the window snaps to whatever it took; a fixed one keeps its size. Through
        // the window's own reference to the view, so the audio thread never waits for it.
        if let Some((width, height)) = self.take_pending_size() {
            let took = self.view_resize(width, height);
            if let Some(size) = took.filter(|&size| size != (width, height)) {
                if !self.being_dragged() {
                    self.resize_native_window(size.0, size.1);
                }
            }
        }
        // An editor opened without the lock is serviced without it too (vendored change): its
        // resize requests come from the host frame's slot and a DPI change goes to the view
        // through the window's own reference. Taking the plug-in's lock here every frame, however
        // briefly, met the audio thread's `try_lock` now and then, and that block went silent.
        #[cfg(target_os = "windows")]
        if let (Some(requests), Some(view)) = (&self.resize_requests, &self.view) {
            let scale_result = self
                .take_pending_scale_factor()
                .map(|factor| view.set_scale_factor(factor));
            let resize = requests.lock().ok().and_then(|mut r| r.take());
            if let Some((width, height)) = resize {
                self.resize_native_window(width, height);
            }
            return match scale_result {
                Some(Err(error)) => Err(error),
                _ => Ok(()),
            };
        }
        let Some(mut plugin) = self.try_lock_plugin() else {
            return Ok(());
        };

        let scale_result = self
            .take_pending_scale_factor()
            .map(|factor| plugin.set_editor_scale_factor(factor));
        let resize = plugin.take_editor_resize_request();

        // Released before touching the native window: on Windows, resizing it may synchronously
        // dispatch window messages back into host code that wants this same lock.
        drop(plugin);

        if let Some((width, height)) = resize {
            self.resize_native_window(width, height);
        }
        match scale_result {
            Some(Err(error)) => Err(error),
            _ => Ok(()),
        }
    }

    /// Take the plugin lock without blocking, recovering a lock poisoned by an unrelated panic
    /// (a poisoned mutex is permanent, and treating it as failure would stop servicing the
    /// editor for the rest of the session).
    fn try_lock_plugin(&self) -> Option<std::sync::MutexGuard<'_, Plugin>> {
        match self.plugin.try_lock() {
            Ok(guard) => Some(guard),
            Err(std::sync::TryLockError::Poisoned(poison)) => Some(poison.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => None,
        }
    }

    /// The newest DPI the window procedure recorded for this window, as a VST3 content scale.
    #[cfg(target_os = "windows")]
    fn take_pending_scale_factor(&self) -> Option<f32> {
        let hwnd = self.native_window?;
        dpi_changes()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(&(hwnd as usize))
            .and_then(dpi_scale_factor)
    }

    /// Only Windows reports DPI changes through the window procedure.
    #[cfg(not(target_os = "windows"))]
    fn take_pending_scale_factor(&self) -> Option<f32> {
        None
    }

    /// Fit the attached editor to a size the user dragged the window to: the size it took.
    #[cfg(target_os = "windows")]
    fn view_resize(&self, width: i32, height: i32) -> Option<(i32, i32)> {
        self.view.as_ref()?.resize(width, height).ok()
    }

    #[cfg(not(target_os = "windows"))]
    fn view_resize(&self, _width: i32, _height: i32) -> Option<(i32, i32)> {
        None
    }

    #[cfg(target_os = "windows")]
    fn being_dragged(&self) -> bool {
        self.native_window.is_some_and(|hwnd| {
            dragging()
                .lock()
                .is_ok_and(|d| d.contains(&(hwnd as usize)))
        })
    }

    #[cfg(not(target_os = "windows"))]
    fn being_dragged(&self) -> bool {
        false
    }

    /// Resize the window's client area to `width` x `height` as dragging its edge would, for
    /// checks (vendored addition): the editor follows on the next
    /// [`Self::service_platform_events`].
    pub fn drag_to(&self, width: i32, height: i32) {
        self.resize_native_window(width, height);
    }

    /// Run this thread's pending window messages for `duration`, for checks and other hosts
    /// without an event loop (vendored addition): editors that apply a resize on their next
    /// message (JUCE's) need one.
    #[cfg(target_os = "windows")]
    pub fn pump_messages(duration: std::time::Duration) {
        use winapi::um::winuser::{DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE};
        let until = std::time::Instant::now() + duration;
        while std::time::Instant::now() < until {
            // SAFETY: `msg` is a plain struct PeekMessageW fills; only this thread's messages.
            unsafe {
                let mut msg: MSG = std::mem::zeroed();
                while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[cfg(not(target_os = "windows"))]
    pub fn pump_messages(duration: std::time::Duration) {
        std::thread::sleep(duration);
    }

    /// The window's client area, in pixels, for checks (vendored addition): after servicing it
    /// is the editor's size, so no part of it is left empty.
    #[cfg(target_os = "windows")]
    pub fn client_size(&self) -> Option<(i32, i32)> {
        let hwnd = self.native_window?;
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: `hwnd` is this window's, alive until `close()`.
        (unsafe { GetClientRect(hwnd, &mut rect) } != 0).then_some((rect.right, rect.bottom))
    }

    #[cfg(not(target_os = "windows"))]
    pub fn client_size(&self) -> Option<(i32, i32)> {
        None
    }

    /// The newest client size the user dragged this window to.
    #[cfg(target_os = "windows")]
    fn take_pending_size(&self) -> Option<(i32, i32)> {
        let hwnd = self.native_window?;
        size_changes()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(&(hwnd as usize))
    }

    /// Only Windows records user resizes in the window procedure.
    #[cfg(not(target_os = "windows"))]
    fn take_pending_size(&self) -> Option<(i32, i32)> {
        None
    }

    /// Resize the native window so it fits an editor of `width` x `height` pixels.
    ///
    /// The plugin has already resized its own view (the host answered `resizeView` with
    /// `onSize`); this catches the window up.
    fn resize_native_window(&self, width: i32, height: i32) {
        if width <= 0 || height <= 0 {
            return;
        }
        log::debug!("editor asked the host to resize its window to {width}x{height}");

        #[cfg(target_os = "macos")]
        {
            let Some(window) = self.native_window.as_ref() else {
                return;
            };
            let size = NSSize::new(width as f64, height as f64);
            if let Some(container) = self.container_view.as_ref() {
                container.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), size));
            }
            window.setContentSize(size);
        }

        #[cfg(target_os = "windows")]
        {
            let Some(hwnd) = self.native_window else {
                return;
            };
            // The plugin sizes its *client* area; grow the frame by the window's chrome, the
            // same way `open()` sizes the window to the editor in the first place, in its
            // current style (a fixed-size editor's window has no resize border).
            let mut rect = RECT {
                left: 0,
                top: 0,
                right: width,
                bottom: height,
            };
            unsafe {
                let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
                winapi::um::winuser::AdjustWindowRectEx(&mut rect, style, 0, 0);
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    0,
                    0,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
        }

        #[cfg(target_os = "linux")]
        {
            let Some(state) = self.native_window.as_ref() else {
                return;
            };
            state.connection.send_request(&xcb::x::ConfigureWindow {
                window: state.window,
                value_list: &[
                    xcb::x::ConfigWindow::Width(width as u32),
                    xcb::x::ConfigWindow::Height(height as u32),
                ],
            });
            let _ = state.connection.flush();
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        let _ = (width, height);
    }

    /// Close the plugin window
    pub fn close(&mut self) {
        // Detach the plugin editor first — it must never outlive the window it is embedded in.
        // A poisoned lock (an earlier panic on the audio thread) is recovered rather than
        // skipped, exactly as every other lock site here does: skipping it would destroy the
        // native window with the plugin's view still attached to it.
        // On Windows the view is taken out under the lock and detached with it released, so the
        // audio thread keeps processing (a vendored change; see VENDORED.md).
        #[cfg(target_os = "windows")]
        {
            // The window's own reference to the view goes before the view is detached, and the
            // frame forgets this window.
            self.view = None;
            self.resize_requests = None;
            if let Some(slot) = self.frame_window.take() {
                slot.store(0, std::sync::atomic::Ordering::Release);
            }
            let released = self
                .plugin
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .release_editor();
            match released {
                Some(editor) => {
                    let _ = editor.detach();
                }
                None => {
                    let _ = self
                        .plugin
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .close_editor();
                }
            }
        }
        #[cfg(not(target_os = "windows"))]
        let _ = self
            .plugin
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .close_editor();

        // Then close the native window
        #[cfg(target_os = "macos")]
        {
            self.container_view = None;
            if let Some(window) = self.native_window.take() {
                window.close();
            }
        }

        #[cfg(target_os = "windows")]
        {
            if let Some(hwnd) = self.native_window.take() {
                if let Ok(mut requests) = close_requests().lock() {
                    requests.remove(&(hwnd as usize));
                }
                if let Ok(mut changes) = dpi_changes().lock() {
                    changes.remove(&(hwnd as usize));
                }
                unsafe {
                    DestroyWindow(hwnd);
                }
            }
        }

        #[cfg(target_os = "linux")]
        {
            if let Some(state) = self.native_window.take() {
                state.connection.send_request(&xcb::x::UnmapWindow {
                    window: state.window,
                });
                state.connection.send_request(&xcb::x::DestroyWindow {
                    window: state.window,
                });
                let _ = state.connection.flush();
            }
        }

        #[cfg(target_os = "android")]
        {
            let _ = self.native_window.take();
        }
    }

    /// Check if the window is currently open.
    ///
    /// Reports `false` once the user has dismissed the window themselves, even though the
    /// handle is still around — see [`Self::closed_by_user`].
    ///
    /// Reads native window state only: it never takes the plugin lock, so it is safe to call
    /// while holding it. Pair it with [`Self::service_platform_events`], which does the work
    /// that has to reach the plugin.
    pub fn is_open(&self) -> bool {
        self.native_window.is_some() && !self.native_window_dismissed()
    }

    /// Whether the user closed the window themselves, through its title-bar close button.
    ///
    /// Neither the plugin nor the host is told when that happens, so a host that tracks "the
    /// editor is open" in its own UI should poll this each frame and drop or
    /// [`close`](Self::close) the window when it reports `true`. Otherwise the editor stays
    /// attached to a window that is gone from the screen.
    ///
    /// Cheap and lock-free in the same sense as [`Self::is_open`]: native window state only,
    /// never the plugin lock.
    pub fn closed_by_user(&self) -> bool {
        self.native_window_dismissed()
    }

    /// Platform probe behind [`Self::closed_by_user`].
    #[cfg(target_os = "macos")]
    fn native_window_dismissed(&self) -> bool {
        let Some(window) = self.native_window.as_ref() else {
            return false;
        };
        // A closed window is ordered out — but so is a miniaturized one, and so is every window
        // of a hidden application. Neither of those means the user dismissed the editor.
        if window.isVisible() || window.isMiniaturized() {
            return false;
        }
        match MainThreadMarker::new() {
            Some(mtm) => !NSApplication::sharedApplication(mtm).isHidden(),
            // AppKit state can only be read from the main thread; assume the window still stands.
            None => false,
        }
    }

    /// Platform probe behind [`Self::closed_by_user`].
    #[cfg(target_os = "windows")]
    fn native_window_dismissed(&self) -> bool {
        let Some(hwnd) = self.native_window else {
            return false;
        };
        close_requests()
            .lock()
            .is_ok_and(|requests| requests.contains(&(hwnd as usize)))
    }

    /// Platform probe behind [`Self::closed_by_user`]. X11 and Android editor windows carry no
    /// close affordance of their own, so there is nothing to observe.
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn native_window_dismissed(&self) -> bool {
        false
    }
}

impl Drop for PluginWindow {
    fn drop(&mut self) {
        self.close();
    }
}

/// Builder for creating plugin windows with egui integration
#[cfg(feature = "egui-widgets")]
pub struct PluginWindowBuilder {
    plugin: Arc<Mutex<Plugin>>,
}

#[cfg(feature = "egui-widgets")]
impl PluginWindowBuilder {
    /// Create a new builder for the given plugin
    pub fn new(plugin: Arc<Mutex<Plugin>>) -> Self {
        Self { plugin }
    }

    /// Build and open a standalone plugin window
    pub fn open_standalone(&self) -> Result<PluginWindow> {
        let mut window = PluginWindow::new(self.plugin.clone());
        window.open()?;
        Ok(window)
    }
}

#[cfg(test)]
mod tests {
    use super::dpi_scale_factor;

    #[test]
    fn windows_dpi_converts_to_vst_content_scale() {
        assert_eq!(dpi_scale_factor(0), None);
        assert_eq!(dpi_scale_factor(96), Some(1.0));
        assert_eq!(dpi_scale_factor(120), Some(1.25));
        assert_eq!(dpi_scale_factor(144), Some(1.5));
        assert_eq!(dpi_scale_factor(192), Some(2.0));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn wm_dpi_wparam_uses_horizontal_dpi() {
        let packed = (192usize << 16) | 144;
        assert_eq!(super::dpi_from_wparam(packed), Some(144));
        assert_eq!(super::dpi_from_wparam(0), None);
    }
}
