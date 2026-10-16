//! 图层里的对象树: 命名, 组合, 展开后的选择窗格.

use std::collections::{HashMap, HashSet};

use crate::document::{Group, LayerRuntime, Obj, ObjKind, VecDocument};
use crate::ids::new_id;

/// 取消组合时记下的成员, 给重新组合用. 只活在这次编辑里.
#[derive(Clone, Debug)]
pub struct DissolvedGroup {
    pub layer_id: String,
    pub name: String,
    pub children: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Layer,
    Group,
    Object,
}

#[derive(Clone, Debug)]
pub struct OutlineRow {
    pub id: String,
    pub layer_index: usize,
    pub kind: RowKind,
    pub depth: u8,
    pub name: String,
    pub visible: bool,
    /// 自己和所有上级都开着, 才会画到画布上.
    pub shown: bool,
    pub expandable: bool,
    pub open: bool,
    pub ancestors: Vec<String>,
    pub ink: Option<[u8; 4]>,
}

pub fn flatten(doc: &VecDocument, expanded: &HashSet<String>) -> Vec<OutlineRow> {
    let mut rows = Vec::new();
    for (storage, layer) in doc.layers.iter().enumerate().rev() {
        let expandable = !layer.objects.is_empty() || !layer.groups.is_empty();
        rows.push(OutlineRow {
            id: layer.id.clone(),
            layer_index: storage,
            kind: RowKind::Layer,
            depth: 0,
            name: format!("{} ({})", layer.name, layer.objects.len()),
            visible: layer.visible,
            shown: layer.visible,
            expandable,
            open: expanded.contains(&layer.id),
            ancestors: Vec::new(),
            ink: layer.objects.iter().find_map(|o| crate::document::obj_ink(o)),
        });
        if expandable && expanded.contains(&layer.id) {
            let roots = roots_bottom_to_top(layer);
            let top: Vec<String> = roots.into_iter().rev().collect();
            push_level(layer, storage, &top, 1, &[], expanded, &mut rows);
        }
    }
    rows
}

fn push_level(
    layer: &LayerRuntime,
    layer_index: usize,
    ids_top_first: &[String],
    depth: u8,
    ancestors: &[String],
    expanded: &HashSet<String>,
    rows: &mut Vec<OutlineRow>,
) {
    let bottom: Vec<String> = ids_top_first.iter().rev().cloned().collect();
    let names = name_map(layer, &bottom);
    for id in ids_top_first {
        if let Some(group) = layer.groups.iter().find(|g| g.id == *id) {
            let leaf_n = descendant_object_ids(layer, id).len();
            let name = names.get(id).cloned().unwrap_or_else(|| group.name.clone());
            let label = if leaf_n > 1 {
                format!("{name} ({leaf_n})")
            } else {
                name
            };
            rows.push(OutlineRow {
                id: group.id.clone(),
                layer_index,
                kind: RowKind::Group,
                depth,
                name: label,
                visible: group.visible,
                shown: chain_shown(layer, ancestors, group.visible),
                expandable: !group.children.is_empty(),
                open: expanded.contains(&group.id),
                ancestors: ancestors.to_vec(),
                ink: first_ink(layer, id),
            });
            if expanded.contains(&group.id) {
                let mut next = ancestors.to_vec();
                next.push(group.id.clone());
                let kids: Vec<String> = group.children.iter().rev().cloned().collect();
                push_level(
                    layer,
                    layer_index,
                    &kids,
                    depth.saturating_add(1),
                    &next,
                    expanded,
                    rows,
                );
            }
        } else if let Some(obj) = layer.objects.iter().find(|o| o.id == *id) {
            rows.push(OutlineRow {
                id: obj.id.clone(),
                layer_index,
                kind: RowKind::Object,
                depth,
                name: names.get(id).cloned().unwrap_or_else(|| "对象".into()),
                visible: obj.visible,
                shown: chain_shown(layer, ancestors, obj.visible),
                expandable: false,
                open: false,
                ancestors: ancestors.to_vec(),
                ink: crate::document::obj_ink(obj),
            });
        }
    }
}

fn chain_shown(layer: &LayerRuntime, ancestors: &[String], own: bool) -> bool {
    if !own || !layer.visible {
        return false;
    }
    ancestors.iter().all(|id| {
        layer
            .groups
            .iter()
            .find(|g| g.id == *id)
            .map(|g| g.visible)
            .unwrap_or(true)
    })
}

