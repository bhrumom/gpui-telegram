//! Cross-platform and headless GPUI viewport rendering `gpui::RenderImage` PNG frames
//! from a [`BrowserSessionBackend`] and forwarding pointer/scroll/keyboard inputs.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bot_browser::{
    BrowserSessionBackend, KeyEventKind, KeyInputEvent, PointerButton, PointerEventKind,
    PointerInputEvent,
};
use futures::channel::oneshot;
use gpui::{
    canvas, div, img, px, rgb, AsyncApp, Bounds, Context, FocusHandle, Focusable,
    InteractiveElement, IntoElement, KeyDownEvent, KeyUpEvent, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, ParentElement, Pixels, Point,
    Render, ScrollDelta, ScrollWheelEvent, Styled, StyledImage, WeakEntity, Window,
};
use parking_lot::Mutex;

use crate::views::tdesktop_theme::{tr_str, TdesktopThemeMode, TdesktopThemePalette};

static FALLBACK_BROWSER_TOKIO_RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

fn resolve_tokio_handle(preferred: Option<&tokio::runtime::Handle>) -> tokio::runtime::Handle {
    if let Some(h) = preferred {
        return h.clone();
    }
    if let Ok(h) = tokio::runtime::Handle::try_current() {
        return h;
    }
    FALLBACK_BROWSER_TOKIO_RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("build fallback Tokio runtime for CdpViewportSurface")
        })
        .handle()
        .clone()
}

async fn run_on_tokio<F, T>(handle: &tokio::runtime::Handle, fut: F) -> Result<T, String>
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = oneshot::channel();
    handle.spawn(async move {
        let out = fut.await;
        let _ = tx.send(out);
    });
    rx.await
        .map_err(|_| "browser backend Tokio task dropped before completion".to_string())
}

/// Decodes PNG screenshot bytes into a BGRA8 [`gpui::RenderImage`] off the UI thread.
pub fn decode_png_to_render_image(
    png_bytes: &[u8],
) -> Result<(Arc<gpui::RenderImage>, (u32, u32)), String> {
    if png_bytes.is_empty() {
        return Err("empty PNG screenshot payload".to_string());
    }
    let decoded = image::load_from_memory_with_format(png_bytes, image::ImageFormat::Png)
        .map_err(|e| format!("invalid PNG screenshot: {e}"))?;
    let mut rgba = decoded.into_rgba8();
    let (w, h) = rgba.dimensions();
    if w == 0 || h == 0 {
        return Err(format!("invalid zero-dimension screenshot ({w}x{h})"));
    }
    // gpui::RenderImage requires BGRA8 pixel order.
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let frame = image::Frame::new(rgba);
    Ok((Arc::new(gpui::RenderImage::new(vec![frame])), (w, h)))
}

