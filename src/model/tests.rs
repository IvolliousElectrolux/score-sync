use super::*;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use mask_tool::mask::MaskRect;

fn stub_page(h: u32) -> Page {
    Page {
        id: new_id(),
        path: PathBuf::from("p.png"),
        disk_path: PathBuf::from("p.png"),
        image: None,
        img_w: 80,
        img_h: h,
        regions: HashMap::new(),
    }
}

fn seed_bands(doc: &mut DocState, page_idx: usize, bands: &[(i32, i32)]) {
    let page_id = doc.pages[page_idx].id.clone();
    for (i, &(y0, y1)) in bands.iter().enumerate() {
        let rid = format!("r{page_idx}-{i}");
        doc.pages[page_idx].regions.insert(
            rid.clone(),
            Region {
                id: rid.clone(),
                page_id: page_id.clone(),
                y0,
                y1,
                kind: "staff".into(),
                color: COLORS[i % COLORS.len()].to_string(),
            },
        );
        doc.groups.push(Group {
            id: format!("g{page_idx}-{i}"),
            region_ids: vec![rid],
            name: String::new(),
        });
    }
    doc.rebuild_rid_index();
}

fn group_y0s(doc: &DocState) -> Vec<(usize, i32)> {
    doc.groups
        .iter()
        .map(|g| {
            let k = doc.group_top_key(g);
            (k.0, k.1)
        })
        .collect()
}

#[test]
fn add_manual_block_inserts_by_top_y_when_manual_order() {
    let mut doc = DocState::new();
    doc.pages.push(stub_page(400));
    seed_bands(&mut doc, 0, &[(10, 20), (40, 50), (70, 80)]);
    doc.groups_manual_order = true;

    doc.add_manual_block(25, 30);
    assert_eq!(group_y0s(&doc), vec![(0, 10), (0, 25), (0, 40), (0, 70)]);

    doc.add_manual_block(1, 5);
    assert_eq!(
        group_y0s(&doc),
        vec![(0, 1), (0, 10), (0, 25), (0, 40), (0, 70)]
    );

    doc.add_manual_block(90, 95);
    assert_eq!(
        group_y0s(&doc),
        vec![(0, 1), (0, 10), (0, 25), (0, 40), (0, 70), (0, 90)]
    );
}

#[test]
fn add_manual_block_stays_on_its_page_between_neighbors() {
    let mut doc = DocState::new();
    doc.pages.push(stub_page(400));
    doc.pages.push(stub_page(400));
    seed_bands(&mut doc, 0, &[(10, 20), (40, 50), (70, 80)]);
    seed_bands(&mut doc, 1, &[(10, 20), (40, 50)]);
    doc.groups_manual_order = true;
    doc.current_page_index = 0;

    doc.add_manual_block(55, 60);
    assert_eq!(
        group_y0s(&doc),
        vec![
            (0, 10),
            (0, 40),
            (0, 55),
            (0, 70),
            (1, 10),
            (1, 40)
        ]
    );
}

fn group_rids(doc: &DocState) -> Vec<Vec<String>> {
    let mut gs: Vec<( (usize, i32, i32), Vec<String> )> = doc
        .groups
        .iter()
        .map(|g| {
            let mut rids = g.region_ids.clone();
            rids.sort_by_key(|rid| doc.region_sort_key(rid));
            (doc.group_top_key(g), rids)
        })
        .collect();
    gs.sort_by_key(|(k, _)| *k);
    gs.into_iter().map(|(_, r)| r).collect()
}

#[test]
fn pair_ungrouped_pairs_current_page_and_spills_odd_to_next() {
    let mut doc = DocState::new();
    doc.pages.push(stub_page(400));
    doc.pages.push(stub_page(400));
    seed_bands(
        &mut doc,
        0,
        &[(10, 20), (30, 40), (50, 60), (70, 80), (90, 100)],
    );
    seed_bands(
        &mut doc,
        1,
        &[(10, 20), (30, 40), (50, 60), (70, 80), (90, 100)],
    );
    doc.current_page_index = 0;
    assert_eq!(doc.pair_ungrouped().unwrap(), 3);
    assert_eq!(
        group_rids(&doc),
        vec![
            vec!["r0-0".to_string(), "r0-1".to_string()],
            vec!["r0-2".to_string(), "r0-3".to_string()],
            vec!["r0-4".to_string(), "r1-0".to_string()],
            vec!["r1-1".to_string()],
            vec!["r1-2".to_string()],
            vec!["r1-3".to_string()],
            vec!["r1-4".to_string()],
        ]
    );

    doc.current_page_index = 1;
    assert_eq!(doc.pair_ungrouped().unwrap(), 2);
    assert_eq!(
        group_rids(&doc),
        vec![
            vec!["r0-0".to_string(), "r0-1".to_string()],
            vec!["r0-2".to_string(), "r0-3".to_string()],
            vec!["r0-4".to_string(), "r1-0".to_string()],
            vec!["r1-1".to_string(), "r1-2".to_string()],
            vec!["r1-3".to_string(), "r1-4".to_string()],
        ]
    );
}