fn first_ink(layer: &LayerRuntime, id: &str) -> Option<[u8; 4]> {
    let leaves = descendant_object_ids(layer, id);
    layer
        .objects
        .iter()
        .find(|o| leaves.contains(&o.id))
        .and_then(|o| crate::document::obj_ink(o))
}

pub fn display_name(doc: &VecDocument, id: &str) -> String {
    if let Some(layer) = doc.layers.iter().find(|l| l.id == id) {
        return layer.name.clone();
    }
    if let Some((li, gi)) = doc.find_group(id) {
        return doc.layers[li].groups[gi].name.clone();
    }
    let Some((li, _)) = doc.find_loc(id) else {
        return String::new();
    };
    let layer = &doc.layers[li];
    let siblings = match doc.parent_of(id) {
        Some((pli, pid)) if pli == li => layer
            .groups
            .iter()
            .find(|g| g.id == pid)
            .map(|g| g.children.clone())
            .unwrap_or_default(),
        _ => roots_bottom_to_top(layer),
    };
    name_map(layer, &siblings)
        .get(id)
        .cloned()
        .unwrap_or_else(|| "对象".into())
}

pub fn set_name(doc: &mut VecDocument, id: &str, name: &str) -> bool {
    let name = name.trim();
    if let Some(layer) = doc.layers.iter_mut().find(|l| l.id == id) {
        if name.is_empty() || layer.name == name {
            return false;
        }
        layer.name = name.to_string();
        return true;
    }
    if let Some((li, gi)) = doc.find_group(id) {
        let next = if name.is_empty() {
            "组".to_string()
        } else {
            name.to_string()
        };
        if doc.layers[li].groups[gi].name == next {
            return false;
        }
        doc.layers[li].groups[gi].name = next;
        return true;
    }
    let mut changed = false;
    doc.mutate(id, |obj| {
        if obj.name != name {
            obj.name = name.to_string();
            changed = true;
        }
    });
    changed
}

pub fn toggle_visible(doc: &mut VecDocument, id: &str) -> bool {
    if let Some(layer) = doc.layers.iter_mut().find(|l| l.id == id) {
        layer.visible = !layer.visible;
        return true;
    }
    if let Some((li, gi)) = doc.find_group(id) {
        let group = &mut doc.layers[li].groups[gi];
        group.visible = !group.visible;
        return true;
    }
    for layer in &mut doc.layers {
        if let Some(slot) = layer.objects.iter_mut().find(|o| o.id == id) {
            let obj = std::sync::Arc::make_mut(slot);
            obj.visible = !obj.visible;
            return true;
        }
    }
    false
}

pub fn thumb_leaves(doc: &VecDocument, row: &OutlineRow) -> Vec<String> {
    match row.kind {
        RowKind::Object => vec![row.id.clone()],
        RowKind::Group => doc.leaf_ids(std::slice::from_ref(&row.id)),
        RowKind::Layer => doc
            .layers
            .get(row.layer_index)
            .map(|layer| layer.objects.iter().map(|o| o.id.clone()).collect())
            .unwrap_or_default(),
    }
}

pub fn group_selection(doc: &mut VecDocument, ids: &[String]) -> Result<String, String> {
    let items = outermost(doc, ids);
    if items.len() < 2 {
        return Err("至少选中两个对象才能组合".into());
    }
    let mut layer_index = None;
    for id in &items {
        let li = layer_of(doc, id).ok_or_else(|| "选中的对象已经不在了".to_string())?;
        if *layer_index.get_or_insert(li) != li {
            return Err("要组合的对象得在同一个图层里".into());
        }
    }
    let li = layer_index.unwrap();
    let keys = order_map(&doc.layers[li]);
    let mut children = items;
    children.sort_by_key(|id| keys.get(id).copied().unwrap_or(usize::MAX));
    let front = children.last().cloned().ok_or("没有可组合的对象")?;
    let parent = doc
        .parent_of(&front)
        .and_then(|(pli, pid)| (pli == li).then_some(pid));
    let name = next_group_name(&doc.layers[li]);
    bind_group(doc, li, name, children, parent)
}

