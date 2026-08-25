//! 蒙版辅助线: 全局开启/同步位置/全局对齐, 以及工具栏右键菜单.

use super::*;
use mask_tool::gui::GuideHostCmd;
use mask_tool::layout::{self, BlockAdjust};

impl ScoreSyncApp {
    pub(super) fn handle_guide_host_cmd(&mut self, cx: &mut Context<Self>) {
        let cmd = self.mask_tool.update(cx, |m, _| m.take_guide_host_cmd());
        let Some(cmd) = cmd else {
            return;
        };
        match cmd {
            GuideHostCmd::EnableAll => self.guides_enable_all(cx),
            GuideHostCmd::DisableAll => self.guides_disable_all(cx),
            GuideHostCmd::SetGlobal(on) => {
                if on {
                    self.guides_enable_all(cx);
                } else {
                    self.guides_disable_all(cx);
                }
            }
            GuideHostCmd::SetSync(on) => {
                self.doc.guides_sync_positions = on;
                self.mask_tool.update(cx, |m, _| m.set_guide_prefs(self.doc.guides_global, on));
                self.mark_dirty();
                if on {
                    self.sync_guide_positions_from_current(cx);
                    self.status = "已开启同根数辅助线位置同步.".into();
                } else {
                    self.status = "已关闭同根数辅助线位置同步.".into();
                }
                cx.notify();
            }
            GuideHostCmd::AlignAll => self.guides_align_all(cx),
            GuideHostCmd::SyncPositions => self.sync_guide_positions_from_current(cx),
            GuideHostCmd::UndoGlobal(token) => self.undo_guide_global(Some(token), cx),
            GuideHostCmd::RedoGlobal(token) => self.redo_guide_global(Some(token), cx),
            GuideHostCmd::UndoGlobalFallback => self.undo_guide_global(None, cx),
            GuideHostCmd::RedoGlobalFallback => self.redo_guide_global(None, cx),
        }
    }

    fn groups_with_guides_count(&self) -> usize {
        self.doc
            .group_guides
            .values()
            .filter(|g| !g.lines.is_empty())
            .count()
    }

    pub(super) fn global_align_available(&self, cx: &Context<Self>) -> bool {
        if self.doc.guides_global {
            return true;
        }
        let m = self.mask_tool.read(cx);
        let current_on = m.guides_on();
        let mut n = self.groups_with_guides_count();
        if current_on {
            if let Some(gid) = self.mask_target.as_ref() {
                if self.doc.get_group_guides(gid).lines.is_empty() {
                    n += 1;
                }
            } else if n == 0 {
                n = 1;
            }
        }
        n >= 2
    }

    fn capture_mask_global_snap(&self) -> MaskGlobalSnap {
        MaskGlobalSnap {
            group_guides: self.doc.group_guides.clone(),
            guides_global: self.doc.guides_global,
            guides_sync_positions: self.doc.guides_sync_positions,
            group_block_layout: self.doc.group_block_layout.clone(),
            group_voff_shift: self.doc.group_voff_shift.clone(),
            group_masks: self.doc.group_masks.clone(),
        }
    }

    fn apply_mask_global_snap(&mut self, snap: MaskGlobalSnap) {
        self.doc.group_guides = snap.group_guides;
        self.doc.guides_global = snap.guides_global;
        self.doc.guides_sync_positions = snap.guides_sync_positions;
        self.doc.group_block_layout = snap.group_block_layout;
        self.doc.group_voff_shift = snap.group_voff_shift;
        self.doc.group_masks = snap.group_masks;
    }

    fn commit_guide_global(
        &mut self,
        cx: &mut Context<Self>,
        refresh_preview: bool,
        apply: impl FnOnce(&mut Self) -> SharedString,
    ) {
        self.flush_mask_to_doc(cx);
        let before = self.capture_mask_global_snap();
        let status = apply(self);
        let after = self.capture_mask_global_snap();
        let token = self.next_guide_token;
        self.next_guide_token = self.next_guide_token.wrapping_add(1);
        self.guide_undo.push(GuideHistEntry {
            token,
            before,
            after,
            refresh_preview,
        });
        if self.guide_undo.len() > CROP_HISTORY_LIMIT {
            self.guide_undo.remove(0);
        }
        self.guide_redo.clear();
        self.mask_tool.update(cx, |m, _| m.push_undo_with_host_token(token));
        self.mark_dirty();
        self.sync_current_mask_after_guides(cx, refresh_preview);
        self.status = status;
        cx.notify();
    }

