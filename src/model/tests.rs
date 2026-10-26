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
        vector: None,
        blank: false,
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
        vec![(0, 10), (0, 40), (0, 55), (0, 70), (1, 10), (1, 40)]
    );
}

fn group_rids(doc: &DocState) -> Vec<Vec<String>> {
    let mut gs: Vec<((usize, i32, i32), Vec<String>)> = doc
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

fn replace_page_regions(
    doc: &mut DocState,
    page_idx: usize,
    bands: &[(i32, i32)],
) -> HashSet<String> {
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
    let rids: Vec<String> = doc
        .groups
        .iter()
        .flat_map(|g| g.region_ids.clone())
        .collect();
    assert_eq!(doc.groups.len(), 2);
    assert!(rids.iter().all(|id| id.starts_with("n0-")));
    assert!(doc
        .groups
        .iter()
        .all(|g| doc.group_top_key(g).0 != usize::MAX));
}

#[test]
fn upsert_page_groups_keeps_other_page_and_strips_cross_page_old_rids() {
    let mut doc = DocState::new();
    doc.pages.push(stub_page(400));
    doc.pages.push(stub_page(400));
    seed_bands(&mut doc, 0, &[(10, 20), (40, 50)]);
    seed_bands(&mut doc, 1, &[(10, 20), (40, 50)]);
    doc.groups
        .retain(|g| g.region_ids != ["r0-1".to_string()] && g.region_ids != ["r1-0".to_string()]);
    doc.groups.push(Group {
        id: "cross".into(),
        region_ids: vec!["r0-1".into(), "r1-0".into()],
        name: String::new(),
    });
    let old_ids = replace_page_regions(&mut doc, 0, &[(12, 22), (42, 52)]);
    doc.upsert_page_groups(0, &old_ids);
    assert!(doc
        .groups
        .iter()
        .all(|g| g.region_ids.iter().all(|id| doc.find_region(id).is_some())));
    assert!(doc
        .groups
        .iter()
        .any(|g| g.region_ids == ["r1-0".to_string()]));
    assert!(doc
        .groups
        .iter()
        .any(|g| g.region_ids == ["r1-1".to_string()]));
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
    assert_eq!(
        page_names(&doc),
        vec!["0.png", "4.png", "5.png", "1.png", "2.png", "3.png"]
    );
    assert_eq!(doc.pages.iter().position(|p| p.id == cur_id), Some(4));
}

#[test]
fn move_pages_block_inserts_before_anchor() {
    let mut doc = DocState::new();
    for i in 0..5 {
        doc.pages.push(named_stub(&format!("{i}.png")));
    }
    doc.move_pages_block(&[3, 4], 0, false);
    assert_eq!(
        page_names(&doc),
        vec!["3.png", "4.png", "0.png", "1.png", "2.png"]
    );
    assert_eq!(doc.current_page_index, 2);
}

#[test]
fn move_page_drops_outside_images_without_decoding_new_neighbors() {
    let mut doc = DocState::new();
    for _ in 0..12 {
        doc.pages.push(stub_page(40));
    }
    let px = Arc::new(image::RgbImage::from_raw(1, 1, vec![1, 2, 3]).unwrap());
    doc.pages[0].image = Some(px.clone());
    doc.pages[1].image = Some(px);
    seed_bands(&mut doc, 0, &[(10, 20)]);
    seed_bands(&mut doc, 1, &[(30, 40)]);
    doc.current_page_index = 0;
    doc.move_page(0, 11);
    assert_eq!(doc.current_page_index, 11);
    assert!(doc.pages[11].image.is_some());
    assert!(doc.pages[0].image.is_none());
    assert!(doc.pages[8].image.is_none());
    assert_eq!(group_y0s(&doc), vec![(0, 30), (11, 10)]);
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
    assert_eq!(
        doc.pages[doc.current_page_index].path.file_name().unwrap(),
        "4.png"
    );
    assert_eq!(doc.groups.len(), 1);

    doc.current_page_index = 3;
    let dead = doc.close_pages_at(&[3]);
    assert_eq!(dead.len(), 1);
    assert_eq!(page_names(&doc), vec!["0.png", "1.png", "4.png"]);
    assert_eq!(
        doc.pages[doc.current_page_index].path.file_name().unwrap(),
        "4.png"
    );
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
    assert!(
        g.lines[0] > mid,
        "单根应略低于画布中线: y={} mid={}",
        g.lines[0],
        mid
    );
    doc.apply_guides_global_off();
    assert!(!doc.guides_global);
    assert!(doc.get_group_guides(&gid).lines.is_empty());
    assert!(!doc.group_guide_defaults.is_empty());
    doc.apply_guides_global_on();
    assert_eq!(doc.get_group_guides(&gid).lines.len(), 1);
}

#[test]
fn render_final_mask_stays_on_scaled_stain() {
    // 小谱面要放大盖住目标页. 蒙版按编辑器习惯存在「预览画布 − 偏移」
    // 坐标系; 终稿先除以 content_scale 盖到未缩放拼合图, 再等比合成.
    let mut doc = DocState::new();
    doc.bg_aspect_w = 2560;
    doc.bg_aspect_h = 1440;
    let sw = 200u32;
    let sh = 250u32;
    let stain = (100u32, 100u32);
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
    doc.bg_image = Some(Arc::new(image::RgbImage::from_pixel(
        2560,
        1440,
        image::Rgb([10, 20, 30]),
    )));

    let frame = doc.group_preview_frame("g1").unwrap();
    assert!(frame.content_scale > 1.0);
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
            brush_radius: 0.0,
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
    let ghost_x = (frame.hoff as f32 + stored_x as f32 * frame.content_scale).round() as i64;
    let ghost_y = (frame.voff as f32 + stored_y as f32 * frame.content_scale).round() as i64;
    if ghost_x >= 0
        && ghost_y >= 0
        && (ghost_x as u32) < out.width()
        && (ghost_y as u32) < out.height()
        && (ghost_x as u32 != cx || ghost_y as u32 != cy)
    {
        let ghost = out.get_pixel(ghost_x as u32, ghost_y as u32);
        assert!(
            ghost[0] < 220,
            "旧偏移位置不该被蒙上, 得到 {ghost:?} at ({ghost_x},{ghost_y})"
        );
    }
}

#[test]
fn remap_masks_shifts_only_below_block() {
    let mut masks = vec![
        MaskRect {
            id: "a".into(),
            x0: 0,
            y0: 10,
            x1: 4,
            y1: 20,
            brush_points: vec![],
            brush_radius: 0.0,
            color: [255, 255, 255],
            poly_points: vec![],
            opacity: 1.0,
            bound_block: None,
        },
        MaskRect {
            id: "b".into(),
            x0: 0,
            y0: 80,
            x1: 4,
            y1: 90,
            brush_points: vec![],
            brush_radius: 0.0,
            color: [255, 255, 255],
            poly_points: vec![],
            opacity: 1.0,
            bound_block: None,
        },
    ];
    super::remap_masks_after_height_change(&mut masks, 0, 50, 20);
    assert_eq!(masks[0].y0, 10);
    assert_eq!(masks[1].y0, 50);
}

#[test]
fn group_heights_use_region_edit() {
    let mut doc = DocState::new();
    let mut page = stub_page(100);
    page.regions.insert(
        "r0".into(),
        Region {
            id: "r0".into(),
            page_id: page.id.clone(),
            y0: 0,
            y1: 79,
            kind: "gap".into(),
            color: "#e74c3c".into(),
        },
    );
    doc.pages.push(page);
    doc.groups.push(Group {
        id: "g0".into(),
        region_ids: vec!["r0".into()],
        name: String::new(),
    });
    doc.rebuild_rid_index();
    assert_eq!(doc.group_member_heights("g0")[0].1, 80);
    doc.region_edits.insert(
        "r0".into(),
        RegionEditMeta {
            kind: EditKind::Raster,
            canvas_w: 40,
            canvas_h: 22,
            paper_rgb: [250, 250, 250],
            source: None,
        },
    );
    assert_eq!(doc.group_member_heights("g0")[0].1, 22);
    assert_eq!(doc.group_sheet_width("g0"), 80);
}

#[test]
fn region_edit_detaches_when_crop_changes_and_reattaches() {
    let mut doc = DocState::new();
    let mut page = stub_page(100);
    let page_id = page.id.clone();
    page.regions.insert(
        "r0".into(),
        Region {
            id: "r0".into(),
            page_id: page_id.clone(),
            y0: 0,
            y1: 79,
            kind: "gap".into(),
            color: "#e74c3c".into(),
        },
    );
    doc.pages.push(page);
    doc.groups.push(Group {
        id: "g0".into(),
        region_ids: vec!["r0".into()],
        name: String::new(),
    });
    doc.rebuild_rid_index();
    doc.region_edits.insert(
        "r0".into(),
        RegionEditMeta {
            kind: EditKind::Raster,
            canvas_w: 40,
            canvas_h: 22,
            paper_rgb: [250, 250, 250],
            source: Some(photo_edit::SourceFingerprint {
                page_id: page_id.clone(),
                y0: 0,
                y1: 79,
                w: 80,
                h: 80,
                extra_top: 4,
                ..Default::default()
            }),
        },
    );
    assert!(doc.region_edit_source_current("r0"));
    assert!(!doc.region_edit_detached("r0"));
    assert_eq!(doc.group_member_heights("g0")[0].1, 22);
    doc.get_region_mut("r0").unwrap().y1 = 59;
    assert!(doc.region_edit_detached("r0"));
    assert!(!doc.region_edit_applies("r0"));
    assert_eq!(doc.group_member_heights("g0")[0].1, 60);
    doc.get_region_mut("r0").unwrap().y1 = 79;
    assert!(doc.region_edit_source_current("r0"));
    assert_eq!(doc.group_member_heights("g0")[0].1, 22);
}

#[test]
fn shared_footnote_edit_height_applies_to_all_groups() {
    let mut doc = DocState::new();
    let mut page = stub_page(100);
    page.regions.insert(
        "note".into(),
        Region {
            id: "note".into(),
            page_id: page.id.clone(),
            y0: 0,
            y1: 59,
            kind: "gap".into(),
            color: "#e74c3c".into(),
        },
    );
    page.regions.insert(
        "staff".into(),
        Region {
            id: "staff".into(),
            page_id: page.id.clone(),
            y0: 60,
            y1: 79,
            kind: "staff".into(),
            color: "#3498db".into(),
        },
    );
    doc.pages.push(page);
    doc.groups.push(Group {
        id: "g1".into(),
        region_ids: vec!["staff".into(), "note".into()],
        name: String::new(),
    });
    doc.groups.push(Group {
        id: "g2".into(),
        region_ids: vec!["note".into()],
        name: String::new(),
    });
    doc.rebuild_rid_index();
    doc.region_edits.insert(
        "note".into(),
        RegionEditMeta {
            kind: EditKind::Raster,
            canvas_w: 80,
            canvas_h: 18,
            paper_rgb: [250, 250, 250],
            source: None,
        },
    );
    assert_eq!(
        doc.group_member_heights("g1")
            .into_iter()
            .find(|(id, _)| id == "note")
            .map(|(_, h)| h),
        Some(18)
    );
    assert_eq!(doc.group_member_heights("g2")[0].1, 18);
}

fn spread_pages(n: usize) -> DocState {
    let mut doc = DocState::new();
    for i in 0..n {
        let mut page = stub_page(12);
        let image =
            image::RgbImage::from_pixel(page.img_w, page.img_h, image::Rgb([255, 255, 255]));
        page.disk_path =
            crate::page_cache::write_rgb_png(&image, &format!("spread-{i}.png")).unwrap();
        doc.pages.push(page);
    }
    doc.enter_spread();
    doc
}

fn paint_mask(doc: &mut DocState, group_index: usize) {
    let gid = doc.groups[group_index].id.clone();
    doc.group_masks.insert(
        gid,
        vec![MaskRect {
            id: format!("m{group_index}"),
            x0: 1,
            y0: 2,
            x1: 3,
            y1: 4,
            brush_points: vec![],
            brush_radius: 0.0,
            color: [255, 255, 255],
            poly_points: vec![],
            opacity: 1.0,
            bound_block: None,
        }],
    );
}

#[test]
fn spread_repair_keeps_mask_when_members_stay() {
    let mut doc = spread_pages(4);
    assert_eq!(doc.groups.len(), 2);
    assert!(doc.groups.iter().all(|g| g.region_ids.len() == 2));
    paint_mask(&mut doc, 0);
    paint_mask(&mut doc, 1);
    let g0 = doc.groups[0].id.clone();
    let g1 = doc.groups[1].id.clone();
    let pair0 = doc.groups[0].region_ids.clone();

    doc.close_pages_at(&[3]);
    assert_eq!(doc.groups.len(), 2);
    assert_eq!(doc.groups[0].id, g0);
    assert_eq!(doc.groups[0].region_ids, pair0);
    assert!(doc.group_masks.contains_key(&g0));
    assert_eq!(doc.groups[1].id, g1);
    assert_eq!(doc.groups[1].region_ids.len(), 1);
    assert!(!doc.group_masks.contains_key(&g1));

    let mut doc = spread_pages(4);
    paint_mask(&mut doc, 0);
    paint_mask(&mut doc, 1);
    let g0 = doc.groups[0].id.clone();
    let pair0 = doc.groups[0].region_ids.clone();
    let g1 = doc.groups[1].id.clone();
    assert!(doc.copy_page_at(2).is_some());
    assert_eq!(doc.pages.len(), 5);
    assert_eq!(doc.groups.len(), 3);
    assert_eq!(doc.groups[0].id, g0);
    assert_eq!(doc.groups[0].region_ids, pair0);
    assert!(doc.group_masks.contains_key(&g0));
    assert_eq!(doc.groups[1].id, g1);
    assert!(!doc.group_masks.contains_key(&g1));
    assert_eq!(doc.groups[2].region_ids.len(), 1);

    let mut doc = spread_pages(4);
    paint_mask(&mut doc, 0);
    paint_mask(&mut doc, 1);
    let ids: Vec<String> = doc.groups.iter().map(|g| g.id.clone()).collect();
    let members: Vec<Vec<String>> = doc.groups.iter().map(|g| g.region_ids.clone()).collect();
    assert!(doc.insert_blank_page(4).unwrap().is_none());
    assert!(doc.insert_blank_page(4).unwrap().is_none());
    assert_eq!(doc.pages.len(), 4);
    assert_eq!(doc.groups.len(), 2);
    assert_eq!(doc.groups[0].id, ids[0]);
    assert_eq!(doc.groups[1].id, ids[1]);
    assert_eq!(doc.groups[0].region_ids, members[0]);
    assert_eq!(doc.groups[1].region_ids, members[1]);
    assert!(doc.group_masks.contains_key(&ids[0]));
    assert!(doc.group_masks.contains_key(&ids[1]));
}

#[test]
fn spread_drops_a_book_of_two_blanks() {
    let mut doc = spread_pages(2);
    assert!(doc.insert_blank_page(0).unwrap().is_some());
    assert_eq!(doc.pages.len(), 3);
    assert!(doc.pages[0].blank);
    assert!(doc.insert_blank_page(0).unwrap().is_none());
    assert_eq!(doc.pages.len(), 2);
    assert!(doc.pages.iter().all(|p| !p.blank));
    assert_eq!(doc.groups.len(), 1);
    assert_eq!(doc.groups[0].region_ids.len(), 2);

    let mut doc = spread_pages(1);
    assert!(doc.insert_blank_page(1).unwrap().is_some());
    assert_eq!(doc.pages.len(), 2);
    assert!(doc.pages[1].blank);
    assert_eq!(doc.groups[0].region_ids.len(), 1);

    let mut doc = spread_pages(4);
    assert!(doc.insert_blank_page(2).unwrap().is_some());
    assert_eq!(doc.pages.len(), 5);
    assert!(doc.pages[2].blank);
    assert!(doc.insert_blank_page(3).unwrap().is_none());
    assert_eq!(doc.pages.len(), 4);
    assert!(doc.pages.iter().all(|p| !p.blank));
    assert_eq!(doc.groups.len(), 2);
}

#[test]
fn spread_caption_stays_with_the_file_not_the_slot() {
    let mut page = stub_page(10);
    page.path = PathBuf::from("sonata_p012.png");
    assert_eq!(page.stable_mark(), "原页 12");
    assert_eq!(page.spread_caption().0, "sonata_p012");
    page.path = PathBuf::from("sonata_p012_copy.png");
    assert_eq!(page.stable_mark(), "原页 12 · 复制");
    page.path = PathBuf::from("photo.png");
    page.img_w = 80;
    page.img_h = 10;
    let (title, meta) = page.spread_caption();
    assert_eq!(title, "photo");
    assert_eq!(meta, "80×10");
    page.path = PathBuf::from("blank_ab12.png");
    assert_eq!(page.spread_caption().0, "空白页");
}

#[test]
fn copy_pages_after_packs_in_given_order() {
    let mut doc = spread_pages(5);
    for (i, page) in doc.pages.iter_mut().enumerate() {
        page.path = PathBuf::from(format!("scan_p{i:03}.png"));
    }
    let new_ids = doc.copy_pages_beside(&[0, 3, 1], 1, true).unwrap();
    assert_eq!(new_ids.len(), 3);
    assert_eq!(
        page_names(&doc),
        vec![
            "scan_p000.png",
            "scan_p001.png",
            "scan_p000_copy.png",
            "scan_p003_copy.png",
            "scan_p001_copy.png",
            "scan_p002.png",
            "scan_p003.png",
            "scan_p004.png",
        ]
    );
    assert_eq!(doc.groups.len(), 4);
    assert!(doc.groups.iter().all(|g| g.region_ids.len() == 2));
    assert_eq!(doc.pages[2].id, new_ids[0]);
    assert_eq!(doc.current_page_index, 2);

    let mut doc = spread_pages(4);
    for (i, page) in doc.pages.iter_mut().enumerate() {
        page.path = PathBuf::from(format!("scan_p{i:03}.png"));
    }
    doc.copy_pages_beside(&[2, 0], 1, false).unwrap();
    assert_eq!(
        page_names(&doc),
        vec![
            "scan_p000.png",
            "scan_p002_copy.png",
            "scan_p000_copy.png",
            "scan_p001.png",
            "scan_p002.png",
            "scan_p003.png",
        ]
    );
}

#[test]
fn copy_pages_beside_in_staff_mode_adds_one_group_per_region() {
    let mut doc = DocState::new();
    for i in 0..3 {
        let mut page = stub_page(40);
        let image =
            image::RgbImage::from_pixel(page.img_w, page.img_h, image::Rgb([255, 255, 255]));
        page.disk_path =
            crate::page_cache::write_rgb_png(&image, &format!("staff-{i}.png")).unwrap();
        page.path = PathBuf::from(format!("scan_p{i:03}.png"));
        doc.pages.push(page);
    }
    seed_bands(&mut doc, 0, &[(0, 10), (12, 22)]);
    seed_bands(&mut doc, 1, &[(0, 8)]);
    let new_ids = doc.copy_pages_beside(&[0, 1], 1, true).unwrap();
    assert_eq!(new_ids.len(), 2);
    assert_eq!(doc.groups.len(), 6);
    for id in &new_ids {
        let page = doc.pages.iter().find(|p| &p.id == id).unwrap();
        for rid in page.regions.keys() {
            assert!(
                doc.groups
                    .iter()
                    .any(|g| g.region_ids.iter().any(|x| x == rid)),
                "region {rid} has no group"
            );
        }
    }
    let keys: Vec<usize> = doc.groups.iter().map(|g| doc.group_sort_key(g).0).collect();
    assert!(keys.windows(2).all(|w| w[0] <= w[1]), "{keys:?}");
}

fn group_holding<'a>(doc: &'a DocState, rid: &str) -> &'a Group {
    doc.groups
        .iter()
        .find(|g| g.region_ids.iter().any(|id| id == rid))
        .unwrap_or_else(|| panic!("no group holds {rid}"))
}