pub fn ungroup_selection(doc: &mut VecDocument, ids: &[String]) -> Result<Vec<DissolvedGroup>, String> {
    let mut targets = Vec::new();
    for id in outermost(doc, ids) {
        if doc.find_group(&id).is_some() {
            if !targets.iter().any(|t: &String| t == &id) {
                targets.push(id);
            }
        } else if let Some((_, parent)) = doc.parent_of(&id) {
            if !targets.iter().any(|t| t == &parent) {
                targets.push(parent);
            }
        }
    }
    if targets.is_empty() {
        return Err("没有可取消的组合".into());
    }
    let mut dissolved = Vec::new();
    for gid in targets {
        if let Some(item) = ungroup_one(doc, &gid) {
            dissolved.push(item);
        }
    }
    if dissolved.is_empty() {
        return Err("没有可取消的组合".into());
    }
    Ok(dissolved)
}

pub fn regroup(
    doc: &mut VecDocument,
    memory: &mut Vec<DissolvedGroup>,
    selected: &[String],
) -> Result<String, String> {
    if memory.is_empty() {
        return Err("没有可重新组合的对象".into());
    }
    let mut pick = None;
    for i in (0..memory.len()).rev() {
        let live = live_children(doc, &memory[i]);
        if live.len() < 2 {
            continue;
        }
        if !selected.is_empty() && !intersects(doc, &live, selected) {
            continue;
        }
        pick = Some((i, live));
        break;
    }
    let Some((index, mut children)) = pick else {
        return Err("这些对象没法重新组合".into());
    };
    let Some(li) = doc.layers.iter().position(|l| l.id == memory[index].layer_id) else {
        return Err("原来的图层已经不在了".into());
    };
    let dissolved = memory.remove(index);
    let keys = order_map(&doc.layers[li]);
    children.sort_by_key(|id| keys.get(id).copied().unwrap_or(usize::MAX));
    let front = children.last().cloned().unwrap_or_default();
    let parent = doc
        .parent_of(&front)
        .and_then(|(pli, pid)| (pli == li).then_some(pid));
    bind_group(doc, li, dissolved.name, children, parent)
}

pub fn duplicate_items(doc: &mut VecDocument, ids: &[String], dx: f32, dy: f32) -> Vec<String> {
    let mut items = outermost(doc, ids);
    items.sort_by(|a, b| {
        let key = |id: &String| {
            layer_of(doc, id)
                .map(|li| order_map(&doc.layers[li]).get(id).copied().unwrap_or(0))
                .unwrap_or(0)
        };
        key(b).cmp(&key(a))
    });
    let mut created = Vec::new();
    for id in items {
        if let Some(nid) = duplicate_one(doc, &id, dx, dy) {
            created.push(nid);
        }
    }
    created.reverse();
    created
}

pub(crate) fn roots_bottom_to_top(layer: &LayerRuntime) -> Vec<String> {
    let mut nested = HashSet::new();
    for group in &layer.groups {
        for child in &group.children {
            nested.insert(child.as_str());
        }
    }
    let mut ids = Vec::new();
    for obj in &layer.objects {
        if !nested.contains(obj.id.as_str()) {
            ids.push(obj.id.clone());
        }
    }
    for group in &layer.groups {
        if !nested.contains(group.id.as_str()) {
            ids.push(group.id.clone());
        }
    }
    let keys = order_map(layer);
    ids.sort_by_key(|id| keys.get(id).copied().unwrap_or(usize::MAX));
    ids
}

pub(crate) fn sanitize_layer(layer: &mut LayerRuntime) {
    let obj_ids: HashSet<&str> = layer.objects.iter().map(|o| o.id.as_str()).collect();
    for _ in 0..8 {
        let gids: HashSet<String> = layer.groups.iter().map(|g| g.id.clone()).collect();
        for group in &mut layer.groups {
            let self_id = group.id.clone();
            group
                .children
                .retain(|c| c != &self_id && (obj_ids.contains(c.as_str()) || gids.contains(c)));
        }
        let mut owner: HashMap<String, usize> = HashMap::new();
        for (gi, group) in layer.groups.iter().enumerate() {
            for child in &group.children {
                owner.insert(child.clone(), gi);
            }
        }
        for (gi, group) in layer.groups.iter_mut().enumerate() {
            group.children.retain(|c| owner.get(c).copied() == Some(gi));
        }
        let before = layer.groups.len();
        layer.groups.retain(|g| !g.children.is_empty());
        if layer.groups.len() == before {
            let gids: HashSet<String> = layer.groups.iter().map(|g| g.id.clone()).collect();
            let mut dirty = false;
            for group in &mut layer.groups {
                let n = group.children.len();
                group
                    .children
                    .retain(|c| obj_ids.contains(c.as_str()) || gids.contains(c));
                if group.children.len() != n {
                    dirty = true;
                }
            }
            if !dirty {
                break;
            }
        }
    }
}