    fn sync_current_mask_after_guides(&mut self, cx: &mut Context<Self>, refresh_preview: bool) {
        let global = self.doc.guides_global;
        let sync = self.doc.guides_sync_positions;
        let guides = self
            .mask_target
            .as_ref()
            .map(|gid| self.doc.get_group_guides(gid))
            .unwrap_or_default();
        self.mask_tool.update(cx, |m, _| {
            m.set_guide_prefs(global, sync);
            m.apply_live_guides(guides.clone());
        });
        if refresh_preview {
            self.refresh_mask_preview_keep_history(cx);
        }
    }

    fn undo_guide_global(&mut self, token: Option<u64>, cx: &mut Context<Self>) {
        let idx = if let Some(t) = token {
            self.guide_undo.iter().rposition(|e| e.token == t)
        } else {
            self.guide_undo.len().checked_sub(1)
        };
        let Some(i) = idx else {
            if token.is_none() {
                self.status = "没有可撤回的操作.".into();
                cx.notify();
            }
            return;
        };
        let entry = self.guide_undo.remove(i);
        let refresh = entry.refresh_preview;
        self.apply_mask_global_snap(entry.before.clone());
        if token.is_none() {
            self.mask_tool.update(cx, |m, _| m.purge_host_token(entry.token));
        }
        self.guide_redo.push(entry);
        self.mark_dirty();
        self.sync_current_mask_after_guides(cx, refresh);
        self.status = "已撤回全局辅助线操作.".into();
        cx.notify();
    }

    fn redo_guide_global(&mut self, token: Option<u64>, cx: &mut Context<Self>) {
        let idx = if let Some(t) = token {
            self.guide_redo.iter().rposition(|e| e.token == t)
        } else {
            self.guide_redo.len().checked_sub(1)
        };
        let Some(i) = idx else {
            if token.is_none() {
                self.status = "没有可重做的操作.".into();
                cx.notify();
            }
            return;
        };
        let entry = self.guide_redo.remove(i);
        let refresh = entry.refresh_preview;
        self.apply_mask_global_snap(entry.after.clone());
        if token.is_none() {
            self.mask_tool.update(cx, |m, _| m.purge_host_token(entry.token));
        }
        self.guide_undo.push(entry);
        if self.guide_undo.len() > CROP_HISTORY_LIMIT {
            self.guide_undo.remove(0);
        }
        self.mark_dirty();
        self.sync_current_mask_after_guides(cx, refresh);
        self.status = "已重做全局辅助线操作.".into();
        cx.notify();
    }

    fn guides_enable_all(&mut self, cx: &mut Context<Self>) {
        self.commit_guide_global(cx, false, |this| {
            this.doc.apply_guides_global_on();
            let n = this.doc.groups.len();
            format!("已全局开启辅助线 ({n} 个组合).").into()
        });
    }

    fn guides_disable_all(&mut self, cx: &mut Context<Self>) {
        self.commit_guide_global(cx, false, |this| {
            this.doc.apply_guides_global_off();
            "已关闭全部组合的辅助线.".into()
        });
    }

