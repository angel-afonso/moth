//! A tab's pane tree: row splits (side by side) or column splits (stacked), each child with its
//! fraction of the space. Splitting halves the pane; inside a split along the same axis it adds
//! a sibling instead of nesting, like iTerm does.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    /// Children left to right.
    Row,
    /// Children top to bottom.
    Column,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaneTree<T> {
    Leaf(T),
    Split {
        axis: Axis,
        children: Vec<PaneTree<T>>,
        /// Each child's fraction of the space; they add up to 1.
        sizes: Vec<f32>,
    },
}

/// Normalized rectangle: the whole tab spans 0 to 1 on each axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

const EPSILON: f32 = 1e-4;

impl<T> PaneTree<T> {
    fn pair(axis: Axis, first: T, second: T) -> Self {
        Self::Split {
            axis,
            children: vec![Self::Leaf(first), Self::Leaf(second)],
            sizes: vec![0.5, 0.5],
        }
    }

    /// The same tree with a different leaf type (e.g. to save the session with folders).
    pub fn map<U>(&self, f: &mut impl FnMut(&T) -> U) -> PaneTree<U> {
        match self {
            Self::Leaf(leaf) => PaneTree::Leaf(f(leaf)),
            Self::Split {
                axis,
                children,
                sizes,
            } => PaneTree::Split {
                axis: *axis,
                children: children.iter().map(|child| child.map(f)).collect(),
                sizes: sizes.clone(),
            },
        }
    }

    /// Fixes what the rest of the code takes for granted, for a tree from a hand-edited file,
    /// say: sizes that add up, and splits with at least two children (an empty split goes,
    /// and one with a single child becomes that child). `None` if no pane is left.
    pub fn normalized(self) -> Option<Self> {
        match self {
            Self::Leaf(_) => Some(self),
            Self::Split {
                axis,
                children,
                sizes,
            } => {
                let valid_sizes =
                    sizes.len() == children.len() && sizes.iter().all(|s| s.is_finite() && *s > 0.);
                let (mut children, sizes): (Vec<Self>, Vec<f32>) = children
                    .into_iter()
                    .enumerate()
                    .filter_map(|(ix, child)| {
                        let size = if valid_sizes { sizes[ix] } else { 1. };
                        child.normalized().map(|child| (child, size))
                    })
                    .unzip();
                match children.len() {
                    0 => None,
                    1 => children.pop(),
                    _ => {
                        let total: f32 = sizes.iter().sum();
                        Some(Self::Split {
                            axis,
                            children,
                            sizes: sizes.into_iter().map(|s| s / total).collect(),
                        })
                    }
                }
            }
        }
    }
}

impl<T: Copy + PartialEq> PaneTree<T> {
    /// Panes in reading order (left to right, top to bottom).
    pub fn leaves(&self) -> Vec<T> {
        match self {
            Self::Leaf(id) => vec![*id],
            Self::Split { children, .. } => children.iter().flat_map(Self::leaves).collect(),
        }
    }

    fn is_leaf(&self, target: T) -> bool {
        matches!(self, Self::Leaf(id) if *id == target)
    }

    /// Adds `new` to the right of (`Row`) or below (`Column`) `target`, taking half of its
    /// space.
    pub fn split(&mut self, target: T, new: T, axis: Axis) -> bool {
        match self {
            Self::Leaf(id) if *id == target => {
                *self = Self::pair(axis, target, new);
                true
            }
            Self::Leaf(_) => false,
            Self::Split {
                axis: own,
                children,
                sizes,
            } => {
                if let Some(ix) = children.iter().position(|child| child.is_leaf(target)) {
                    if *own == axis {
                        let half = sizes[ix] / 2.;
                        sizes[ix] = half;
                        sizes.insert(ix + 1, half);
                        children.insert(ix + 1, Self::Leaf(new));
                    } else {
                        children[ix] = Self::pair(axis, target, new);
                    }
                    return true;
                }
                children
                    .iter_mut()
                    .any(|child| child.split(target, new, axis))
            }
        }
    }

    /// Removes `target`; its space goes to the previous sibling (or the next one if it was
    /// first). A split left with a single child is replaced by that child. Returns `false` if
    /// it wasn't found or was the only pane.
    pub fn remove(&mut self, target: T) -> bool {
        let Self::Split {
            axis,
            children,
            sizes,
        } = self
        else {
            return false;
        };
        if let Some(ix) = children.iter().position(|child| child.is_leaf(target)) {
            children.remove(ix);
            let freed = sizes.remove(ix);
            let heir = ix.saturating_sub(1).min(sizes.len() - 1);
            sizes[heir] += freed;
        } else if !children.iter_mut().any(|child| child.remove(target)) {
            return false;
        }
        // If a child collapsed into a split along the same axis, its children become siblings
        // with their share of the space.
        let own = *axis;
        let mut flat_children = Vec::new();
        let mut flat_sizes = Vec::new();
        for (child, size) in children.drain(..).zip(sizes.drain(..)) {
            match child {
                Self::Split {
                    axis,
                    children,
                    sizes,
                } if axis == own => {
                    flat_sizes.extend(sizes.iter().map(|s| s * size));
                    flat_children.extend(children);
                }
                child => {
                    flat_children.push(child);
                    flat_sizes.push(size);
                }
            }
        }
        *children = flat_children;
        *sizes = flat_sizes;
        if children.len() == 1 {
            *self = children.remove(0);
        }
        true
    }