fn bind_group(
    doc: &mut VecDocument,
    layer_index: usize,
    name: String,
    children: Vec<String>,
    parent: Option<String>,
) -> Result<String, String> {
    if children.len() < 2 {
        return Err("至少选中两个对象才能组合".into());
    }
    let layer = doc.layers.get_mut(layer_index).ok_or("图层不存在")?;
    gather_members(layer, &children);
    let set: HashSet<String> = children.iter().cloned().collect();
    detach(layer, &set);
    let gid = new_id();
    layer.groups.push(Group {
        id: gid.clone(),
        name,
        visible: true,
        children,
    });
    if let Some(pid) = parent {
        if let Some(group) = layer.groups.iter_mut().find(|g| g.id == pid) {
            group.children.push(gid.clone());
        }
        let keys = order_map(layer);
        if let Some(group) = layer.groups.iter_mut().find(|g| g.id == pid) {
            group
                .children
                .sort_by_key(|c| keys.get(c).copied().unwrap_or(usize::MAX));
        }
    }
    sanitize_layer(layer);
    Ok(gid)
}

fn ungroup_one(doc: &mut VecDocument, gid: &str) -> Option<DissolvedGroup> {
    let (li, gi) = doc.find_group(gid)?;
    let group = doc.layers[li].groups.remove(gi);
    let layer_id = doc.layers[li].id.clone();
    if let Some(parent_i) = doc.layers[li]
        .groups
        .iter()
        .position(|g| g.children.iter().any(|c| c == gid))
    {
        let parent = &mut doc.layers[li].groups[parent_i];
        if let Some(at) = parent.children.iter().position(|c| c == gid) {
            parent.children.remove(at);
            for (k, child) in group.children.iter().cloned().enumerate() {
                parent.children.insert(at + k, child);
            }
        }
    }
    Some(DissolvedGroup {
        layer_id,
        name: group.name,
        children: group.children,
    })
}

fn gather_members(layer: &mut LayerRuntime, item_ids: &[String]) {
    let mut set = HashSet::new();
    for id in item_ids {
        set.extend(descendant_object_ids(layer, id));
    }
    if set.len() < 2 {
        return;
    }
    let Some(top) = layer.objects.iter().rposition(|o| set.contains(&o.id)) else {
        return;
    };
    let mut before = Vec::new();
    let mut block = Vec::new();
    let mut after = Vec::new();
    for (i, obj) in layer.objects.iter().enumerate() {
        if set.contains(&obj.id) {
            block.push(obj.clone());
        } else if i < top {
            before.push(obj.clone());
        } else {
            after.push(obj.clone());
        }
    }
    before.append(&mut block);
    before.append(&mut after);
    layer.objects = before;
}

fn detach(layer: &mut LayerRuntime, ids: &HashSet<String>) {
    for group in &mut layer.groups {
        group.children.retain(|c| !ids.contains(c));
    }
}

fn duplicate_one(doc: &mut VecDocument, id: &str, dx: f32, dy: f32) -> Option<String> {
    if doc.find_group(id).is_some() {
        duplicate_group(doc, id, dx, dy)
    } else {
        duplicate_object(doc, id, dx, dy)
    }
}

fn duplicate_object(doc: &mut VecDocument, id: &str, dx: f32, dy: f32) -> Option<String> {
    let (li, oi) = doc.find_loc(id)?;
    let mut obj = (*doc.layers[li].objects[oi]).clone();
    obj.id = new_id();
    crate::geom::translate_obj(&mut obj, dx, dy);
    let nid = obj.id.clone();
    let parent = doc.parent_of(id).filter(|(pli, _)| *pli == li).map(|(_, p)| p);
    if li == doc.active {
        doc.layers[li].objects.insert(oi + 1, std::sync::Arc::new(obj));
        if let Some(pid) = parent {
            if let Some(group) = doc.layers[li].groups.iter_mut().find(|g| g.id == pid) {
                if let Some(at) = group.children.iter().position(|c| c == id) {
                    group.children.insert(at + 1, nid.clone());
                }
            }
        }
    } else {
        doc.layers.get_mut(doc.active)?.objects.push(std::sync::Arc::new(obj));
    }
    Some(nid)
}