    fn sync_guide_positions_from_current(&mut self, cx: &mut Context<Self>) {
        self.flush_mask_to_doc(cx);
        let Some(src_gid) = self.mask_target.clone() else {
            return;
        };
        let src = self.doc.get_group_guides(&src_gid);
        if src.lines.is_empty() {
            return;
        }
        let src_n = src.lines.len();
        let src_h = self
            .doc
            .group_preview_frame(&src_gid)
            .map(|f| f.canvas_h as i32)
            .unwrap_or(0);
        if src_h <= 0 {
            return;
        }
        let gids: Vec<String> = self.doc.groups.iter().map(|g| g.id.clone()).collect();
        let mut n = 0u32;
        for gid in &gids {
            if gid == &src_gid {
                continue;
            }
            let dst = self.doc.get_group_guides(gid);
            if dst.lines.len() != src_n {
                continue;
            }
            let Some(frame) = self.doc.group_preview_frame(gid) else {
                continue;
            };
            let scaled = src.scaled_to(src_h, frame.canvas_h as i32);
            self.doc.set_group_guides(gid, scaled);
            n += 1;
        }
        if n > 0 {
            self.mark_dirty();
            self.status = format!("已同步辅助线位置到 {n} 个同样根数的组合.").into();
        }
        cx.notify();
    }

    pub(super) fn cancel_align_all(&mut self) {
        if self.align_all_running {
            self.align_all_running = false;
            self.align_all_gen = self.align_all_gen.wrapping_add(1);
        }
    }

    fn guides_align_all(&mut self, cx: &mut Context<Self>) {
        if !self.global_align_available(cx) {
            self.status = "需要多个组合有辅助线, 或已勾选全局开启, 才能全局对齐.".into();
            cx.notify();
            return;
        }
        self.flush_mask_to_doc(cx);
        if self.doc.guided_groups_anchors_ready() {
            self.commit_guide_global(cx, true, |this| this.apply_align_all_from_cache());
            return;
        }
        self.start_align_all_async(cx);
    }

    fn apply_align_all_from_cache(&mut self) -> SharedString {
        let gids: Vec<String> = self.doc.groups.iter().map(|g| g.id.clone()).collect();
        let mut n = 0u32;
        for gid in &gids {
            if self.align_group_from_cache(gid) {
                n += 1;
            }
        }
        format!("已全局对齐 {n} 个组合.").into()
    }

    fn align_group_from_cache(&mut self, gid: &str) -> bool {
        let guides = self.doc.get_group_guides(gid);
        if guides.lines.is_empty() {
            return false;
        }
        let Some(anchors) = self.doc.block_align_anchors_for_group(gid) else {
            return false;
        };
        let Some(frame) = self.doc.group_preview_frame(gid) else {
            return false;
        };
        let heights = self.doc.group_member_heights(gid);
        if heights.is_empty() {
            return false;
        }
        let assignments = mask_tool::staff::assignments_for_guides(&anchors, &guides.lines);
        if assignments.is_empty() {
            return false;
        }
        let layout = self.doc.get_block_layout(gid).to_vec();
        let voff_i32 = frame.voff.max(0).min(i32::MAX as i64) as i32;
        let page_h = if frame.shows_bg { frame.canvas_h as i32 } else { 0 };
        let (new_layout, voff_delta) = layout::align_blocks_to_targets(
            &heights,
            &layout,
            voff_i32,
            &assignments,
            page_h,
        );
        self.apply_group_align_result(gid, &heights, &layout, new_layout, frame.voff, voff_delta)
    }

    fn apply_group_align_result(
        &mut self,
        gid: &str,
        heights: &[(String, u32)],
        old_layout: &[BlockAdjust],
        new_layout: Vec<BlockAdjust>,
        voff: i64,
        voff_delta: i32,
    ) -> bool {
        if new_layout == old_layout && voff_delta == 0 {
            return false;
        }
        self.shift_group_masks_sheet(gid, heights, old_layout, &new_layout);
        let new_voff_target = voff + voff_delta as i64;
        let new_shift = self.resolve_group_voff_shift_for(gid, &new_layout, new_voff_target);
        self.doc.set_block_layout(gid, new_layout);
        self.doc.set_group_voff_shift(gid, new_shift);
        self.mark_video_pool_dirty_group(gid);
        true
    }

