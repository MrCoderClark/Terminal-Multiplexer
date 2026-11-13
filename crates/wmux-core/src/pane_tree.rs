use serde::{Deserialize, Serialize};

pub type PaneId = u32;

// Axis of a split. `H` means the two children sit side-by-side (left | right);
// `V` means they stack (top / bottom). See SPEC §5 for keybinding mapping:
// `prefix "` opens a new pane below (Dir::V), `prefix %` opens to the right
// (Dir::H).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Dir {
    H,
    V,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FocusDir {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum PaneTree {
    Leaf(PaneId),
    Split {
        dir: Dir,
        // (0.0, 1.0): fraction of the (non-border) area given to child `a`.
        ratio: f32,
        a: Box<PaneTree>,
        b: Box<PaneTree>,
    },
}

impl PaneTree {
    pub fn leaf(id: PaneId) -> Self {
        PaneTree::Leaf(id)
    }

    pub fn contains(&self, id: PaneId) -> bool {
        match self {
            PaneTree::Leaf(leaf) => *leaf == id,
            PaneTree::Split { a, b, .. } => a.contains(id) || b.contains(id),
        }
    }

    pub fn all_pane_ids(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.collect_ids(&mut out);
        out
    }

    fn collect_ids(&self, out: &mut Vec<PaneId>) {
        match self {
            PaneTree::Leaf(id) => out.push(*id),
            PaneTree::Split { a, b, .. } => {
                a.collect_ids(out);
                b.collect_ids(out);
            }
        }
    }

    // Replace `Leaf(target)` with a 50/50 split containing the original
    // target as `a` and a fresh pane as `b`. Returns true if the target
    // was found.
    pub fn split_leaf(&mut self, target: PaneId, new_id: PaneId, dir: Dir) -> bool {
        match self {
            PaneTree::Leaf(id) if *id == target => {
                *self = PaneTree::Split {
                    dir,
                    ratio: 0.5,
                    a: Box::new(PaneTree::Leaf(target)),
                    b: Box::new(PaneTree::Leaf(new_id)),
                };
                true
            }
            PaneTree::Leaf(_) => false,
            PaneTree::Split { a, b, .. } => {
                a.split_leaf(target, new_id, dir) || b.split_leaf(target, new_id, dir)
            }
        }
    }

    // Remove `id` and promote its sibling in place of the containing Split.
    // Returns false if `id` was not found or if `id` is the only leaf in
    // the tree (caller should refuse to kill the last pane).
    pub fn remove(&mut self, id: PaneId) -> bool {
        if matches!(self, PaneTree::Leaf(leaf) if *leaf == id) {
            // Cannot remove the root leaf — would leave an empty tree.
            return false;
        }

        match self {
            PaneTree::Leaf(_) => false,
            PaneTree::Split { a, b, .. } => {
                if let PaneTree::Leaf(aid) = a.as_ref() {
                    if *aid == id {
                        let sibling = std::mem::replace(b.as_mut(), PaneTree::Leaf(0));
                        *self = sibling;
                        return true;
                    }
                }
                if let PaneTree::Leaf(bid) = b.as_ref() {
                    if *bid == id {
                        let sibling = std::mem::replace(a.as_mut(), PaneTree::Leaf(0));
                        *self = sibling;
                        return true;
                    }
                }
                if a.contains(id) {
                    a.remove(id)
                } else if b.contains(id) {
                    b.remove(id)
                } else {
                    false
                }
            }
        }
    }

    // Walk the tree, dividing `area` by `ratio` with a 1-cell border reserved
    // at each split. Appends `(pane_id, rect)` pairs to `out`.
    pub fn compute_rects(&self, area: Rect) -> Vec<(PaneId, Rect)> {
        let mut out = Vec::new();
        self.collect_rects(area, &mut out);
        out
    }

    fn collect_rects(&self, area: Rect, out: &mut Vec<(PaneId, Rect)>) {
        match self {
            PaneTree::Leaf(id) => out.push((*id, area)),
            PaneTree::Split { dir, ratio, a, b } => {
                let ratio = ratio.clamp(0.1, 0.9);
                let (ar, br) = match dir {
                    Dir::H => {
                        let total = area.w.saturating_sub(1).max(2);
                        let aw = ((total as f32 * ratio).round() as u16).clamp(1, total - 1);
                        let bw = total - aw;
                        (
                            Rect {
                                x: area.x,
                                y: area.y,
                                w: aw,
                                h: area.h,
                            },
                            Rect {
                                x: area.x + aw + 1,
                                y: area.y,
                                w: bw,
                                h: area.h,
                            },
                        )
                    }
                    Dir::V => {
                        let total = area.h.saturating_sub(1).max(2);
                        let ah = ((total as f32 * ratio).round() as u16).clamp(1, total - 1);
                        let bh = total - ah;
                        (
                            Rect {
                                x: area.x,
                                y: area.y,
                                w: area.w,
                                h: ah,
                            },
                            Rect {
                                x: area.x,
                                y: area.y + ah + 1,
                                w: area.w,
                                h: bh,
                            },
                        )
                    }
                };
                a.collect_rects(ar, out);
                b.collect_rects(br, out);
            }
        }
    }

    // Border segments needed to render the tree. One per internal Split.
    pub fn collect_borders(&self, area: Rect) -> Vec<BorderSeg> {
        let mut out = Vec::new();
        self.collect_borders_inner(area, &mut out);
        out
    }

    fn collect_borders_inner(&self, area: Rect, out: &mut Vec<BorderSeg>) {
        match self {
            PaneTree::Leaf(_) => {}
            PaneTree::Split { dir, ratio, a, b } => {
                let ratio = ratio.clamp(0.1, 0.9);
                match dir {
                    Dir::H => {
                        let total = area.w.saturating_sub(1).max(2);
                        let aw = ((total as f32 * ratio).round() as u16).clamp(1, total - 1);
                        out.push(BorderSeg::Vertical {
                            x: area.x + aw,
                            y: area.y,
                            h: area.h,
                        });
                        a.collect_borders_inner(
                            Rect {
                                x: area.x,
                                y: area.y,
                                w: aw,
                                h: area.h,
                            },
                            out,
                        );
                        b.collect_borders_inner(
                            Rect {
                                x: area.x + aw + 1,
                                y: area.y,
                                w: total - aw,
                                h: area.h,
                            },
                            out,
                        );
                    }
                    Dir::V => {
                        let total = area.h.saturating_sub(1).max(2);
                        let ah = ((total as f32 * ratio).round() as u16).clamp(1, total - 1);
                        out.push(BorderSeg::Horizontal {
                            x: area.x,
                            y: area.y + ah,
                            w: area.w,
                        });
                        a.collect_borders_inner(
                            Rect {
                                x: area.x,
                                y: area.y,
                                w: area.w,
                                h: ah,
                            },
                            out,
                        );
                        b.collect_borders_inner(
                            Rect {
                                x: area.x,
                                y: area.y + ah + 1,
                                w: area.w,
                                h: total - ah,
                            },
                            out,
                        );
                    }
                }
            }
        }
    }

    // Find the pane immediately adjacent to `current` in `dir`. Looks for
    // a pane whose rect shares an edge with `current`'s rect and overlaps
    // on the orthogonal axis. First match in tree-preorder wins.
    pub fn focus_from(&self, current: PaneId, dir: FocusDir, area: Rect) -> Option<PaneId> {
        let rects = self.compute_rects(area);
        let current_rect = rects
            .iter()
            .find(|(id, _)| *id == current)
            .map(|(_, r)| *r)?;
        for (id, r) in &rects {
            if *id == current {
                continue;
            }
            let adjacent = match dir {
                FocusDir::Left => r.x + r.w + 1 == current_rect.x && overlaps_y(r, &current_rect),
                FocusDir::Right => {
                    current_rect.x + current_rect.w + 1 == r.x && overlaps_y(r, &current_rect)
                }
                FocusDir::Up => r.y + r.h + 1 == current_rect.y && overlaps_x(r, &current_rect),
                FocusDir::Down => {
                    current_rect.y + current_rect.h + 1 == r.y && overlaps_x(r, &current_rect)
                }
            };
            if adjacent {
                return Some(*id);
            }
        }
        None
    }
}

#[derive(Clone, Copy, Debug)]
pub enum BorderSeg {
    Vertical { x: u16, y: u16, h: u16 },
    Horizontal { x: u16, y: u16, w: u16 },
}

fn overlaps_x(a: &Rect, b: &Rect) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w
}