    pub fn replace(&mut self, old: T, new: T) -> bool {
        match self {
            Self::Leaf(id) if *id == old => {
                *id = new;
                true
            }
            Self::Leaf(_) => false,
            Self::Split { children, .. } => {
                children.iter_mut().any(|child| child.replace(old, new))
            }
        }
    }

    /// Moves the divider before child `divider` of the split at `path` (child indices from the
    /// root) to `position`, between 0 and 1 along that split. No pane ends up smaller than
    /// `min`.
    pub fn resize(&mut self, path: &[usize], divider: usize, position: f32, min: f32) -> bool {
        let mut node = self;
        for &ix in path {
            let Self::Split { children, .. } = node else {
                return false;
            };
            let Some(child) = children.get_mut(ix) else {
                return false;
            };
            node = child;
        }
        let Self::Split { sizes, .. } = node else {
            return false;
        };
        if divider == 0 || divider >= sizes.len() {
            return false;
        }
        let before: f32 = sizes[..divider - 1].iter().sum();
        let pair = sizes[divider - 1] + sizes[divider];
        let min = min.min(pair / 2.);
        let first = (position - before).clamp(min, pair - min);
        sizes[divider - 1] = first;
        sizes[divider] = pair - first;
        true
    }

    /// Where each pane sits within the tab.
    pub fn rects(&self) -> Vec<(T, Rect)> {
        let mut out = Vec::new();
        self.collect_rects(
            Rect {
                x: 0.,
                y: 0.,
                w: 1.,
                h: 1.,
            },
            &mut out,
        );
        out
    }

    fn collect_rects(&self, rect: Rect, out: &mut Vec<(T, Rect)>) {
        match self {
            Self::Leaf(id) => out.push((*id, rect)),
            Self::Split {
                axis,
                children,
                sizes,
            } => {
                let mut offset = 0.;
                for (child, size) in children.iter().zip(sizes) {
                    let part = match axis {
                        Axis::Row => Rect {
                            x: rect.x + rect.w * offset,
                            w: rect.w * size,
                            ..rect
                        },
                        Axis::Column => Rect {
                            y: rect.y + rect.h * offset,
                            h: rect.h * size,
                            ..rect
                        },
                    };
                    offset += size;
                    child.collect_rects(part, out);
                }
            }
        }
    }

    /// The pane next to `from` in `direction`: the closest one on that side that overlaps on
    /// the other axis; among equally close ones, the best aligned.
    pub fn neighbor(&self, from: T, direction: Direction) -> Option<T> {
        let rects = self.rects();
        let (_, current) = *rects.iter().find(|(id, _)| *id == from)?;
        let overlap = |a0: f32, a1: f32, b0: f32, b1: f32| a1.min(b1) - a0.max(b0);
        rects
            .iter()
            .filter(|(id, _)| *id != from)
            .filter_map(|&(id, r)| {
                let vertical = overlap(r.y, r.y + r.h, current.y, current.y + current.h);
                let horizontal = overlap(r.x, r.x + r.w, current.x, current.x + current.w);
                let (gap, shared) = match direction {
                    Direction::Left => (current.x - (r.x + r.w), vertical),
                    Direction::Right => (r.x - (current.x + current.w), vertical),
                    Direction::Up => (current.y - (r.y + r.h), horizontal),
                    Direction::Down => (r.y - (current.y + current.h), horizontal),
                };
                (gap > -EPSILON && shared > EPSILON).then_some((id, gap, shared))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1).then(b.2.total_cmp(&a.2)))
            .map(|(id, _, _)| id)
    }
}

#[cfg(test)]
mod tests {
    use super::{Axis::*, Direction::*, *};

    fn leaf(id: u32) -> PaneTree<u32> {
        PaneTree::Leaf(id)
    }

    fn sizes_of(tree: &PaneTree<u32>) -> Vec<f32> {
        match tree {
            PaneTree::Split { sizes, .. } => sizes.clone(),
            PaneTree::Leaf(_) => vec![1.],
        }
    }

    fn assert_close(actual: &[f32], expected: &[f32]) {
        assert_eq!(actual.len(), expected.len(), "{actual:?}");
        for (a, e) in actual.iter().zip(expected) {
            assert!((a - e).abs() < 1e-5, "{actual:?} != {expected:?}");
        }
    }