    fn start_align_all_async(&mut self, cx: &mut Context<Self>) {
        self.align_all_gen = self.align_all_gen.wrapping_add(1);
        let gen = self.align_all_gen;
        self.align_all_running = true;
        let job = self.build_align_all_job();
        let n_pages = job.pages.len();
        self.status = if n_pages > 0 {
            format!("正在后台全局对齐 ({n_pages} 页需读图)…").into()
        } else {
            "正在后台全局对齐…".into()
        };
        self.hint = self.status.clone();
        cx.notify();
        let (tx, rx) = async_channel::bounded::<AlignAllResult>(1);
        std::thread::spawn(move || {
            let _ = tx.send_blocking(run_align_all_job(job));
        });
        cx.spawn(async move |this, cx| {
            let Ok(result) = rx.recv().await else {
                return;
            };
            this.update(cx, |view, cx| {
                view.finish_align_all(gen, result, cx);
            })
            .ok();
        })
        .detach();
    }

    fn build_align_all_job(&self) -> AlignAllJob {
        let mut pages: HashMap<PathBuf, Vec<(String, i32, i32)>> = HashMap::new();
        let mut groups = Vec::new();
        for g in &self.doc.groups {
            if self.doc.get_group_guides(&g.id).lines.is_empty() {
                continue;
            }
            let Some(frame) = self.doc.group_preview_frame(&g.id) else {
                continue;
            };
            let heights = self.doc.group_member_heights(&g.id);
            if heights.is_empty() {
                continue;
            }
            for rid in &g.region_ids {
                if self.doc.region_staff_anchors.contains_key(rid) {
                    continue;
                }
                if let Some((pi, r)) = self.doc.find_region(rid) {
                    if let Some(page) = self.doc.pages.get(pi) {
                        pages
                            .entry(page.disk_path.clone())
                            .or_default()
                            .push((rid.clone(), r.y0, r.y1));
                    }
                }
            }
            groups.push(AlignGroupSpec {
                gid: g.id.clone(),
                heights,
                layout: self.doc.get_block_layout(&g.id).to_vec(),
                guide_lines: self.doc.get_group_guides(&g.id).lines.clone(),
                voff: frame.voff,
                page_h: if frame.shows_bg { frame.canvas_h as i32 } else { 0 },
            });
        }
        AlignAllJob {
            threshold: self.doc.ink_threshold,
            cached: self.doc.region_staff_anchors.clone(),
            pages: pages.into_iter().collect(),
            groups,
        }
    }

    fn finish_align_all(
        &mut self,
        gen: u64,
        result: AlignAllResult,
        cx: &mut Context<Self>,
    ) {
        if self.align_all_gen != gen {
            return;
        }
        self.align_all_running = false;
        self.doc.ingest_region_staff_anchors(result.new_anchors);
        let n_fail = result.n_fail;
        let groups = result.groups;
        self.commit_guide_global(cx, true, move |this| {
            let mut n = 0u32;
            for g in groups {
                let heights = this.doc.group_member_heights(&g.gid);
                let old_layout = this.doc.get_block_layout(&g.gid).to_vec();
                if this.apply_group_align_result(
                    &g.gid,
                    &heights,
                    &old_layout,
                    g.layout,
                    g.voff,
                    g.voff_delta,
                ) {
                    n += 1;
                }
            }
            if n_fail > 0 {
                format!("已全局对齐 {n} 个组合 ({n_fail} 个未能加载).").into()
            } else {
                format!("已全局对齐 {n} 个组合.").into()
            }
        });
    }