fn region_at_y(page: &Page, y0: i32) -> String {
    page.regions
        .values()
        .find(|r| r.y0 == y0)
        .map(|r| r.id.clone())
        .unwrap_or_else(|| panic!("no region at y0={y0}"))
}

/// 第 0 页一块和第 1 页第一块合成「跨页」. 第 1 页其余两块是「本页」.
fn cross_page_doc() -> (DocState, String, String, String, String) {
    let mut doc = DocState::new();
    for i in 0..2 {
        let mut page = stub_page(80);
        let image =
            image::RgbImage::from_pixel(page.img_w, page.img_h, image::Rgb([255, 255, 255]));
        page.disk_path =
            crate::page_cache::write_rgb_png(&image, &format!("cross-{i}.png")).unwrap();
        doc.pages.push(page);
    }
    let p0 = doc.pages[0].id.clone();
    let p1 = doc.pages[1].id.clone();
    let bands = [
        (0, "r0", 0, 10),
        (1, "r1", 0, 10),
        (1, "r2", 20, 30),
        (1, "r3", 40, 50),
    ];
    for (page_idx, rid, y0, y1) in bands {
        let page_id = if page_idx == 0 {
            p0.clone()
        } else {
            p1.clone()
        };
        doc.pages[page_idx].regions.insert(
            rid.to_string(),
            Region {
                id: rid.to_string(),
                page_id,
                y0,
                y1,
                kind: "staff".into(),
                color: COLORS[0].to_string(),
            },
        );
    }
    doc.groups.push(Group {
        id: "cross".into(),
        region_ids: vec!["r0".into(), "r1".into()],
        name: "跨页".into(),
    });
    doc.groups.push(Group {
        id: "local".into(),
        region_ids: vec!["r2".into(), "r3".into()],
        name: "本页".into(),
    });
    doc.group_masks.insert(
        "cross".into(),
        vec![
            MaskRect {
                id: "unbound".into(),
                x0: 1,
                y0: 2,
                x1: 3,
                y1: 4,
                brush_points: vec![],
                brush_radius: 0.0,
                color: [255, 255, 255],
                poly_points: vec![],
                opacity: 1.0,
                bound_block: None,
            },
            MaskRect {
                id: "bound-r1".into(),
                x0: 10,
                y0: 11,
                x1: 12,
                y1: 13,
                brush_points: vec![],
                brush_radius: 0.0,
                color: [1, 2, 3],
                poly_points: vec![],
                opacity: 1.0,
                bound_block: Some("r1".into()),
            },
        ],
    );
    doc.group_masks.insert(
        "local".into(),
        vec![MaskRect {
            id: "bound-r2".into(),
            x0: 20,
            y0: 21,
            x1: 22,
            y1: 23,
            brush_points: vec![],
            brush_radius: 0.0,
            color: [4, 5, 6],
            poly_points: vec![],
            opacity: 0.5,
            bound_block: Some("r2".into()),
        }],
    );
    doc.set_block_layout(
        "cross",
        vec![
            BlockAdjust {
                region_id: "r0".into(),
                extra_top: -1,
                ..Default::default()
            },
            BlockAdjust {
                region_id: "r1".into(),
                shift_x: 6,
                ..Default::default()
            },
        ],
    );
    doc.set_block_layout(
        "local",
        vec![
            BlockAdjust {
                region_id: "r2".into(),
                shift_x: 4,
                ..Default::default()
            },
            BlockAdjust {
                region_id: "r3".into(),
                extra_top: -2,
                ..Default::default()
            },
        ],
    );
    doc.set_group_voff_shift("cross", 12);
    doc.set_group_voff_shift("local", 5);
    doc.set_group_guides(
        "cross",
        GuideState {
            lines: vec![7],
            locked: false,
        },
    );
    doc.set_group_guides(
        "local",
        GuideState {
            lines: vec![9],
            locked: false,
        },
    );
    doc.group_guide_defaults.insert(
        "local".into(),
        GuideState {
            lines: vec![11],
            locked: false,
        },
    );
    doc.region_staff_anchors.insert("r3".into(), Some(3));
    doc.staff_line_atoms.insert(
        p1.clone(),
        vec![crate::staff_detect::LineBand {
            y0: 0,
            y1: 10,
            kind: "system".into(),
            anchor: Some(1),
        }],
    );
    let mut edit = photo_edit::EditDocument::from_rgb(
        &image::RgbImage::from_pixel(80, 11, image::Rgb([9, 8, 7])),
        [250, 250, 248],
    );
    edit.source = Some(photo_edit::SourceFingerprint {
        page_id: p1,
        y0: 20,
        y1: 30,
        w: 80,
        h: 11,
        ..Default::default()
    });
    doc.write_region_edit("r2", &edit).unwrap();
    doc.rebuild_rid_index();
    (doc, "r0".into(), "r1".into(), "r2".into(), "r3".into())
}