fn replace_page_regions(doc: &mut DocState, page_idx: usize, bands: &[(i32, i32)]) -> HashSet<String> {
    let old_ids: HashSet<String> = doc.pages[page_idx].regions.keys().cloned().collect();
    let page_id = doc.pages[page_idx].id.clone();
    doc.pages[page_idx].regions.clear();
    for (i, &(y0, y1)) in bands.iter().enumerate() {
        let rid = format!("n{page_idx}-{i}");
        doc.pages[page_idx].regions.insert(
            rid.clone(),
            Region {
                id: rid,
                page_id: page_id.clone(),
                y0,
                y1,
                kind: "system".into(),
                color: COLORS[i % COLORS.len()].to_string(),
            },
        );
    }
    doc.rebuild_rid_index();
    old_ids
}

#[test]
fn upsert_page_groups_drops_stale_rids_from_redetect() {
    let mut doc = DocState::new();
    doc.pages.push(stub_page(400));
    seed_bands(&mut doc, 0, &[(10, 20), (40, 50), (70, 80)]);
    let old_ids = replace_page_regions(&mut doc, 0, &[(12, 22), (42, 52)]);
    doc.upsert_page_groups(0, &old_ids);
    let rids: Vec<String> = doc.groups.iter().flat_map(|g| g.region_ids.clone()).collect();
    assert_eq!(doc.groups.len(), 2);
    assert!(rids.iter().all(|id| id.starts_with("n0-")));
    assert!(doc.groups.iter().all(|g| doc.group_top_key(g).0 != usize::MAX));
}

#[test]
fn upsert_page_groups_keeps_other_page_and_strips_cross_page_old_rids() {
    let mut doc = DocState::new();
    doc.pages.push(stub_page(400));
    doc.pages.push(stub_page(400));
    seed_bands(&mut doc, 0, &[(10, 20), (40, 50)]);
    seed_bands(&mut doc, 1, &[(10, 20), (40, 50)]);
    doc.groups.retain(|g| {
        g.region_ids != ["r0-1".to_string()] && g.region_ids != ["r1-0".to_string()]
    });
    doc.groups.push(Group {
        id: "cross".into(),
        region_ids: vec!["r0-1".into(), "r1-0".into()],
        name: String::new(),
    });
    let old_ids = replace_page_regions(&mut doc, 0, &[(12, 22), (42, 52)]);
    doc.upsert_page_groups(0, &old_ids);
    assert!(
        doc.groups
            .iter()
            .all(|g| g.region_ids.iter().all(|id| doc.find_region(id).is_some()))
    );
    assert!(doc.groups.iter().any(|g| g.region_ids == ["r1-0".to_string()]));
    assert!(doc.groups.iter().any(|g| g.region_ids == ["r1-1".to_string()]));
    assert_eq!(
        doc.groups
            .iter()
            .filter(|g| g.region_ids.iter().any(|id| id.starts_with("n0-")))
            .count(),
        2
    );
}

#[test]
fn prune_dangling_skips_while_some_page_has_no_regions() {
    let mut doc = DocState::new();
    doc.pages.push(stub_page(400));
    doc.pages.push(stub_page(400));
    seed_bands(&mut doc, 0, &[(10, 20)]);
    doc.groups.push(Group {
        id: "pending".into(),
        region_ids: vec!["r1-future".into()],
        name: String::new(),
    });
    doc.prune_dangling_groups_if_hydrated();
    assert!(doc.groups.iter().any(|g| g.id == "pending"));
    seed_bands(&mut doc, 1, &[(10, 20)]);
    doc.prune_dangling_groups_if_hydrated();
    assert!(!doc.groups.iter().any(|g| g.id == "pending"));
}