fn overlaps_y(a: &Rect, b: &Rect) -> bool {
    a.y < b.y + b.h && b.y < a.y + a.h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_leaf_takes_full_area() {
        let t = PaneTree::leaf(1);
        let rects = t.compute_rects(Rect {
            x: 0,
            y: 0,
            w: 80,
            h: 24,
        });
        assert_eq!(rects.len(), 1);
        assert_eq!(rects[0].0, 1);
        assert_eq!(rects[0].1.w, 80);
        assert_eq!(rects[0].1.h, 24);
    }

    #[test]
    fn horizontal_split_uses_one_col_border() {
        let mut t = PaneTree::leaf(1);
        assert!(t.split_leaf(1, 2, Dir::H));
        let rects = t.compute_rects(Rect {
            x: 0,
            y: 0,
            w: 80,
            h: 24,
        });
        let a = rects.iter().find(|(id, _)| *id == 1).unwrap().1;
        let b = rects.iter().find(|(id, _)| *id == 2).unwrap().1;
        // 1 column reserved for the border between them.
        assert_eq!(a.w + b.w + 1, 80);
        assert_eq!(a.x, 0);
        assert_eq!(b.x, a.w + 1);
    }

    #[test]
    fn remove_promotes_sibling() {
        let mut t = PaneTree::leaf(1);
        t.split_leaf(1, 2, Dir::H);
        assert!(t.remove(2));
        assert!(matches!(t, PaneTree::Leaf(1)));
    }

    #[test]
    fn remove_last_pane_fails() {
        let mut t = PaneTree::leaf(1);
        assert!(!t.remove(1));
    }

    #[test]
    fn focus_right_crosses_border() {
        let mut t = PaneTree::leaf(1);
        t.split_leaf(1, 2, Dir::H);
        let area = Rect {
            x: 0,
            y: 0,
            w: 80,
            h: 24,
        };
        assert_eq!(t.focus_from(1, FocusDir::Right, area), Some(2));
        assert_eq!(t.focus_from(2, FocusDir::Left, area), Some(1));
        assert_eq!(t.focus_from(1, FocusDir::Up, area), None);
    }
}