struct ClonedTree {
    objects: Vec<std::sync::Arc<Obj>>,
    groups: Vec<Group>,
    root_id: String,
}

fn duplicate_group(doc: &mut VecDocument, gid: &str, dx: f32, dy: f32) -> Option<String> {
    let (li, _) = doc.find_group(gid)?;
    let cloned = clone_subtree(&doc.layers[li], gid, dx, dy)?;
    let dest = if li == doc.active { li } else { doc.active };
    if dest >= doc.layers.len() {
        return None;
    }
    let parent = if dest == li {
        doc.parent_of(gid).filter(|(pli, _)| *pli == li).map(|(_, p)| p)
    } else {
        None
    };
    let insert_at = if dest == li {
        let leaves = descendant_object_ids(&doc.layers[li], gid);
        doc.layers[li]
            .objects
            .iter()
            .rposition(|o| leaves.contains(&o.id))
            .map(|i| i + 1)
            .unwrap_or(doc.layers[li].objects.len())
    } else {
        doc.layers[dest].objects.len()
    };
    let root_id = cloned.root_id;
    let layer = &mut doc.layers[dest];
    for (k, obj) in cloned.objects.into_iter().enumerate() {
        layer.objects.insert(insert_at + k, obj);
    }
    layer.groups.extend(cloned.groups);
    if let Some(pid) = parent {
        if let Some(group) = layer.groups.iter_mut().find(|g| g.id == pid) {
            if let Some(at) = group.children.iter().position(|c| c == gid) {
                group.children.insert(at + 1, root_id.clone());
            }
        }
    }
    sanitize_layer(layer);
    Some(root_id)
}

fn clone_subtree(layer: &LayerRuntime, gid: &str, dx: f32, dy: f32) -> Option<ClonedTree> {
    if layer.groups.iter().all(|g| g.id != gid) {
        return None;
    }
    let mut map: HashMap<String, String> = HashMap::new();
    let mut stack = vec![gid.to_string()];
    let mut group_ids = Vec::new();
    while let Some(id) = stack.pop() {
        if map.contains_key(&id) {
            continue;
        }
        map.insert(id.clone(), new_id());
        group_ids.push(id.clone());
        let group = layer.groups.iter().find(|g| g.id == id)?;
        for child in &group.children {
            if layer.groups.iter().any(|g| g.id == *child) {
                stack.push(child.clone());
            }
        }
    }
    let leaves = descendant_object_ids(layer, gid);
    let mut objects = Vec::new();
    for obj in &layer.objects {
        if !leaves.contains(&obj.id) {
            continue;
        }
        let mut copy = (**obj).clone();
        let nid = new_id();
        map.insert(copy.id.clone(), nid.clone());
        copy.id = nid;
        crate::geom::translate_obj(&mut copy, dx, dy);
        objects.push(std::sync::Arc::new(copy));
    }
    let mut groups = Vec::new();
    for id in &group_ids {
        let group = layer.groups.iter().find(|g| g.id == *id)?;
        groups.push(Group {
            id: map.get(&group.id).cloned().unwrap_or_else(new_id),
            name: group.name.clone(),
            visible: group.visible,
            children: group.children.iter().filter_map(|c| map.get(c).cloned()).collect(),
        });
    }
    Some(ClonedTree {
        root_id: map.get(gid).cloned()?,
        objects,
        groups,
    })
}

fn outermost(doc: &VecDocument, ids: &[String]) -> Vec<String> {
    let set: HashSet<&str> = ids.iter().map(|s| s.as_str()).collect();
    let mut out = Vec::new();
    for id in ids {
        if layer_of(doc, id).is_none() {
            continue;
        }
        let mut cur = id.clone();
        let mut blocked = false;
        let mut guard = 0;
        while let Some((_, parent)) = doc.parent_of(&cur) {
            guard += 1;
            if guard > 32 {
                break;
            }
            if set.contains(parent.as_str()) {
                blocked = true;
                break;
            }
            cur = parent;
        }
        if !blocked && !out.iter().any(|e| e == id) {
            out.push(id.clone());
        }
    }
    out
}