    pub(super) fn start_seed_align_anchors(&mut self, cx: &mut Context<Self>) {
        let gen = self.hydrate_gen;
        let thr = self.doc.ink_threshold;
        let mut jobs: Vec<(PathBuf, Vec<(String, i32, i32)>)> = Vec::new();
        for p in &self.doc.pages {
            let missing: Vec<(String, i32, i32)> = p
                .regions
                .values()
                .filter(|r| !self.doc.region_staff_anchors.contains_key(&r.id))
                .map(|r| (r.id.clone(), r.y0, r.y1))
                .collect();
            if missing.is_empty() {
                continue;
            }
            if p.image.is_some() {
                continue;
            }
            jobs.push((p.disk_path.clone(), missing));
        }
        for i in 0..self.doc.pages.len() {
            if self.doc.pages[i].image.is_some() {
                self.doc.seed_region_anchors_for_page(i);
            }
        }
        if jobs.is_empty() {
            return;
        }
        let (tx, rx) = async_channel::unbounded::<Vec<(String, Option<i32>)>>();
        std::thread::spawn(move || {
            for (path, bands) in jobs {
                let mut out = Vec::new();
                if let Ok(img) = crate::page_cache::load_rgb(&path) {
                    for (rid, y0, y1) in bands {
                        out.push((
                            rid,
                            mask_tool::staff::band_staff_anchor(&img, y0, y1, thr),
                        ));
                    }
                }
                let _ = tx.send_blocking(out);
            }
        });
        cx.spawn(async move |this, cx| {
            while let Ok(items) = rx.recv().await {
                this.update(cx, |view, _cx| {
                    if view.hydrate_gen != gen {
                        return;
                    }
                    view.doc.ingest_region_staff_anchors(items);
                })
                .ok();
            }
        })
        .detach();
    }

    fn resolve_group_voff_shift_for(
        &self,
        group_id: &str,
        layout: &[BlockAdjust],
        voff_target: i64,
    ) -> i64 {
        if !self.doc.bg_enabled {
            return 0;
        }
        let Some(bg) = self.doc.bg_image.as_ref() else {
            return 0;
        };
        let heights = self.doc.group_member_heights(group_id);
        if heights.is_empty() {
            return 0;
        }
        let sw = self.doc.group_sheet_width(group_id);
        let sh = layout::sheet_height(&heights, layout);
        let natural = apply_bg::process::natural_voff(
            sw,
            sh,
            bg.width(),
            bg.height(),
            self.doc.bg_aspect_w,
            self.doc.bg_aspect_h,
        );
        voff_target - natural
    }

    fn shift_group_masks_sheet(
        &mut self,
        gid: &str,
        heights: &[(String, u32)],
        old_layout: &[BlockAdjust],
        new_layout: &[BlockAdjust],
    ) {
        let deltas = layout::block_content_shifts(heights, old_layout, new_layout);
        if deltas.is_empty() {
            return;
        }
        let old_spans = layout::compute_spans(heights, old_layout);
        let Some(masks) = self.doc.group_masks.get_mut(gid) else {
            return;
        };
        for m in masks {
            let bound = m
                .bound_block
                .as_ref()
                .filter(|b| heights.iter().any(|(id, _)| id == *b))
                .cloned();
            let target = bound.or_else(|| {
                let cy = (m.y0 + m.y1) as f32 / 2.0;
                old_spans
                    .iter()
                    .find(|(_, y0, y1)| (*y0 as f32) <= cy && cy <= (*y1 as f32))
                    .map(|(rid, ..)| rid.clone())
            });
            if let Some(rid) = target {
                if let Some(&d) = deltas.get(&rid) {
                    m.offset_y(d);
                }
            }
        }
    }

    pub(super) fn guide_context_menu_overlay(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (guide_menu, align_menu, global, sync, guides_on, staff_n, block_n, guide_n) = {
            let m = self.mask_tool.read(cx);
            (
                m.guide_menu(),
                m.align_menu(),
                m.guides_global(),
                m.guides_sync(),
                m.guides_on(),
                m.staff_block_count(),
                m.block_count(),
                m.guide_count(),
            )
        };
        if let Some((x, y)) = guide_menu {
            return self
                .guide_menu_panel(x, y, global, sync, guides_on, staff_n, block_n, guide_n, cx)
                .into_any_element();
        }
        if let Some((x, y)) = align_menu {
            return self.align_menu_panel(x, y, cx).into_any_element();
        }
        div().into_any_element()
    }

