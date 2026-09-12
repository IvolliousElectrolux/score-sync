//! 内存分项 dump: `SCORE_SYNC_TRACE=1` 每 2 秒采样, `{ms}M` 立刻打到状态栏和日志.

use super::*;
use super::ScoreSyncApp;
use crate::mem::{self, fmt_bytes};

fn gpu_bytes(img: &RenderImage) -> u64 {
    let sz = img.size(0);
    let w = (sz.width.0 as i32).max(0) as u64;
    let h = (sz.height.0 as i32).max(0) as u64;
    mem::dims_bgra(w as u32, h as u32)
}

impl ScoreSyncApp {
    pub(super) fn start_memory_trace(&mut self, cx: &mut Context<Self>) {
        if !crate::trace::enabled() {
            return;
        }
        self.emit_memory("boot", false, cx);
        crate::trace::log("mem: 周期采样每 2s, 工作集或已计缓冲变化时写入; 30s 心跳");
        cx.spawn(async move |this, cx| {
            let mut ticks = 0u32;
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(2))
                    .await;
                ticks = ticks.wrapping_add(1);
                let heartbeat = ticks % 15 == 0;
                let cont = this
                    .update(cx, |view, cx| {
                        view.trace_memory_if_changed(heartbeat, cx);
                        true
                    })
                    .unwrap_or(false);
                if !cont {
                    break;
                }
            }
        })
        .detach();
    }

    pub(super) fn dump_memory_now(&mut self, cx: &mut Context<Self>) {
        let summary = self.emit_memory("hotkey", true, cx);
        self.status = summary.into();
        self.hint = if let Some(p) = crate::trace::log_path() {
            format!("内存分项已写入 {}", p.display()).into()
        } else {
            "内存分项已写入 %TEMP%/score_sync_trace.log".into()
        };
        cx.notify();
    }

    fn trace_memory_if_changed(&mut self, heartbeat: bool, cx: &mut Context<Self>) {
        let snap = self.collect_memory(cx);
        let ws_delta = snap.proc.working_set.abs_diff(self.mem_last_ws);
        let acc_delta = snap.accounted.abs_diff(self.mem_last_accounted);
        if !heartbeat && acc_delta < 256 * 1024 && ws_delta < 1024 * 1024 {
            return;
        }
        self.record_and_write(if heartbeat { "heartbeat" } else { "poll" }, false, snap);
    }

    pub(super) fn emit_memory(&mut self, reason: &str, force_file: bool, cx: &Context<Self>) -> String {
        let snap = self.collect_memory(cx);
        self.record_and_write(reason, force_file, snap)
    }

    fn record_and_write(&mut self, reason: &str, _force_file: bool, snap: MemSnap) -> String {
        self.mem_last_accounted = snap.accounted;
        self.mem_last_ws = snap.proc.working_set;
        let lines = snap.lines(reason);
        if crate::trace::enabled() {
            for line in &lines {
                crate::trace::log(line);
            }
        } else {
            for line in &lines {
                crate::trace::log_always(line);
            }
        }
        snap.summary()
    }

    fn collect_memory(&self, cx: &Context<Self>) -> MemSnap {
        let mut seen = std::collections::HashSet::new();
        let (page_n, pages) = self.doc.loaded_image_bytes_unique(&mut seen);
        let lod = self
            .lod_full
            .as_ref()
            .map(|img| mem::rgb_arc_unique_bytes(img, &mut seen))
            .unwrap_or(0);
        let bg_doc = self
            .doc
            .bg_image
            .as_ref()
            .map(|img| mem::rgb_arc_unique_bytes(img, &mut seen))
            .unwrap_or(0);
        let bg_ui = self
            .bg
            .cached_image
            .as_ref()
            .map(|img| mem::rgb_arc_unique_bytes(img, &mut seen))
            .unwrap_or(0);
        let bg_shared = matches!(
            (
                self.doc.bg_image.as_ref().map(|a| Arc::as_ptr(a) as usize),
                self.bg.cached_image.as_ref().map(|a| Arc::as_ptr(a) as usize),
            ),
            (Some(a), Some(b)) if a == b
        );
        let mut canvas_gpu = 0u64;
        let mut canvas_drop = 0u64;
        if let Some(img) = self.fit_render_image.as_ref() {
            canvas_gpu += gpu_bytes(img);
        }
        if let Some(img) = self.render_image.as_ref() {
            let extra = self
                .fit_render_image
                .as_ref()
                .map(|f| !Arc::ptr_eq(f, img))
                .unwrap_or(true);
            if extra {
                canvas_gpu += gpu_bytes(img);
            }
        }
        for img in &self.gpu_drop {
            canvas_drop += gpu_bytes(img);
        }
        let mut org = 0u64;
        for img in self.org_thumbs.values() {
            org += gpu_bytes(img);
        }
        let mut bg_preview = 0u64;
        if let Some(img) = self.bg.pending_preview.as_ref() {
            bg_preview += gpu_bytes(img);
        }
        if let Some(img) = self.bg.sb_image.as_ref() {
            bg_preview += gpu_bytes(img);
        }
        if let Some(img) = self.bg.hue_image.as_ref() {
            bg_preview += gpu_bytes(img);
        }
        let host_bg_tile_raw = self
            .bg_tile_cache
            .as_ref()
            .map(|(_, _, t)| {
                let (w, h) = t.thumb.dimensions();
                mem::dims_rgb(w, h) + mem::dims_bgra(w, h)
            })
            .unwrap_or(0);
        let pdf = self
            .pdf_import
            .as_ref()
            .and_then(|st| st.preview_image.as_ref())
            .map(|img| gpu_bytes(img))
            .unwrap_or(0);
        let mask = self.mask_tool.read(cx).accounted_memory();
        let video = self.score_video.read(cx).accounted_memory();
        let proc = mem::process_memory();
        let host_bg_tile = if mask.bg_gpu > 0 { 0 } else { host_bg_tile_raw };
        let accounted = pages
            + lod
            + bg_doc
            + bg_ui
            + canvas_gpu
            + canvas_drop
            + org
            + bg_preview
            + host_bg_tile
            + pdf
            + mask.total()
            + video.total();
        MemSnap {
            proc,
            accounted,
            page_n,
            page_total: self.doc.pages.len(),
            pages,
            radius: self.doc.memory_window_radius(),
            lod,
            bg_doc,
            bg_ui,
            bg_shared,
            canvas_gpu,
            canvas_drop,
            org,
            org_n: self.org_thumbs.len(),
            bg_preview,
            host_bg_tile,
            pdf,
            mask,
            video,
            tool: format!("{:?}", self.side_tool),
        }
    }
}