fn layer_of(doc: &VecDocument, id: &str) -> Option<usize> {
    if let Some((li, _)) = doc.find_loc(id) {
        return Some(li);
    }
    if let Some((li, _)) = doc.find_group(id) {
        return Some(li);
    }
    None
}

fn live_children(doc: &VecDocument, dissolved: &DissolvedGroup) -> Vec<String> {
    let Some(li) = doc.layers.iter().position(|l| l.id == dissolved.layer_id) else {
        return Vec::new();
    };
    dissolved
        .children
        .iter()
        .filter(|id| layer_of(doc, id) == Some(li))
        .cloned()
        .collect()
}

fn intersects(doc: &VecDocument, live: &[String], selected: &[String]) -> bool {
    let sel_leaves = doc.leaf_ids(selected);
    for child in live {
        if selected.iter().any(|s| s == child) {
            return true;
        }
        let leaves = doc.leaf_ids(std::slice::from_ref(child));
        if leaves.iter().any(|id| sel_leaves.iter().any(|s| s == id)) {
            return true;
        }
    }
    false
}

fn next_group_name(layer: &LayerRuntime) -> String {
    for n in 1..10_000 {
        let candidate = if n == 1 {
            "组".to_string()
        } else {
            format!("组 {n}")
        };
        if layer.groups.iter().all(|g| g.name != candidate) {
            return candidate;
        }
    }
    "组".to_string()
}

fn order_map(layer: &LayerRuntime) -> HashMap<String, usize> {
    let mut memo = HashMap::new();
    for obj in &layer.objects {
        order_key(layer, &obj.id, &mut memo);
    }
    for group in &layer.groups {
        order_key(layer, &group.id, &mut memo);
    }
    memo
}

fn order_key(layer: &LayerRuntime, id: &str, memo: &mut HashMap<String, usize>) -> usize {
    if let Some(v) = memo.get(id) {
        return *v;
    }
    if let Some(i) = layer.objects.iter().position(|o| o.id == id) {
        memo.insert(id.to_string(), i);
        return i;
    }
    memo.insert(id.to_string(), usize::MAX);
    let Some(group) = layer.groups.iter().find(|g| g.id == id) else {
        return usize::MAX;
    };
    let mut best = usize::MAX;
    for child in &group.children {
        best = best.min(order_key(layer, child, memo));
    }
    memo.insert(id.to_string(), best);
    best
}

fn descendant_object_ids(layer: &LayerRuntime, id: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    walk_objects(layer, id, &mut out, 0);
    out
}

fn walk_objects(layer: &LayerRuntime, id: &str, out: &mut HashSet<String>, depth: u8) {
    if depth > 32 {
        return;
    }
    if layer.objects.iter().any(|o| o.id == id) {
        out.insert(id.to_string());
        return;
    }
    let Some(group) = layer.groups.iter().find(|g| g.id == id) else {
        return;
    };
    for child in &group.children {
        walk_objects(layer, child, out, depth + 1);
    }
}

fn name_map(layer: &LayerRuntime, ids_bottom_to_top: &[String]) -> HashMap<String, String> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut out = HashMap::new();
    for id in ids_bottom_to_top {
        if let Some(obj) = layer.objects.iter().find(|o| o.id == *id) {
            if !obj.name.is_empty() {
                out.insert(id.clone(), obj.name.clone());
                continue;
            }
            let label = kind_label(obj);
            let n = counts.entry(label.clone()).or_insert(0);
            *n += 1;
            let name = if *n == 1 {
                label
            } else {
                format!("{label} {n}")
            };
            out.insert(id.clone(), name);
        } else if let Some(group) = layer.groups.iter().find(|g| g.id == *id) {
            out.insert(id.clone(), group.name.clone());
        }
    }
    out
}

fn kind_label(obj: &Obj) -> String {
    match &obj.kind {
        ObjKind::Text(text) => {
            let flat: String = text.text.split_whitespace().collect::<Vec<_>>().join(" ");
            let flat: String = flat.chars().take(18).collect();
            if flat.is_empty() { "文字".into() } else { flat }
        }
        ObjKind::Image(_) => "图像".into(),
        ObjKind::Path(path) => path_label(path).into(),
    }
}

