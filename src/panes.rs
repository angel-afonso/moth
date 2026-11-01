//! Árbol de paneles de una pestaña: divisiones en fila (lado a lado) o en columna (uno sobre
//! otro), con hijos del mismo tamaño. Dividir dentro de una división del mismo sentido agrega
//! un hermano en vez de anidar, como en iTerm.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// Hijos de izquierda a derecha.
    Row,
    /// Hijos de arriba abajo.
    Column,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PaneTree<T> {
    Leaf(T),
    Split {
        axis: Axis,
        children: Vec<PaneTree<T>>,
    },
}

/// Rectángulo normalizado: la pestaña entera va de 0 a 1 en cada eje.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

const EPSILON: f32 = 1e-4;

impl<T: Copy + PartialEq> PaneTree<T> {
    /// Los paneles en orden de lectura (izquierda a derecha, arriba abajo).
    pub fn leaves(&self) -> Vec<T> {
        match self {
            Self::Leaf(id) => vec![*id],
            Self::Split { children, .. } => children.iter().flat_map(Self::leaves).collect(),
        }
    }

    fn is_leaf(&self, target: T) -> bool {
        matches!(self, Self::Leaf(id) if *id == target)
    }

    /// Agrega `new` a la derecha (`Row`) o debajo (`Column`) de `target`.
    pub fn split(&mut self, target: T, new: T, axis: Axis) -> bool {
        match self {
            Self::Leaf(id) if *id == target => {
                *self = Self::Split {
                    axis,
                    children: vec![Self::Leaf(target), Self::Leaf(new)],
                };
                true
            }
            Self::Leaf(_) => false,
            Self::Split {
                axis: own,
                children,
            } => {
                if let Some(ix) = children.iter().position(|child| child.is_leaf(target)) {
                    if *own == axis {
                        children.insert(ix + 1, Self::Leaf(new));
                    } else {
                        children[ix] = Self::Split {
                            axis,
                            children: vec![Self::Leaf(target), Self::Leaf(new)],
                        };
                    }
                    return true;
                }
                children
                    .iter_mut()
                    .any(|child| child.split(target, new, axis))
            }
        }
    }

    /// Quita `target`. Una división que queda con un solo hijo se reemplaza por él. Devuelve
    /// `false` si no estaba o si era el único panel.
    pub fn remove(&mut self, target: T) -> bool {
        let Self::Split { axis, children } = self else {
            return false;
        };
        if let Some(ix) = children.iter().position(|child| child.is_leaf(target)) {
            children.remove(ix);
        } else if !children.iter_mut().any(|child| child.remove(target)) {
            return false;
        }
        // Si un hijo colapsó a una división del mismo sentido, sus hijos pasan a ser hermanos.
        let own = *axis;
        let flattened = children
            .drain(..)
            .flat_map(|child| match child {
                Self::Split { axis, children } if axis == own => children,
                child => vec![child],
            })
            .collect();
        *children = flattened;
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

    /// Dónde queda cada panel dentro de la pestaña.
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
            Self::Split { axis, children } => {
                let n = children.len() as f32;
                for (ix, child) in children.iter().enumerate() {
                    let ix = ix as f32;
                    let part = match axis {
                        Axis::Row => Rect {
                            x: rect.x + rect.w * ix / n,
                            w: rect.w / n,
                            ..rect
                        },
                        Axis::Column => Rect {
                            y: rect.y + rect.h * ix / n,
                            h: rect.h / n,
                            ..rect
                        },
                    };
                    child.collect_rects(part, out);
                }
            }
        }
    }

    /// El panel vecino de `from` hacia `direction`: el más cercano de los que están de ese lado
    /// y comparten parte del otro eje; entre los que están igual de cerca, el más alineado.
    pub fn neighbor(&self, from: T, direction: Direction) -> Option<T> {
        let rects = self.rects();
        let (_, current) = *rects.iter().find(|(id, _)| *id == from)?;
        let overlap = |a0: f32, a1: f32, b0: f32, b1: f32| a1.min(b1) - a0.max(b0);
        rects
            .iter()
            .filter(|(id, _)| *id != from)
            .filter_map(|&(id, r)| {
                let (gap, shared) = match direction {
                    Direction::Left => (
                        current.x - (r.x + r.w),
                        overlap(r.y, r.y + r.h, current.y, current.y + current.h),
                    ),
                    Direction::Right => (
                        r.x - (current.x + current.w),
                        overlap(r.y, r.y + r.h, current.y, current.y + current.h),
                    ),
                    Direction::Up => (
                        current.y - (r.y + r.h),
                        overlap(r.x, r.x + r.w, current.x, current.x + current.w),
                    ),
                    Direction::Down => (
                        r.y - (current.y + current.h),
                        overlap(r.x, r.x + r.w, current.x, current.x + current.w),
                    ),
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

    #[test]
    fn splitting_the_same_way_adds_siblings() {
        let mut tree = leaf(1);
        assert!(tree.split(1, 2, Row));
        assert!(tree.split(2, 3, Row));
        assert_eq!(
            tree,
            PaneTree::Split {
                axis: Row,
                children: vec![leaf(1), leaf(2), leaf(3)]
            }
        );
        // En el otro sentido, el panel se convierte en una división.
        assert!(tree.split(2, 4, Column));
        assert_eq!(tree.leaves(), vec![1, 2, 4, 3]);
        assert!(!tree.split(9, 5, Row));
    }

    #[test]
    fn removing_collapses_and_flattens() {
        let mut tree = leaf(1);
        tree.split(1, 2, Row);
        tree.split(2, 3, Column);
        tree.split(3, 4, Row);
        // Row[1, Column[2, Row[3, 4]]]: al quitar 2 queda Row[1, 3, 4], sin anidar.
        assert!(tree.remove(2));
        assert_eq!(
            tree,
            PaneTree::Split {
                axis: Row,
                children: vec![leaf(1), leaf(3), leaf(4)]
            }
        );
        assert!(tree.remove(3));
        assert!(tree.remove(4));
        assert_eq!(tree, leaf(1));
        // El último panel no se quita: se cierra la pestaña.
        assert!(!tree.remove(1));
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
        let rects = tree.rects();
        assert_eq!(
            rects[2],
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
    fn replace_swaps_a_pane() {
        let mut tree = leaf(1);
        tree.split(1, 2, Column);
        assert!(tree.replace(2, 7));
        assert_eq!(tree.leaves(), vec![1, 7]);
    }
}