    #[test]
    fn splitting_halves_the_pane() {
        let mut tree = leaf(1);
        assert!(tree.split(1, 2, Row));
        assert!(tree.split(2, 3, Row));
        assert_eq!(tree.leaves(), vec![1, 2, 3]);
        assert_eq!(sizes_of(&tree), vec![0.5, 0.25, 0.25]);
        // Along the other axis, the pane turns into a split.
        assert!(tree.split(2, 4, Column));
        assert_eq!(tree.leaves(), vec![1, 2, 4, 3]);
        assert!(!tree.split(9, 5, Row));
    }

    #[test]
    fn removing_gives_the_space_away_and_flattens() {
        let mut tree = leaf(1);
        tree.split(1, 2, Row);
        tree.split(2, 3, Column);
        tree.split(3, 4, Row);
        // Row[1, Column[2, Row[3, 4]]]: removing 2 leaves Row[1, 3, 4], flattened, with 3 and
        // 4 sharing the right half.
        assert!(tree.remove(2));
        assert_eq!(tree.leaves(), vec![1, 3, 4]);
        assert_eq!(sizes_of(&tree), vec![0.5, 0.25, 0.25]);
        // 3's space goes to 1, the previous sibling.
        assert!(tree.remove(3));
        assert_eq!(sizes_of(&tree), vec![0.75, 0.25]);
        assert!(tree.remove(4));
        assert_eq!(tree, leaf(1));
        // The last pane isn't removed: the tab gets closed instead.
        assert!(!tree.remove(1));
    }

    #[test]
    fn resizing_moves_one_divider_within_limits() {
        let mut tree = leaf(1);
        tree.split(1, 2, Row);
        tree.split(2, 3, Row);
        // [0.5, 0.25, 0.25]: the divider between 2 and 3 (before child 2) moves to 0.9.
        assert!(tree.resize(&[], 2, 0.9, 0.05));
        assert_close(&sizes_of(&tree), &[0.5, 0.4, 0.1]);
        // No pane goes below the minimum.
        assert!(tree.resize(&[], 1, 0.0, 0.1));
        assert_close(&sizes_of(&tree), &[0.1, 0.8, 0.1]);
        assert!(!tree.resize(&[0], 1, 0.5, 0.1));
        assert!(!tree.resize(&[], 0, 0.5, 0.1));
    }

    #[test]
    fn neighbors_follow_the_layout() {
        // ┌───┬───┐
        // │ 1 │ 2 │
        // │   ├───┤
        // │   │ 3 │
        // └───┴───┘
        let mut tree = leaf(1);
        tree.split(1, 2, Row);
        tree.split(2, 3, Column);
        assert_eq!(tree.neighbor(1, Right), Some(2));
        assert_eq!(tree.neighbor(3, Left), Some(1));
        assert_eq!(tree.neighbor(2, Down), Some(3));
        assert_eq!(tree.neighbor(3, Up), Some(2));
        assert_eq!(tree.neighbor(1, Left), None);
        assert_eq!(tree.neighbor(2, Right), None);
        assert_eq!(
            tree.rects()[2],
            (
                3,
                Rect {
                    x: 0.5,
                    y: 0.5,
                    w: 0.5,
                    h: 0.5
                }
            )
        );
    }

    #[test]
    fn normalizing_drops_empty_and_single_child_splits() {
        let empty = || PaneTree::<u32>::Split {
            axis: Row,
            children: vec![],
            sizes: vec![],
        };
        assert_eq!(empty().normalized(), None);
        let lonely = PaneTree::Split {
            axis: Column,
            children: vec![PaneTree::Split {
                axis: Row,
                children: vec![leaf(1)],
                sizes: vec![1.],
            }],
            sizes: vec![1.],
        };
        assert_eq!(lonely.normalized(), Some(leaf(1)));
        let mixed = PaneTree::Split {
            axis: Row,
            children: vec![empty(), leaf(1), leaf(2)],
            sizes: vec![0.2, 0.4, 0.4],
        };
        let mixed = mixed.normalized().unwrap();
        assert_eq!(mixed.leaves(), vec![1, 2]);
        assert_eq!(sizes_of(&mixed), vec![0.5, 0.5]);
    }

    #[test]
    fn replace_map_and_normalize() {
        let mut tree = leaf(1);
        tree.split(1, 2, Column);
        assert!(tree.replace(2, 7));
        assert_eq!(tree.leaves(), vec![1, 7]);
        let doubled = tree.map(&mut |id| id * 2);
        assert_eq!(doubled.leaves(), vec![2, 14]);
        let broken = PaneTree::Split {
            axis: Row,
            children: vec![leaf(1), leaf(2)],
            sizes: vec![3.],
        };
        assert_eq!(sizes_of(&broken.normalized().unwrap()), vec![0.5, 0.5]);
    }
}