fn sanitize_unit_clamp(val: f64) -> f64 {
    if val.is_finite() {
        val.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn sanitize_range_clamp(val: f64, max: f64) -> f64 {
    let safe_max = if max.is_finite() && max >= 0.0 {
        max
    } else {
        0.0
    };
    if val.is_finite() {
        val.clamp(0.0, safe_max)
    } else {
        0.0
    }
}

/// Maps a GPUI window-space `point` inside `bounds` (laid out with `ObjectFit::Contain`)
/// into browser CSS viewport coordinates `(0.0..=vp_w, 0.0..=vp_h)`.
pub fn map_point_contain(
    point: Point<Pixels>,
    bounds: Bounds<Pixels>,
    image_size: (u32, u32),
    viewport_size: (u32, u32),
) -> (f64, f64) {
    let raw_bw = f32::from(bounds.size.width);
    let raw_bh = f32::from(bounds.size.height);
    let bw = if raw_bw.is_finite() { raw_bw.max(1.0) } else { 1.0 };
    let bh = if raw_bh.is_finite() { raw_bh.max(1.0) } else { 1.0 };
    let ox = {
        let v = f32::from(bounds.origin.x);
        if v.is_finite() { v } else { 0.0 }
    };
    let oy = {
        let v = f32::from(bounds.origin.y);
        if v.is_finite() { v } else { 0.0 }
    };
    let img_w = (image_size.0.max(1)) as f32;
    let img_h = (image_size.1.max(1)) as f32;
    let vp_w = f64::from(viewport_size.0.max(1));
    let vp_h = f64::from(viewport_size.1.max(1));

    let img_aspect = img_w / img_h;
    let vp_aspect = bw / bh;
    let (img_x, img_y, draw_w, draw_h) = if vp_aspect > img_aspect {
        let w = bh * img_aspect;
        ((bw - w) * 0.5, 0.0, w, bh)
    } else {
        let h = bw / img_aspect;
        (0.0, (bh - h) * 0.5, bw, h)
    };

    let px_x = f32::from(point.x);
    let px_y = f32::from(point.y);
    let nx = if px_x.is_finite() {
        let rel_x = f64::from(px_x) - f64::from(ox) - f64::from(img_x);
        sanitize_unit_clamp(rel_x / f64::from(draw_w.max(1.0)))
    } else {
        0.0
    };
    let ny = if px_y.is_finite() {
        let rel_y = f64::from(px_y) - f64::from(oy) - f64::from(img_y);
        sanitize_unit_clamp(rel_y / f64::from(draw_h.max(1.0)))
    } else {
        0.0
    };
    (
        sanitize_range_clamp(nx * vp_w, vp_w),
        sanitize_range_clamp(ny * vp_h, vp_h),
    )
}

fn gpui_modifiers_to_cdp(m: Modifiers) -> u8 {
    let mut bits = 0u8;
    if m.alt {
        bits |= 1;
    }
    if m.control {
        bits |= 2;
    }
    if m.platform {
        bits |= 4;
    }
    if m.shift {
        bits |= 8;
    }
    bits
}

fn gpui_button_to_pointer(btn: MouseButton) -> Option<PointerButton> {
    match btn {
        MouseButton::Left => Some(PointerButton::Left),
        MouseButton::Middle => Some(PointerButton::Middle),
        MouseButton::Right => Some(PointerButton::Right),
        _ => None,
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum SurfaceErrorOrigin {
    Navigation,
    ScreenshotCapture,
    InputDispatch,
}

/// Interactive cross-platform/headless GPUI viewport backed by an async [`BrowserSessionBackend`].
pub struct CdpViewportSurface {
    backend: Arc<dyn BrowserSessionBackend>,
    profile_id: Option<String>,
    viewport_size: (u32, u32),
    rendered_frame: Option<Arc<gpui::RenderImage>>,
    frame_pixel_size: Option<(u32, u32)>,
    frame_revision: u64,
    next_refresh_seq: u64,
    latest_applied_seq: u64,
    next_nav_seq: u64,
    latest_applied_nav_seq: u64,
    in_flight_refreshes: usize,
    in_flight_navs: usize,
    current_url: String,
    page_title: String,
    last_error: Option<String>,
    last_error_origin: Option<SurfaceErrorOrigin>,
    interactive_enabled: bool,
    auto_refresh_generation: u64,
    focus_handle: FocusHandle,
    viewport_bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    tokio_handle: Option<tokio::runtime::Handle>,
    palette: TdesktopThemePalette,
    language_id: String,
}

impl CdpViewportSurface {
    pub fn new(backend: Arc<dyn BrowserSessionBackend>, cx: &mut Context<Self>) -> Self {
        Self::with_options(
            backend,
            None,
            (1280, 800),
            tokio::runtime::Handle::try_current().ok(),
            cx,
        )
    }

    pub fn with_options(
        backend: Arc<dyn BrowserSessionBackend>,
        profile_id: Option<String>,
        viewport_size: (u32, u32),
        tokio_handle: Option<tokio::runtime::Handle>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.on_release(|this: &mut Self, cx: &mut gpui::App| {
            if let Some(prev) = this.rendered_frame.take() {
                cx.drop_image(prev, None);
            }
        })
        .detach();

        Self {
            backend,
            profile_id,
            viewport_size: (viewport_size.0.max(1), viewport_size.1.max(1)),
            rendered_frame: None,
            frame_pixel_size: None,
            frame_revision: 0,
            next_refresh_seq: 0,
            latest_applied_seq: 0,
            next_nav_seq: 0,
            latest_applied_nav_seq: 0,
            in_flight_refreshes: 0,
            in_flight_navs: 0,
            current_url: String::new(),
            page_title: String::new(),
            last_error: None,
            last_error_origin: None,
            interactive_enabled: true,
            auto_refresh_generation: 0,
            focus_handle: cx.focus_handle(),
            viewport_bounds: Arc::new(Mutex::new(None)),
            tokio_handle,
            palette: TdesktopThemePalette::build(TdesktopThemeMode::Night, 0x3b82f6, 14, 10),
            language_id: "en".to_string(),
        }
    }

    pub fn set_palette(
        &mut self,
        palette: TdesktopThemePalette,
        language_id: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        self.palette = palette;
        self.language_id = language_id.into();
        cx.notify();
    }

    pub fn set_viewport_size(&mut self, width: u32, height: u32, cx: &mut Context<Self>) {
        self.viewport_size = (width.max(1), height.max(1));
        cx.notify();
    }

    pub fn set_viewport_bounds_for_test(&self, bounds: Option<Bounds<Pixels>>) {
        *self.viewport_bounds.lock() = bounds;
    }

    pub fn last_frame_revision(&self) -> u64 {
        self.frame_revision
    }

    pub fn has_rendered_frame(&self) -> bool {
        self.rendered_frame.is_some()
    }

    pub fn viewport_size(&self) -> (u32, u32) {
        self.viewport_size
    }

    pub fn frame_pixel_size(&self) -> Option<(u32, u32)> {
        self.frame_pixel_size
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn clear_error(&mut self, cx: &mut Context<Self>) {
        if self.last_error.is_some() || self.last_error_origin.is_some() {
            self.last_error = None;
            self.last_error_origin = None;
            cx.notify();
        }
    }

    pub fn is_refreshing(&self) -> bool {
        self.in_flight_refreshes > 0
    }

    pub fn is_navigating(&self) -> bool {
        self.in_flight_navs > 0
    }

    pub fn backend(&self) -> &Arc<dyn BrowserSessionBackend> {
        &self.backend
    }

    pub fn current_url(&self) -> &str {
        &self.current_url
    }

    pub fn page_title(&self) -> &str {
        &self.page_title
    }

    pub fn profile_id(&self) -> Option<&str> {
        self.profile_id.as_deref()
    }

    pub fn rendered_image(&self) -> Option<&Arc<gpui::RenderImage>> {
        self.rendered_frame.as_ref()
    }

    pub fn is_interactive_enabled(&self) -> bool {
        self.interactive_enabled
    }

    pub fn set_interactive_enabled(&mut self, enabled: bool) {
        self.interactive_enabled = enabled;
    }

    fn apply_frame_outcome(
        &mut self,
        decoded: Result<(Arc<gpui::RenderImage>, (u32, u32)), String>,
        err_origin: SurfaceErrorOrigin,
        preserve_nav_error: bool,
        cx: &mut Context<Self>,
    ) {
        match decoded {
            Ok((render_img, dims)) => {
                if let Some(prev) = self.rendered_frame.replace(render_img) {
                    cx.drop_image(prev, None);
                }
                self.frame_pixel_size = Some(dims);
                self.frame_revision = self.frame_revision.saturating_add(1);
                if !(preserve_nav_error
                    && self.last_error_origin == Some(SurfaceErrorOrigin::Navigation))
                {
                    self.last_error = None;
                    self.last_error_origin = None;
                }
            }
            Err(err_msg) => {
                if !(preserve_nav_error
                    && self.last_error_origin == Some(SurfaceErrorOrigin::Navigation))
                {
                    self.last_error = Some(err_msg);
                    self.last_error_origin = Some(err_origin);
                }
            }
        }
    }

    fn apply_refresh_frame(
        &mut self,
        seq: u64,
        spawn_nav_seq: u64,
        nav_in_flight_at_spawn: bool,
        meta: Option<(Option<String>, Option<String>)>,
        decoded: Result<(Arc<gpui::RenderImage>, (u32, u32)), String>,
        cx: &mut Context<Self>,
    ) {
        self.in_flight_refreshes = self.in_flight_refreshes.saturating_sub(1);
        if seq < self.latest_applied_seq || self.latest_applied_nav_seq > spawn_nav_seq {
            cx.notify();
            return;
        }
        self.latest_applied_seq = seq;
        if !nav_in_flight_at_spawn
            && self.in_flight_navs == 0
            && spawn_nav_seq == self.next_nav_seq
        {
            if let Some((url_opt, title_opt)) = meta {
                if let Some(url) = url_opt {
                    self.current_url = url;
                }
                if let Some(title) = title_opt {
                    self.page_title = title;
                }
            }
        }
        self.apply_frame_outcome(decoded, SurfaceErrorOrigin::ScreenshotCapture, true, cx);
        cx.notify();
    }

    fn apply_nav_frame(
        &mut self,
        nav_seq: u64,
        nav_meta: Option<(Option<String>, Option<String>)>,
        decoded: Result<(Arc<gpui::RenderImage>, (u32, u32)), String>,
        err_origin: SurfaceErrorOrigin,
        cx: &mut Context<Self>,
    ) {
        self.in_flight_navs = self.in_flight_navs.saturating_sub(1);
        self.in_flight_refreshes = self.in_flight_refreshes.saturating_sub(1);
        if nav_seq < self.latest_applied_nav_seq {
            cx.notify();
            return;
        }
        self.latest_applied_nav_seq = nav_seq;
        // Advance the screenshot frame horizon past any pre-nav refresh_frame
        // spawned while this navigation was still settling.
        self.next_refresh_seq = self.next_refresh_seq.saturating_add(1);
        self.latest_applied_seq = self.next_refresh_seq;
        if let Some((url_opt, title_opt)) = nav_meta {
            if let Some(url) = url_opt {
                self.current_url = url;
            }
            if let Some(title) = title_opt {
                self.page_title = title;
            }
        }
        self.apply_frame_outcome(decoded, err_origin, false, cx);
        cx.notify();
    }

    pub fn refresh_frame(&mut self, cx: &mut Context<Self>) {
        self.next_refresh_seq = self.next_refresh_seq.saturating_add(1);
        let seq = self.next_refresh_seq;
        let spawn_nav_seq = self.next_nav_seq;
        let nav_in_flight_at_spawn = self.in_flight_navs > 0;
        self.in_flight_refreshes = self.in_flight_refreshes.saturating_add(1);

        let backend = Arc::clone(&self.backend);
        let handle = resolve_tokio_handle(self.tokio_handle.as_ref());

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let fetch_res = run_on_tokio(&handle, async move {
                let png_res = backend
                    .capture_screenshot()
                    .await
                    .map_err(|e| e.to_string());
                let url_opt = backend
                    .current_url()
                    .await
                    .ok()
                    .map(|u| u.trim().to_string())
                    .filter(|u| !u.is_empty());
                let title_opt = backend
                    .page_title()
                    .await
                    .ok()
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty());
                (url_opt, title_opt, png_res)
            })
            .await;

            let (meta, decoded) = match fetch_res {
                Ok((url_opt, title_opt, Ok(bytes))) => {
                    let dec = app
                        .background_executor()
                        .spawn(async move { decode_png_to_render_image(&bytes) })
                        .await;
                    (Some((url_opt, title_opt)), dec)
                }
                Ok((url_opt, title_opt, Err(err))) => (Some((url_opt, title_opt)), Err(err)),
                Err(err) => (None, Err(err)),
            };

            let _ = weak.update(app, |this, cx| {
                this.apply_refresh_frame(
                    seq,
                    spawn_nav_seq,
                    nav_in_flight_at_spawn,
                    meta,
                    decoded,
                    cx,
                );
            });
        })
        .detach();
    }

    pub fn navigate_and_refresh(&mut self, url: impl Into<String>, cx: &mut Context<Self>) {
        let target_url = url.into();
        self.next_nav_seq = self.next_nav_seq.saturating_add(1);
        let nav_seq = self.next_nav_seq;
        self.in_flight_navs = self.in_flight_navs.saturating_add(1);
        self.in_flight_refreshes = self.in_flight_refreshes.saturating_add(1);
        self.last_error = None;
        self.last_error_origin = None;

        let backend = Arc::clone(&self.backend);
        let handle = resolve_tokio_handle(self.tokio_handle.as_ref());

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let nav_and_png = run_on_tokio(&handle, async move {
                match backend.navigate(&target_url).await {
                    Ok(nav) => {
                        let png_res = backend
                            .capture_screenshot()
                            .await
                            .map_err(|e| e.to_string());
                        let final_url = backend
                            .current_url()
                            .await
                            .ok()
                            .map(|u| u.trim().to_string())
                            .filter(|u| !u.is_empty())
                            .or_else(|| (!nav.url.trim().is_empty()).then_some(nav.url));
                        let title = backend
                            .page_title()
                            .await
                            .ok()
                            .map(|t| t.trim().to_string())
                            .filter(|t| !t.is_empty())
                            .or_else(|| (!nav.title.trim().is_empty()).then_some(nav.title));
                        Ok((final_url, title, png_res))
                    }
                    Err(e) => {
                        let err = e.to_string();
                        let cur_url = backend
                            .current_url()
                            .await
                            .ok()
                            .map(|u| u.trim().to_string())
                            .filter(|u| !u.is_empty());
                        let cur_title = backend
                            .page_title()
                            .await
                            .ok()
                            .map(|t| t.trim().to_string())
                            .filter(|t| !t.is_empty());
                        Err((err, cur_url, cur_title))
                    }
                }
            })
            .await;

            match nav_and_png {
                Ok(Ok((final_url, title, Ok(bytes)))) => {
                    let decoded = app
                        .background_executor()
                        .spawn(async move { decode_png_to_render_image(&bytes) })
                        .await;
                    let _ = weak.update(app, |this, cx| {
                        this.apply_nav_frame(
                            nav_seq,
                            Some((final_url, title)),
                            decoded,
                            SurfaceErrorOrigin::ScreenshotCapture,
                            cx,
                        );
                    });
                }
                Ok(Ok((final_url, title, Err(png_err)))) => {
                    let _ = weak.update(app, |this, cx| {
                        this.apply_nav_frame(
                            nav_seq,
                            Some((final_url, title)),
                            Err(png_err),
                            SurfaceErrorOrigin::ScreenshotCapture,
                            cx,
                        );
                    });
                }
                Ok(Err((err, cur_url, cur_title))) => {
                    let _ = weak.update(app, |this, cx| {
                        this.apply_nav_frame(
                            nav_seq,
                            Some((cur_url, cur_title)),
                            Err(err),
                            SurfaceErrorOrigin::Navigation,
                            cx,
                        );
                    });
                }
                Err(task_err) => {
                    let _ = weak.update(app, |this, cx| {
                        this.apply_nav_frame(
                            nav_seq,
                            None,
                            Err(task_err),
                            SurfaceErrorOrigin::Navigation,
                            cx,
                        );
                    });
                }
            }
        })
        .detach();
    }

    pub fn go_back_and_refresh(&mut self, cx: &mut Context<Self>) {
        self.run_history_op_and_refresh(HistoryNavOp::Back, cx);
    }

    pub fn go_forward_and_refresh(&mut self, cx: &mut Context<Self>) {
        self.run_history_op_and_refresh(HistoryNavOp::Forward, cx);
    }

    pub fn reload_and_refresh(&mut self, cx: &mut Context<Self>) {
        self.run_history_op_and_refresh(HistoryNavOp::Reload, cx);
    }

    fn run_history_op_and_refresh(&mut self, op: HistoryNavOp, cx: &mut Context<Self>) {
        self.next_nav_seq = self.next_nav_seq.saturating_add(1);
        let nav_seq = self.next_nav_seq;
        self.in_flight_navs = self.in_flight_navs.saturating_add(1);
        self.in_flight_refreshes = self.in_flight_refreshes.saturating_add(1);
        self.last_error = None;
        self.last_error_origin = None;

        let backend = Arc::clone(&self.backend);
        let handle = resolve_tokio_handle(self.tokio_handle.as_ref());

        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let op_res = run_on_tokio(&handle, async move {
                let op_result = match op {
                    HistoryNavOp::Back => backend.go_back().await,
                    HistoryNavOp::Forward => backend.go_forward().await,
                    HistoryNavOp::Reload => backend.reload().await,
                };
                match op_result {
                    Ok(()) => {
                        let png_res = backend
                            .capture_screenshot()
                            .await
                            .map_err(|e| e.to_string());
                        let url = backend
                            .current_url()
                            .await
                            .ok()
                            .map(|u| u.trim().to_string())
                            .filter(|u| !u.is_empty());
                        let title = backend
                            .page_title()
                            .await
                            .ok()
                            .map(|t| t.trim().to_string())
                            .filter(|t| !t.is_empty());
                        Ok((url, title, png_res))
                    }
                    Err(e) => {
                        let err = e.to_string();
                        let cur_url = backend
                            .current_url()
                            .await
                            .ok()
                            .map(|u| u.trim().to_string())
                            .filter(|u| !u.is_empty());
                        let cur_title = backend
                            .page_title()
                            .await
                            .ok()
                            .map(|t| t.trim().to_string())
                            .filter(|t| !t.is_empty());
                        Err((err, cur_url, cur_title))
                    }
                }
            })
            .await;

            match op_res {
                Ok(Ok((url, title, Ok(bytes)))) => {
                    let decoded = app
                        .background_executor()
                        .spawn(async move { decode_png_to_render_image(&bytes) })
                        .await;
                    let _ = weak.update(app, |this, cx| {
                        this.apply_nav_frame(
                            nav_seq,
                            Some((url, title)),
                            decoded,
                            SurfaceErrorOrigin::ScreenshotCapture,
                            cx,
                        );
                    });
                }
                Ok(Ok((url, title, Err(png_err)))) => {
                    let _ = weak.update(app, |this, cx| {
                        this.apply_nav_frame(
                            nav_seq,
                            Some((url, title)),
                            Err(png_err),
                            SurfaceErrorOrigin::ScreenshotCapture,
                            cx,
                        );
                    });
                }
                Ok(Err((err, cur_url, cur_title))) => {
                    let _ = weak.update(app, |this, cx| {
                        this.apply_nav_frame(
                            nav_seq,
                            Some((cur_url, cur_title)),
                            Err(err),
                            SurfaceErrorOrigin::Navigation,
                            cx,
                        );
                    });
                }
                Err(task_err) => {
                    let _ = weak.update(app, |this, cx| {
                        this.apply_nav_frame(
                            nav_seq,
                            None,
                            Err(task_err),
                            SurfaceErrorOrigin::Navigation,
                            cx,
                        );
                    });
                }
            }
        })
        .detach();
    }

    fn set_input_error(&mut self, err: String, cx: &mut Context<Self>) {
        self.last_error = Some(err);
        self.last_error_origin = Some(SurfaceErrorOrigin::InputDispatch);
        cx.notify();
    }

    pub fn handle_click(
        &mut self,
        x: f64,
        y: f64,
        button: PointerButton,
        click_count: u8,
        modifiers: u8,
        cx: &mut Context<Self>,
    ) {
        if !self.interactive_enabled {
            return;
        }
        let backend = Arc::clone(&self.backend);
        let handle = resolve_tokio_handle(self.tokio_handle.as_ref());
        let clicks = click_count.max(1);
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_tokio(&handle, async move {
                backend
                    .dispatch_pointer_event(PointerInputEvent {
                        kind: PointerEventKind::Down,
                        x,
                        y,
                        button,
                        click_count: clicks,
                        delta_x: 0.0,
                        delta_y: 0.0,
                        modifiers,
                    })
                    .await
                    .map_err(|e| e.to_string())?;
                backend
                    .dispatch_pointer_event(PointerInputEvent {
                        kind: PointerEventKind::Up,
                        x,
                        y,
                        button,
                        click_count: clicks,
                        delta_x: 0.0,
                        delta_y: 0.0,
                        modifiers,
                    })
                    .await
                    .map_err(|e| e.to_string())
            })
            .await;

            if let Ok(Err(err)) | Err(err) = res {
                let _ = weak.update(app, |this, cx| {
                    this.set_input_error(err, cx);
                });
            } else {
                let _ = weak.update(app, |this, cx| {
                    this.refresh_frame(cx);
                });
            }
        })
        .detach();
    }

    pub fn handle_mouse_move(
        &mut self,
        x: f64,
        y: f64,
        button: PointerButton,
        modifiers: u8,
        cx: &mut Context<Self>,
    ) {
        if !self.interactive_enabled {
            return;
        }
        let backend = Arc::clone(&self.backend);
        let handle = resolve_tokio_handle(self.tokio_handle.as_ref());
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_tokio(&handle, async move {
                backend
                    .dispatch_pointer_event(PointerInputEvent {
                        kind: PointerEventKind::Move,
                        x,
                        y,
                        button,
                        click_count: 0,
                        delta_x: 0.0,
                        delta_y: 0.0,
                        modifiers,
                    })
                    .await
                    .map_err(|e| e.to_string())
            })
            .await;
            if let Ok(Err(err)) | Err(err) = res {
                let _ = weak.update(app, |this, cx| {
                    this.set_input_error(err, cx);
                });
            }
        })
        .detach();
    }

    pub fn handle_scroll(
        &mut self,
        x: f64,
        y: f64,
        delta_x: f64,
        delta_y: f64,
        modifiers: u8,
        cx: &mut Context<Self>,
    ) {
        if !self.interactive_enabled {
            return;
        }
        let backend = Arc::clone(&self.backend);
        let handle = resolve_tokio_handle(self.tokio_handle.as_ref());
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_tokio(&handle, async move {
                backend
                    .dispatch_pointer_event(PointerInputEvent {
                        kind: PointerEventKind::Wheel,
                        x,
                        y,
                        button: PointerButton::None,
                        click_count: 0,
                        delta_x,
                        delta_y,
                        modifiers,
                    })
                    .await
                    .map_err(|e| e.to_string())
            })
            .await;
            if let Ok(Err(err)) | Err(err) = res {
                let _ = weak.update(app, |this, cx| {
                    this.set_input_error(err, cx);
                });
            } else {
                let _ = weak.update(app, |this, cx| {
                    this.refresh_frame(cx);
                });
            }
        })
        .detach();
    }

    pub fn handle_key_down(
        &mut self,
        key: impl Into<String>,
        code: Option<String>,
        text: Option<String>,
        windows_virtual_key_code: Option<u32>,
        modifiers: u8,
        cx: &mut Context<Self>,
    ) {
        if !self.interactive_enabled {
            return;
        }
        let key_str = key.into();
        let backend = Arc::clone(&self.backend);
        let handle = resolve_tokio_handle(self.tokio_handle.as_ref());
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_tokio(&handle, async move {
                backend
                    .dispatch_key_event(KeyInputEvent {
                        kind: KeyEventKind::KeyDown,
                        key: key_str,
                        code,
                        text,
                        windows_virtual_key_code,
                        modifiers,
                    })
                    .await
                    .map_err(|e| e.to_string())
            })
            .await;
            if let Ok(Err(err)) | Err(err) = res {
                let _ = weak.update(app, |this, cx| {
                    this.set_input_error(err, cx);
                });
            }
        })
        .detach();
    }

    pub fn handle_key_up(
        &mut self,
        key: impl Into<String>,
        code: Option<String>,
        windows_virtual_key_code: Option<u32>,
        modifiers: u8,
        cx: &mut Context<Self>,
    ) {
        if !self.interactive_enabled {
            return;
        }
        let key_str = key.into();
        let backend = Arc::clone(&self.backend);
        let handle = resolve_tokio_handle(self.tokio_handle.as_ref());
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_tokio(&handle, async move {
                backend
                    .dispatch_key_event(KeyInputEvent {
                        kind: KeyEventKind::KeyUp,
                        key: key_str,
                        code,
                        text: None,
                        windows_virtual_key_code,
                        modifiers,
                    })
                    .await
                    .map_err(|e| e.to_string())
            })
            .await;
            if let Ok(Err(err)) | Err(err) = res {
                let _ = weak.update(app, |this, cx| {
                    this.set_input_error(err, cx);
                });
            }
        })
        .detach();
    }

    pub fn send_text_input(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        if !self.interactive_enabled {
            return;
        }
        let text_str = text.into();
        if text_str.is_empty() {
            return;
        }
        let backend = Arc::clone(&self.backend);
        let handle = resolve_tokio_handle(self.tokio_handle.as_ref());
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_tokio(&handle, async move {
                backend
                    .dispatch_key_event(KeyInputEvent {
                        kind: KeyEventKind::Char,
                        key: "Unidentified".to_string(),
                        code: None,
                        text: Some(text_str),
                        windows_virtual_key_code: None,
                        modifiers: 0,
                    })
                    .await
                    .map_err(|e| e.to_string())
            })
            .await;
            if let Ok(Err(err)) | Err(err) = res {
                let _ = weak.update(app, |this, cx| {
                    this.set_input_error(err, cx);
                });
            } else {
                let _ = weak.update(app, |this, cx| {
                    this.refresh_frame(cx);
                });
            }
        })
        .detach();
    }

    pub fn start_auto_refresh(&mut self, interval: Duration, cx: &mut Context<Self>) {
        let clamped = interval.max(Duration::from_millis(16));
        self.auto_refresh_generation = self.auto_refresh_generation.wrapping_add(1);
        let generation = self.auto_refresh_generation;
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| loop {
            app.background_executor().timer(clamped).await;
            let keep_going = weak
                .update(app, |this, cx| {
                    if this.auto_refresh_generation != generation {
                        return false;
                    }
                    if !this.is_refreshing() {
                        this.refresh_frame(cx);
                    }
                    true
                })
                .unwrap_or(false);
            if !keep_going {
                break;
            }
        })
        .detach();
    }

    pub fn stop_auto_refresh(&mut self) {
        self.auto_refresh_generation = self.auto_refresh_generation.wrapping_add(1);
    }

    pub fn map_window_point_to_viewport(&self, point: Point<Pixels>) -> (f64, f64) {
        let bounds_opt = *self.viewport_bounds.lock();
        if let Some(bounds) = bounds_opt {
            let img_dims = self.frame_pixel_size.unwrap_or(self.viewport_size);
            map_point_contain(point, bounds, img_dims, self.viewport_size)
        } else {
            let max_x = f64::from(self.viewport_size.0.max(1));
            let max_y = f64::from(self.viewport_size.1.max(1));
            (
                sanitize_range_clamp(f64::from(f32::from(point.x)), max_x),
                sanitize_range_clamp(f64::from(f32::from(point.y)), max_y),
            )
        }
    }

    fn dispatch_single_pointer_event(
        &mut self,
        event: PointerInputEvent,
        refresh_after: bool,
        cx: &mut Context<Self>,
    ) {
        let backend = Arc::clone(&self.backend);
        let handle = resolve_tokio_handle(self.tokio_handle.as_ref());
        cx.spawn(async move |weak: WeakEntity<Self>, app: &mut AsyncApp| {
            let res = run_on_tokio(&handle, async move {
                backend
                    .dispatch_pointer_event(event)
                    .await
                    .map_err(|e| e.to_string())
            })
            .await;
            if let Ok(Err(err)) | Err(err) = res {
                let _ = weak.update(app, |this, cx| {
                    this.set_input_error(err, cx);
                });
            } else if refresh_after {
                let _ = weak.update(app, |this, cx| {
                    this.refresh_frame(cx);
                });
            }
        })
        .detach();
    }
}