#[test]
fn copy_keeps_in_page_groups_and_splits_cross_page_blocks() {
    let (mut doc, r0, r1, r2, r3) = cross_page_doc();
    let new_ids = doc.copy_pages_beside(&[1], 1, true).unwrap();
    assert_eq!(doc.pages.len(), 3);
    assert_eq!(doc.groups.len(), 4);
    assert_eq!(doc.groups[0].region_ids, vec![r0.clone(), r1.clone()]);
    assert_eq!(doc.groups[0].name, "跨页");
    assert_eq!(doc.group_masks.get("cross").map(|m| m.len()), Some(2));
    assert_eq!(doc.get_group_voff_shift("cross"), 12);

    let page = doc.pages.iter().find(|p| p.id == new_ids[0]).unwrap();
    let a = region_at_y(page, 0);
    let b = region_at_y(page, 20);
    let c = region_at_y(page, 40);
    let solo = group_holding(&doc, &a);
    assert_eq!(solo.region_ids, vec![a.clone()]);
    assert!(solo.name.is_empty());
    assert!(!solo.region_ids.contains(&r0));
    assert!(!solo.region_ids.contains(&r1));
    let solo_id = solo.id.clone();
    let solo_masks = doc.get_group_masks(&solo_id);
    assert_eq!(solo_masks.len(), 1);
    assert_eq!(solo_masks[0].bound_block.as_deref(), Some(a.as_str()));
    assert_eq!(solo_masks[0].x0, 10);
    assert_ne!(solo_masks[0].id, "bound-r1");
    assert_eq!(doc.get_group_voff_shift(&solo_id), 0);
    assert!(doc.get_group_guides(&solo_id).lines.is_empty());
    assert_eq!(doc.get_block_layout(&solo_id)[0].shift_x, 6);
    assert_eq!(doc.get_block_layout(&solo_id)[0].region_id, a);

    let local = group_holding(&doc, &b);
    assert_eq!(local.name, "本页");
    assert_eq!(local.region_ids, vec![b.clone(), c.clone()]);
    assert_ne!(local.id, "local");
    let local_id = local.id.clone();
    assert_eq!(doc.get_group_voff_shift(&local_id), 5);
    assert_eq!(doc.get_group_guides(&local_id).lines, vec![9]);
    assert_eq!(
        doc.group_guide_defaults
            .get(&local_id)
            .map(|g| g.lines.clone()),
        Some(vec![11])
    );
    let masks = doc.get_group_masks(&local_id);
    assert_eq!(masks.len(), 1);
    assert_eq!(masks[0].bound_block.as_deref(), Some(b.as_str()));
    assert_eq!(masks[0].color, [4, 5, 6]);
    let layout = doc.get_block_layout(&local_id);
    assert_eq!(layout[0].region_id, b);
    assert_eq!(layout[0].shift_x, 4);
    assert_eq!(layout[1].region_id, c);
    assert_eq!(layout[1].extra_top, -2);

    assert!(doc.region_edit_applies(&b));
    assert_eq!(
        doc.region_edits
            .get(&b)
            .and_then(|m| m.source.as_ref())
            .map(|s| s.page_id.as_str()),
        Some(new_ids[0].as_str())
    );
    assert!(doc.region_edit_applies(&r2));
    assert_eq!(doc.region_staff_anchors.get(&c).copied(), Some(Some(3)));
    assert_eq!(doc.region_staff_anchors.get(&r3).copied(), Some(Some(3)));
    assert_eq!(
        doc.staff_line_atoms.get(&new_ids[0]).map(|v| v.len()),
        Some(1)
    );

    let (mut doc, r0, r1, _, _) = cross_page_doc();
    let new_ids = doc.copy_pages_beside(&[0, 1], 1, true).unwrap();
    assert_eq!(doc.groups.len(), 4);
    assert_eq!(doc.groups[0].id, "cross");
    assert_eq!(doc.groups[0].region_ids, vec![r0, r1]);
    let copy0 = doc.pages.iter().find(|p| p.id == new_ids[0]).unwrap();
    let copy1 = doc.pages.iter().find(|p| p.id == new_ids[1]).unwrap();
    let a0 = region_at_y(copy0, 0);
    let a1 = region_at_y(copy1, 0);
    let cross = group_holding(&doc, &a0);
    assert_eq!(cross.name, "跨页");
    assert_eq!(cross.region_ids, vec![a0.clone(), a1.clone()]);
    let gid = cross.id.clone();
    assert_eq!(doc.get_group_voff_shift(&gid), 12);
    assert_eq!(doc.get_group_guides(&gid).lines, vec![7]);
    let masks = doc.get_group_masks(&gid);
    assert_eq!(masks.len(), 2);
    assert!(masks.iter().any(|m| m.bound_block.is_none()));
    assert!(masks
        .iter()
        .any(|m| m.bound_block.as_deref() == Some(a1.as_str())));
}