#[test]
fn page_width_stays_original_when_image_is_proxy() {
    let mut page = stub_page(6000);
    page.img_w = 4000;
    page.img_h = 6000;
    page.image = Some(Arc::new(image::RgbImage::from_pixel(
        400,
        600,
        image::Rgb([1, 2, 3]),
    )));
    assert!(!page.is_full_res());
    assert_eq!(page.width(), 4000);
    assert_eq!(page.height(), 6000);
    assert_eq!(page.estimated_bytes(), 400 * 600 * 3);
    assert_eq!(page.estimated_full_bytes(), 4000 * 6000 * 3);
}

#[test]
fn loaded_image_bytes_unique_shares_arc() {
    let img = Arc::new(image::RgbImage::from_pixel(8, 4, image::Rgb([1, 2, 3])));
    let mut doc = DocState::new();
    let mut a = stub_page(100);
    a.image = Some(img.clone());
    let mut b = stub_page(100);
    b.image = Some(img);
    doc.pages.push(a);
    doc.pages.push(b);
    let mut seen = std::collections::HashSet::new();
    let (n, bytes) = doc.loaded_image_bytes_unique(&mut seen);
    assert_eq!(n, 2);
    assert_eq!(bytes, 8 * 4 * 3);
}

#[test]
fn set_project_bg_arc_shares_pixels() {
    let mut doc = DocState::new();
    let img = Arc::new(image::RgbImage::from_pixel(4, 4, image::Rgb([1, 2, 3])));
    doc.set_project_bg_arc(img.clone(), None, 16, 9).unwrap();
    assert!(Arc::ptr_eq(doc.bg_image.as_ref().unwrap(), &img));
    let img2 = Arc::new(image::RgbImage::from_pixel(2, 2, image::Rgb([9, 9, 9])));
    doc.set_project_bg_arc(img2, None, 16, 9).unwrap();
    assert_eq!(doc.bg_image.as_ref().unwrap().dimensions(), (2, 2));
}

#[test]
fn unload_proxy_keeps_original_dims() {
    let mut doc = DocState::new();
    let mut page = stub_page(6000);
    page.img_w = 4000;
    page.img_h = 6000;
    page.image = Some(Arc::new(image::RgbImage::from_pixel(
        400,
        600,
        image::Rgb([1, 2, 3]),
    )));
    doc.pages.push(page);
    doc.unload_page_image(0);
    assert!(doc.pages[0].image.is_none());
    assert_eq!(doc.pages[0].width(), 4000);
    assert_eq!(doc.pages[0].height(), 6000);
}

#[test]
fn crop_region_maps_proxy_coordinates() {
    let mut doc = DocState::new();
    let mut page = stub_page(200);
    page.img_w = 80;
    page.img_h = 200;
    let mut img = image::RgbImage::from_pixel(40, 100, image::Rgb([10, 10, 10]));
    for y in 25..50 {
        for x in 0..40 {
            img.put_pixel(x, y, image::Rgb([200, 0, 0]));
        }
    }
    page.image = Some(Arc::new(img));
    let page_id = page.id.clone();
    doc.pages.push(page);
    let r0 = Region {
        id: "r0".into(),
        page_id,
        y0: 50,
        y1: 99,
        kind: "system".into(),
        color: "#e74c3c".into(),
    };
    doc.pages[0].regions.insert(r0.id.clone(), r0);
    doc.rebuild_rid_index();
    let band = doc.crop_region("r0").unwrap();
    assert_eq!(band.height(), 25);
    assert_eq!(*band.get_pixel(0, 0), image::Rgb([200, 0, 0]));
}