struct MemSnap {
    proc: mem::ProcessMem,
    accounted: u64,
    page_n: usize,
    page_total: usize,
    pages: u64,
    radius: usize,
    lod: u64,
    bg_doc: u64,
    bg_ui: u64,
    bg_shared: bool,
    canvas_gpu: u64,
    canvas_drop: u64,
    org: u64,
    org_n: usize,
    bg_preview: u64,
    host_bg_tile: u64,
    pdf: u64,
    mask: mask_tool::gui::MaskMemory,
    video: score_video::gui::VideoMemory,
    tool: String,
}

impl MemSnap {
    fn lines(&self, reason: &str) -> Vec<String> {
        let other = self.proc.working_set.saturating_sub(self.accounted);
        let bg_note = if self.bg_shared {
            "SHARED"
        } else if self.bg_doc > 0 && self.bg_ui > 0 {
            "DUP"
        } else {
            "-"
        };
        vec![
            format!(
                "mem [{reason}] tool={}  ws={}  private={}  accounted={}  other≈{}",
                self.tool,
                fmt_bytes(self.proc.working_set),
                fmt_bytes(self.proc.private),
                fmt_bytes(self.accounted),
                fmt_bytes(other)
            ),
            format!(
                "  pages {}/{} loaded  unique={}  window±{}",
                self.page_n,
                self.page_total,
                fmt_bytes(self.pages),
                self.radius
            ),
            format!(
                "  lod_full={}  bg_doc={}  bg_ui={} ({})  bg_preview={}  host_bg_tile={}",
                fmt_bytes(self.lod),
                fmt_bytes(self.bg_doc),
                fmt_bytes(self.bg_ui),
                bg_note,
                fmt_bytes(self.bg_preview),
                fmt_bytes(self.host_bg_tile)
            ),
            format!(
                "  canvas_gpu={}  gpu_drop={}  org_thumbs={} (n={})  pdf_preview={}",
                fmt_bytes(self.canvas_gpu),
                fmt_bytes(self.canvas_drop),
                fmt_bytes(self.org),
                self.org_n,
                fmt_bytes(self.pdf)
            ),
            format!(
                "  mask total={}  rgb={}  tiles_rgb={}  tiles_gpu={} (n={})  bg_rgb={}  bg_gpu={}  render={}  drop={}  picker={}",
                fmt_bytes(self.mask.total()),
                fmt_bytes(self.mask.rgb),
                fmt_bytes(self.mask.tiles_rgb),
                fmt_bytes(self.mask.tiles_gpu),
                self.mask.tile_n,
                fmt_bytes(self.mask.bg_rgb),
                fmt_bytes(self.mask.bg_gpu),
                fmt_bytes(self.mask.render_gpu),
                fmt_bytes(self.mask.gpu_drop),
                fmt_bytes(self.mask.picker_gpu)
            ),
            format!(
                "  video pool_gpu={} (n={})  waveform={}",
                fmt_bytes(self.video.pool_gpu),
                self.video.pool_n,
                fmt_bytes(self.video.waveform)
            ),
        ]
    }

    fn summary(&self) -> String {
        format!(
            "内存 ws={} 已计={} 页图={} 底色={} 蒙版={} 视频={}",
            fmt_bytes(self.proc.working_set),
            fmt_bytes(self.accounted),
            fmt_bytes(self.pages),
            fmt_bytes(self.bg_doc + self.bg_ui),
            fmt_bytes(self.mask.total()),
            fmt_bytes(self.video.total())
        )
    }
}