    fn menu_shell(
        &self,
        child: impl IntoElement,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id("guide-ctx-backdrop")
            .absolute()
            .inset_0()
            // 阻断背后命中; move/up/滚轮也不让冒泡到工具栏和蒙版面板
            .occlude()
            .on_scroll_wheel(cx.listener(|_, _, _, cx| {
                cx.stop_propagation();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.mask_tool.update(cx, |m, _| m.close_guide_menus());
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, _, cx| {
                    this.mask_tool.update(cx, |m, _| m.close_guide_menus());
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|_, _, _, cx| {
                cx.stop_propagation();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .child(child)
    }

    fn menu_item_label(checked: bool, text: &str) -> SharedString {
        if checked {
            format!("✓  {text}").into()
        } else {
            format!("    {text}").into()
        }
    }

    fn guide_menu_panel(
        &self,
        x: f32,
        y: f32,
        global: bool,
        sync: bool,
        guides_on: bool,
        staff_n: usize,
        block_n: usize,
        guide_n: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let show_count = guides_on && block_n > staff_n && staff_n > 0;
        let can_dec = show_count && guide_n > staff_n;
        let can_inc = show_count && guide_n < block_n;
        let mut menu = div()
            .id("guide-ctx-menu")
            .absolute()
            .left(px(x))
            .top(px(y))
            .min_w(px(196.))
            .py_1()
            .rounded_md()
            .bg(rgb(0xffffff))
            .border_1()
            .border_color(rgb(0x94a3b8))
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .child(
                div()
                    .id("guide-ctx-global")
                    .px_3()
                    .py_1()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(0xdbeafe)))
                    .child(Self::menu_item_label(global, "全局开启"))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.mask_tool
                                .update(cx, |m, cx| m.request_set_global(!global, cx));
                        }),
                    ),
            )
            .child(
                div()
                    .id("guide-ctx-sync")
                    .px_3()
                    .py_1()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(0xdbeafe)))
                    .child(Self::menu_item_label(sync, "同步同根数位置"))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.mask_tool
                                .update(cx, |m, cx| m.request_set_sync(!sync, cx));
                        }),
                    ),
            );
        if show_count {
            menu = menu.child(
                div()
                    .mt_1()
                    .pt_1()
                    .border_t_1()
                    .border_color(rgb(0xe2e8f0))
                    .px_3()
                    .py_1()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x475569))
                            .child("当前页根数"),
                    )
                    .child(self.guide_count_step_btn("guide-cnt-dec", "−", can_dec, false, cx))
                    .child(
                        div()
                            .min_w(px(20.))
                            .text_sm()
                            .text_color(rgb(0x0f172a))
                            .child(format!("{guide_n}")),
                    )
                    .child(self.guide_count_step_btn("guide-cnt-inc", "+", can_inc, true, cx)),
            );
        }
        self.menu_shell(menu, cx)
    }

    fn guide_count_step_btn(
        &self,
        id: &'static str,
        label: &'static str,
        enabled: bool,
        inc: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let fg = if enabled { rgb(0x0f172a) } else { rgb(0x94a3b8) };
        let mut el = div()
            .id(id)
            .w(px(22.))
            .h(px(22.))
            .rounded_sm()
            .flex()
            .items_center()
            .justify_center()
            .border_1()
            .border_color(rgb(0xcbd5e1))
            .text_color(fg)
            .child(label);
        if enabled {
            el = el.cursor_pointer().hover(|s| s.bg(rgb(0xe2e8f0))).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.mask_tool.update(cx, |m, cx| {
                        let n = m.guide_count() as u32;
                        let next = if inc { n.saturating_add(1) } else { n.saturating_sub(1) };
                        m.set_guide_count(next, cx);
                    });
                }),
            );
        }
        el
    }

    fn align_menu_panel(&self, x: f32, y: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let can_global = self.global_align_available(cx);
        let global_fg = if can_global {
            rgb(0x0f172a)
        } else {
            rgb(0x94a3b8)
        };
        let mut global_row = div()
            .id("align-ctx-global")
            .px_3()
            .py_1()
            .text_color(global_fg)
            .child("全局对齐");
        if can_global {
            global_row = global_row.cursor_pointer().hover(|s| s.bg(rgb(0xdbeafe))).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.mask_tool.update(cx, |m, cx| m.request_align_all(cx));
                }),
            );
        }
        self.menu_shell(
            div()
                .id("align-ctx-menu")
                .absolute()
                .left(px(x))
                .top(px(y))
                .min_w(px(168.))
                .py_1()
                .rounded_md()
                .bg(rgb(0xffffff))
                .border_1()
                .border_color(rgb(0x94a3b8))
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_, _, _, cx| {
                        cx.stop_propagation();
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|_, _, _, cx| {
                        cx.stop_propagation();
                    }),
                )
                .child(global_row)
                .child(
                    div()
                        .id("align-ctx-reset")
                        .px_3()
                        .py_1()
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(0xdbeafe)))
                        .child("还原初始状态")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.mask_tool.update(cx, |m, cx| m.reset_block_layout(cx));
                            }),
                        ),
                ),
            cx,
        )
    }
}