#[test]
fn compose_group_with_layout_applies_gap_and_extend() {
    let mut doc = DocState::new();
    let mut page = stub_page(100);
    page.image = Some(Arc::new(image::RgbImage::from_pixel(
        80,
        100,
        image::Rgb([250, 250, 250]),
    )));
    let page_id = page.id.clone();
    doc.pages.push(page);
    let r0 = Region {
        id: "r0".into(),
        page_id: page_id.clone(),
        y0: 0,
        y1: 29,
        kind: "system".into(),
        color: "#e74c3c".into(),
    };
    let r1 = Region {
        id: "r1".into(),
        page_id: page_id.clone(),
        y0: 30,
        y1: 59,
        kind: "system".into(),
        color: "#3498db".into(),
    };
    doc.pages[0].regions.insert(r0.id.clone(), r0);
    doc.pages[0].regions.insert(r1.id.clone(), r1);
    doc.rebuild_rid_index();
    doc.groups.push(Group {
        id: "g1".into(),
        region_ids: vec!["r0".into(), "r1".into()],
        name: String::new(),
    });

    // 无调整: 高度应等于两块之和 (各 30px).
    let plain = doc.compose_group("g1").unwrap();
    assert_eq!(plain.height(), 60);

    // r1 前插入 10px 间距, r1 底边再向外扩 5px.
    doc.set_block_layout(
        "g1",
        vec![
            BlockAdjust {
                region_id: "r0".into(),
                ..Default::default()
            },
            BlockAdjust {
                region_id: "r1".into(),
                extra_top: 0,
                extra_bottom: 5,
                gap_before: 10,
                ..Default::default()
            },
        ],
    );
    let adjusted = doc.compose_group("g1").unwrap();
    assert_eq!(adjusted.height(), 60 + 10 + 5);
    assert_eq!(adjusted.width(), plain.width());

    // 顶边向内裁掉 8px 应减小总高.
    doc.set_block_layout(
        "g1",
        vec![
            BlockAdjust {
                region_id: "r0".into(),
                extra_top: -8,
                ..Default::default()
            },
            BlockAdjust {
                region_id: "r1".into(),
                ..Default::default()
            },
        ],
    );
    let trimmed = doc.compose_group("g1").unwrap();
    assert_eq!(trimmed.height(), 60 - 8);

    // 全 0 调整应自动回退到「未设置」状态 (不占工程文件空间).
    doc.set_block_layout(
        "g1",
        vec![
            BlockAdjust {
                region_id: "r0".into(),
                ..Default::default()
            },
            BlockAdjust {
                region_id: "r1".into(),
                ..Default::default()
            },
        ],
    );
    assert!(doc.group_block_layout.get("g1").is_none());
}

fn named_stub(name: &str) -> Page {
    let mut p = stub_page(80);
    p.path = PathBuf::from(name);
    p
}