fn path_label(path: &crate::document::PathObj) -> &'static str {
    if path.subpaths.len() > 1 {
        return "复合路径";
    }
    let Some(sub) = path.subpaths.first() else {
        return "路径";
    };
    if sub.closed {
        return "形状";
    }
    let curve = sub.segs.iter().any(|seg| matches!(seg, crate::document::Seg::Cubic { .. }));
    if !curve && sub.segs.len() <= 1 {
        "线段"
    } else if curve {
        "曲线"
    } else {
        "路径"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{FillRule, LineCap, LineJoin, Paint, PathObj, Seg, Stroke, Subpath};

    fn line(a: (f32, f32), b: (f32, f32)) -> Obj {
        Obj::path(PathObj {
            subpaths: vec![Subpath {
                closed: false,
                segs: vec![Seg::Line { a, b }],
            }],
            fill: None,
            stroke: Some(Stroke {
                paint: Paint::rgb(0, 0, 0),
                width: 1.0,
                cap: LineCap::Butt,
                join: LineJoin::Miter,
                dash: Vec::new(),
                dash_phase: 0.0,
            }),
            fill_rule: FillRule::Winding,
            clip: Vec::new(),
        })
    }

    #[test]
    fn group_gathers_and_roundtrips_regroup() {
        let mut doc = VecDocument::from_objects(
            30.0,
            10.0,
            vec![line((0.0, 0.0), (1.0, 0.0)), line((2.0, 0.0), (3.0, 0.0)), line((4.0, 0.0), (5.0, 0.0))],
        );
        let a = doc.layers[0].objects[0].id.clone();
        let b = doc.layers[0].objects[1].id.clone();
        let c = doc.layers[0].objects[2].id.clone();
        assert_eq!(display_name(&doc, &a), "线段");
        assert_eq!(display_name(&doc, &c), "线段 3");
        let gid = group_selection(&mut doc, &[a.clone(), c.clone()]).unwrap();
        let order: Vec<_> = doc.layers[0].objects.iter().map(|o| o.id.clone()).collect();
        assert_eq!(order, vec![b.clone(), a.clone(), c.clone()]);
        assert_eq!(doc.leaf_ids(&[gid.clone()]), vec![a.clone(), c.clone()]);
        assert_eq!(doc.pick_target(&a), gid);
        assert_eq!(doc.plain_click_target(&[], &a), gid);
        assert_eq!(doc.plain_click_target(&[gid.clone()], &a), a);
        assert_eq!(doc.plain_click_target(&[a.clone()], &c), c);
        assert_eq!(doc.plain_click_target(&[a.clone()], &b), b);
        let dissolved = ungroup_selection(&mut doc, &[gid]).unwrap();
        assert!(doc.layers[0].groups.is_empty());
        assert_eq!(dissolved[0].children, vec![a.clone(), c.clone()]);
        let mut memory = dissolved;
        let again = regroup(&mut doc, &mut memory, &[]).unwrap();
        assert!(memory.is_empty());
        assert_eq!(doc.leaf_ids(&[again.clone()]), vec![a, c]);
        let copy = duplicate_items(&mut doc, &[again], 3.0, 0.0);
        assert_eq!(copy.len(), 1);
        assert_eq!(doc.layers[0].groups.len(), 2);
        assert_eq!(doc.leaf_ids(&copy).len(), 2);
    }

    #[test]
    fn hidden_group_is_not_hit() {
        let mut doc = VecDocument::from_objects(
            20.0,
            10.0,
            vec![line((0.0, 1.0), (10.0, 1.0)), line((0.0, 4.0), (10.0, 4.0))],
        );
        let a = doc.layers[0].objects[0].id.clone();
        let b = doc.layers[0].objects[1].id.clone();
        let gid = group_selection(&mut doc, &[a.clone(), b.clone()]).unwrap();
        assert!(doc.hit_test(5.0, 1.0, 0.8).is_some());
        toggle_visible(&mut doc, &gid);
        assert!(doc.hit_test(5.0, 1.0, 0.8).is_none());
        assert!(doc.hit_test(5.0, 4.0, 0.8).is_none());
    }
}