#[derive(Copy, Clone)]
enum HistoryNavOp {
    Back,
    Forward,
    Reload,
}

impl Focusable for CdpViewportSurface {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for CdpViewportSurface {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = self.palette;
        let vp_bounds_for_canvas = self.viewport_bounds.clone();
        let bounds_tracker = canvas(
            |bounds, _, _| bounds,
            move |bounds, _, _, _| {
                *vp_bounds_for_canvas.lock() = Some(bounds);
            },
        )
        .absolute()
        .inset_0();

        let content = if let Some(ref frame) = self.rendered_frame {
            img(frame.clone())
                .size_full()
                .object_fit(ObjectFit::Contain)
                .into_any_element()
        } else if let Some(ref err) = self.last_error {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .p(px(16.0))
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(pal.danger))
                        .child(format!(
                            "{}: {err}",
                            tr_str(&self.language_id, "浏览器渲染异常", "Browser viewport error")
                        )),
                )
                .into_any_element()
        } else {
            let label = if self.is_refreshing() {
                tr_str(&self.language_id, "正在加载浏览器画面…", "Loading browser frame…")
            } else {
                tr_str(&self.language_id, "浏览器视口就绪", "Browser viewport ready")
            };
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(div().text_sm().text_color(rgb(pal.text_secondary)).child(label))
                .into_any_element()
        };

        let mut root = div()
            .id("cdp-viewport-surface")
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(rgb(pal.history_bg))
            .track_focus(&self.focus_handle);

        if self.interactive_enabled {
            root = root
                .cursor_pointer()
                .on_any_mouse_down(cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    this.focus_handle.focus(window);
                    let Some(button) = gpui_button_to_pointer(ev.button) else {
                        return;
                    };
                    let (x, y) = this.map_window_point_to_viewport(ev.position);
                    let modifiers = gpui_modifiers_to_cdp(ev.modifiers);
                    let click_count = ev.click_count.clamp(1, 255) as u8;
                    this.dispatch_single_pointer_event(
                        PointerInputEvent {
                            kind: PointerEventKind::Down,
                            x,
                            y,
                            button,
                            click_count,
                            delta_x: 0.0,
                            delta_y: 0.0,
                            modifiers,
                        },
                        false,
                        cx,
                    );
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, ev: &MouseUpEvent, _, cx| {
                        let (x, y) = this.map_window_point_to_viewport(ev.position);
                        let modifiers = gpui_modifiers_to_cdp(ev.modifiers);
                        this.dispatch_single_pointer_event(
                            PointerInputEvent {
                                kind: PointerEventKind::Up,
                                x,
                                y,
                                button: PointerButton::Left,
                                click_count: ev.click_count.clamp(1, 255) as u8,
                                delta_x: 0.0,
                                delta_y: 0.0,
                                modifiers,
                            },
                            true,
                            cx,
                        );
                    }),
                )
                .on_mouse_up(
                    MouseButton::Right,
                    cx.listener(|this, ev: &MouseUpEvent, _, cx| {
                        let (x, y) = this.map_window_point_to_viewport(ev.position);
                        let modifiers = gpui_modifiers_to_cdp(ev.modifiers);
                        this.dispatch_single_pointer_event(
                            PointerInputEvent {
                                kind: PointerEventKind::Up,
                                x,
                                y,
                                button: PointerButton::Right,
                                click_count: ev.click_count.clamp(1, 255) as u8,
                                delta_x: 0.0,
                                delta_y: 0.0,
                                modifiers,
                            },
                            true,
                            cx,
                        );
                    }),
                )
                .on_mouse_up(
                    MouseButton::Middle,
                    cx.listener(|this, ev: &MouseUpEvent, _, cx| {
                        let (x, y) = this.map_window_point_to_viewport(ev.position);
                        let modifiers = gpui_modifiers_to_cdp(ev.modifiers);
                        this.dispatch_single_pointer_event(
                            PointerInputEvent {
                                kind: PointerEventKind::Up,
                                x,
                                y,
                                button: PointerButton::Middle,
                                click_count: ev.click_count.clamp(1, 255) as u8,
                                delta_x: 0.0,
                                delta_y: 0.0,
                                modifiers,
                            },
                            true,
                            cx,
                        );
                    }),
                )
                .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                    let (x, y) = this.map_window_point_to_viewport(ev.position);
                    let button = ev
                        .pressed_button
                        .and_then(gpui_button_to_pointer)
                        .unwrap_or(PointerButton::None);
                    let modifiers = gpui_modifiers_to_cdp(ev.modifiers);
                    this.handle_mouse_move(x, y, button, modifiers, cx);
                }))
                .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, _, cx| {
                    let (x, y) = this.map_window_point_to_viewport(ev.position);
                    let (dx, dy) = match ev.delta {
                        ScrollDelta::Pixels(p) => (
                            f64::from(f32::from(p.x)),
                            f64::from(f32::from(p.y)),
                        ),
                        ScrollDelta::Lines(p) => (
                            f64::from(p.x) * 40.0,
                            f64::from(p.y) * 40.0,
                        ),
                    };
                    let modifiers = gpui_modifiers_to_cdp(ev.modifiers);
                    this.handle_scroll(x, y, -dx, -dy, modifiers, cx);
                }))
                .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                    if ev.keystroke.modifiers.platform || ev.keystroke.modifiers.control {
                        return;
                    }
                    let modifiers = gpui_modifiers_to_cdp(ev.keystroke.modifiers);
                    let key_char = ev.keystroke.key_char.clone();
                    this.handle_key_down(
                        ev.keystroke.key.clone(),
                        None,
                        key_char,
                        None,
                        modifiers,
                        cx,
                    );
                }))
                .on_key_up(cx.listener(|this, ev: &KeyUpEvent, _, cx| {
                    if ev.keystroke.modifiers.platform || ev.keystroke.modifiers.control {
                        return;
                    }
                    let modifiers = gpui_modifiers_to_cdp(ev.keystroke.modifiers);
                    this.handle_key_up(ev.keystroke.key.clone(), None, None, modifiers, cx);
                }));
        }

        root.child(content).child(bounds_tracker)
    }
}