fn page_names(doc: &DocState) -> Vec<String> {
    doc.pages
        .iter()
        .map(|p| p.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn move_pages_block_inserts_after_anchor() {
    let mut doc = DocState::new();
    for i in 0..6 {
        doc.pages.push(named_stub(&format!("{i}.png")));
    }
    doc.current_page_index = 2;
    let cur_id = doc.pages[2].id.clone();
    doc.move_pages_block(&[1, 2, 3], 5, true);
    assert_eq!(page_names(&doc), vec!["0.png", "4.png", "5.png", "1.png", "2.png", "3.png"]);
    assert_eq!(doc.pages.iter().position(|p| p.id == cur_id), Some(4));
}

#[test]
fn move_pages_block_inserts_before_anchor() {
    let mut doc = DocState::new();
    for i in 0..5 {
        doc.pages.push(named_stub(&format!("{i}.png")));
    }
    doc.move_pages_block(&[3, 4], 0, false);
    assert_eq!(page_names(&doc), vec!["3.png", "4.png", "0.png", "1.png", "2.png"]);
    assert_eq!(doc.current_page_index, 2);
}

#[test]
fn close_pages_at_prefers_next_then_prev() {
    let mut doc = DocState::new();
    for i in 0..6 {
        doc.pages.push(named_stub(&format!("{i}.png")));
    }
    seed_bands(&mut doc, 2, &[(10, 20)]);
    seed_bands(&mut doc, 4, &[(10, 20)]);
    doc.current_page_index = 2;
    let dead = doc.close_pages_at(&[2, 3]);
    assert_eq!(dead.len(), 2);
    assert_eq!(page_names(&doc), vec!["0.png", "1.png", "4.png", "5.png"]);
    assert_eq!(doc.pages[doc.current_page_index].path.file_name().unwrap(), "4.png");
    assert_eq!(doc.groups.len(), 1);

    doc.current_page_index = 3;
    let dead = doc.close_pages_at(&[3]);
    assert_eq!(dead.len(), 1);
    assert_eq!(page_names(&doc), vec!["0.png", "1.png", "4.png"]);
    assert_eq!(doc.pages[doc.current_page_index].path.file_name().unwrap(), "4.png");
}

#[test]
fn close_page_at_matches_batch() {
    let mut doc = DocState::new();
    for i in 0..3 {
        doc.pages.push(named_stub(&format!("{i}.png")));
    }
    doc.current_page_index = 1;
    assert!(doc.close_page_at(1));
    assert_eq!(page_names(&doc), vec!["0.png", "2.png"]);
    assert_eq!(doc.current_page_index, 1);
}

#[test]
fn global_guides_switch_uses_precomputed_defaults_and_can_turn_off() {
    let mut doc = DocState::new();
    doc.pages.push(stub_page(1440));
    seed_bands(&mut doc, 0, &[(10, 200), (300, 490)]);
    doc.seed_guide_defaults();
    assert_eq!(doc.group_guide_defaults.len(), 2);
    doc.apply_guides_global_on();
    assert!(doc.guides_global);
    let gid = doc.groups[0].id.clone();
    let g = doc.get_group_guides(&gid);
    assert_eq!(g.lines.len(), 1);
    let h = doc.group_preview_frame(&gid).unwrap().canvas_h as i32;
    let mid = h / 2;
    assert!(g.lines[0] > mid, "单根应略低于画布中线: y={} mid={}", g.lines[0], mid);
    doc.apply_guides_global_off();
    assert!(!doc.guides_global);
    assert!(doc.get_group_guides(&gid).lines.is_empty());
    assert!(!doc.group_guide_defaults.is_empty());
    doc.apply_guides_global_on();
    assert_eq!(doc.get_group_guides(&gid).lines.len(), 1);
}

#[test]
fn render_final_mask_stays_on_scaled_stain() {
    // 高谱面会缩小装进 16:9 页面. 蒙版按编辑器习惯存在「预览画布 − 偏移」
    // 坐标系; 终稿先除以 content_scale 盖到未缩放拼合图, 再等比合成.
    let mut doc = DocState::new();
    doc.bg_aspect_w = 16;
    doc.bg_aspect_h = 9;
    let sw = 200u32;
    let sh = 250u32;
    let stain = (100u32, 200u32);
    let mut sheet = image::RgbImage::from_pixel(sw, sh, image::Rgb([180, 180, 180]));
    sheet.put_pixel(stain.0, stain.1, image::Rgb([255, 0, 0]));
    let mut page = stub_page(sh);
    page.img_w = sw;
    page.image = Some(Arc::new(sheet));
    let page_id = page.id.clone();
    doc.pages.push(page);
    doc.pages[0].regions.insert(
        "r0".into(),
        Region {
            id: "r0".into(),
            page_id,
            y0: 0,
            y1: (sh - 1) as i32,
            kind: "system".into(),
            color: "#e74c3c".into(),
        },
    );
    doc.rebuild_rid_index();
    doc.groups.push(Group {
        id: "g1".into(),
        region_ids: vec!["r0".into()],
        name: String::new(),
    });
    doc.bg_enabled = true;
    doc.bg_image = Some(Arc::new(image::RgbImage::from_pixel(800, 800, image::Rgb([10, 20, 30]))));

    let frame = doc.group_preview_frame("g1").unwrap();
    assert!(frame.content_scale < 1.0);
    let stored_x = ((stain.0 as f32) * frame.content_scale).round() as i32;
    let stored_y = ((stain.1 as f32) * frame.content_scale).round() as i32;
    doc.set_group_masks(
        "g1",
        vec![MaskRect {
            id: "cover".into(),
            x0: stored_x - 2,
            y0: stored_y - 2,
            x1: stored_x + 2,
            y1: stored_y + 2,
            brush_points: Vec::new(),
            brush_radius: 0,
            color: [255, 255, 255],
            poly_points: Vec::new(),
            opacity: 1.0,
            bound_block: None,
        }],
    );

    let out = doc.render_group_final("g1").unwrap().unwrap();
    let cx = (frame.hoff as i32 + stored_x).max(0) as u32;
    let cy = (frame.voff as i32 + stored_y).max(0) as u32;
    let covered = out.get_pixel(cx, cy);
    assert!(
        covered[0] > 200 && covered[1] > 200 && covered[2] > 200,
        "污点对应画布位置应为白蒙版, 得到 {covered:?} at ({cx},{cy})"
    );
    // 旧实现会把蒙版打在未缩放拼合图的 (stored_x, stored_y), 缩小后
    // 出现在更靠近原点处; 那里应仍是谱面灰, 不是白块.
    let ghost_x = (frame.hoff as f32 + stored_x as f32 * frame.content_scale).round() as u32;
    let ghost_y = (frame.voff as f32 + stored_y as f32 * frame.content_scale).round() as u32;
    if ghost_x != cx || ghost_y != cy {
        let ghost = out.get_pixel(ghost_x, ghost_y);
        assert!(
            ghost[0] < 220,
            "旧偏移位置不该被蒙上, 得到 {ghost:?} at ({ghost_x},{ghost_y})"
        );
    }
}