struct AlignGroupSpec {
    gid: String,
    heights: Vec<(String, u32)>,
    layout: Vec<BlockAdjust>,
    guide_lines: Vec<i32>,
    voff: i64,
    page_h: i32,
}

struct AlignAllJob {
    threshold: i32,
    cached: HashMap<String, Option<i32>>,
    pages: Vec<(PathBuf, Vec<(String, i32, i32)>)>,
    groups: Vec<AlignGroupSpec>,
}

struct AlignGroupResult {
    gid: String,
    layout: Vec<BlockAdjust>,
    voff: i64,
    voff_delta: i32,
}

struct AlignAllResult {
    groups: Vec<AlignGroupResult>,
    new_anchors: Vec<(String, Option<i32>)>,
    n_fail: u32,
}

fn run_align_all_job(job: AlignAllJob) -> AlignAllResult {
    let mut anchors = job.cached;
    let mut n_fail = 0u32;
    for (path, bands) in job.pages {
        match crate::page_cache::load_rgb(&path) {
            Ok(img) => {
                for (rid, y0, y1) in bands {
                    let y = mask_tool::staff::band_staff_anchor(&img, y0, y1, job.threshold);
                    anchors.insert(rid, y);
                }
            }
            Err(_) => {}
        }
    }
    let new_anchors: Vec<(String, Option<i32>)> = anchors
        .iter()
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    let mut groups = Vec::new();
    for g in job.groups {
        if let Some(r) = align_group_spec(&g, &anchors) {
            groups.push(r);
        } else {
            n_fail += 1;
        }
    }
    AlignAllResult {
        groups,
        new_anchors,
        n_fail,
    }
}

fn align_group_spec(
    g: &AlignGroupSpec,
    anchors: &HashMap<String, Option<i32>>,
) -> Option<AlignGroupResult> {
    let spans = layout::compute_spans(&g.heights, &g.layout);
    let mut block_anchors = Vec::with_capacity(spans.len());
    for (rid, y0, y1) in spans {
        let piece_y = anchors.get(&rid).copied().flatten();
        let extra = BlockAdjust::find(&g.layout, &rid)
            .map(|a| a.extra_top)
            .unwrap_or(0);
        let span_h = (y1 - y0 + 1) as i32;
        block_anchors.push(mask_tool::staff::block_anchor_from_piece_y(
            rid, piece_y, extra, span_h,
        ));
    }
    let assignments = mask_tool::staff::assignments_for_guides(&block_anchors, &g.guide_lines);
    if assignments.is_empty() {
        return Some(AlignGroupResult {
            gid: g.gid.clone(),
            layout: g.layout.clone(),
            voff: g.voff,
            voff_delta: 0,
        });
    }
    let voff_i32 = g.voff.max(0).min(i32::MAX as i64) as i32;
    let (new_layout, voff_delta) = layout::align_blocks_to_targets(
        &g.heights,
        &g.layout,
        voff_i32,
        &assignments,
        g.page_h,
    );
    Some(AlignGroupResult {
        gid: g.gid.clone(),
        layout: new_layout,
        voff: g.voff,
        voff_delta,
    })
}