fn seed_labeled(doc: &mut DocState, page_idx: usize, bands: &[(&str, i32, i32)]) {
    let page_id = doc.pages[page_idx].id.clone();
    for (i, &(kind, y0, y1)) in bands.iter().enumerate() {
        let rid = format!("r{page_idx}-{i}");
        doc.pages[page_idx].regions.insert(
            rid.clone(),
            Region {
                id: rid.clone(),
                page_id: page_id.clone(),
                y0,
                y1,
                kind: kind.into(),
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

#[test]
fn within_page_pack_joins_bounds_and_leaves_odd_tail() {
    let mut doc = DocState::new();
    doc.pages.push(stub_page(400));
    doc.pages.push(stub_page(400));
    seed_labeled(
        &mut doc,
        0,
        &[
            ("header", 0, 8),
            ("system", 20, 40),
            ("gap", 41, 48),
            ("system", 50, 70),
            ("system", 90, 110),
            ("system", 130, 150),
            ("system", 170, 190),
            ("footer", 210, 240),
        ],
    );
    seed_labeled(&mut doc, 1, &[("system", 20, 40), ("system", 60, 80)]);
    let applied = doc.apply_staff_pack(2, crate::staff_detect::StaffPackMode::WithinPage);
    assert_eq!(applied, StaffPackApply::Applied);
    let mut spans: Vec<_> = doc.pages[0]
        .regions
        .values()
        .map(|r| (r.kind.as_str(), r.y0, r.y1))
        .collect();
    spans.sort();
    assert_eq!(
        spans,
        vec![
            ("footer", 210, 240),
            ("header", 0, 8),
            ("system", 20, 70),
            ("system", 90, 150),
            ("system", 170, 190),
        ]
    );
    let page1: Vec<_> = doc.pages[1]
        .regions
        .values()
        .map(|r| (r.y0, r.y1))
        .collect();
    assert_eq!(page1, vec![(20, 80)]);
}

#[test]
fn across_pack_spills_to_next_page_and_skips_header_footer() {
    let mut doc = DocState::new();
    doc.pages.push(stub_page(400));
    doc.pages.push(stub_page(400));
    seed_labeled(
        &mut doc,
        0,
        &[
            ("header", 0, 8),
            ("system", 10, 20),
            ("system", 30, 40),
            ("system", 50, 60),
            ("system", 70, 80),
            ("system", 90, 100),
            ("footer", 120, 140),
        ],
    );
    seed_labeled(
        &mut doc,
        1,
        &[
            ("header", 0, 8),
            ("system", 10, 20),
            ("system", 30, 40),
            ("system", 50, 60),
            ("system", 70, 80),
            ("system", 90, 100),
            ("footer", 120, 140),
        ],
    );
    assert_eq!(
        doc.apply_staff_pack(2, crate::staff_detect::StaffPackMode::AcrossPages),
        StaffPackApply::Applied
    );
    let grouped = group_rids(&doc);
    assert_eq!(
        grouped,
        vec![
            vec!["r0-0".to_string()],
            vec!["r0-1".to_string(), "r0-2".to_string()],
            vec!["r0-3".to_string(), "r0-4".to_string()],
            vec!["r0-5".to_string(), "r1-1".to_string()],
            vec!["r0-6".to_string()],
            vec!["r1-0".to_string()],
            vec!["r1-2".to_string(), "r1-3".to_string()],
            vec!["r1-4".to_string(), "r1-5".to_string()],
            vec!["r1-6".to_string()],
        ]
    );
}
