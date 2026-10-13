//! Rounding layout to the device-pixel grid.
//!
//! Layout runs in CSS pixels. Rounding exists so that box edges land on whole
//! *device* pixels and adjacent boxes never gain gaps or overlaps. Rounding in
//! CSS pixels is only equivalent to that when the device pixel ratio is 1.
//! At other ratios it destroys device-pixel-snapped lengths: Stylo snaps a
//! `1px` border at 125% to one device pixel (0.8 CSS px), and rounding that
//! to whole CSS pixels makes it 0 or 1 depending on where the box sits, so
//! whole border edges disappear.
//!
//! [`round_layout_to_device_pixels`] is `taffy::round_layout` with every
//! rounding done on the device-pixel grid (`scale` = device pixels per CSS
//! pixel). At a scale of 1 it is identical to `taffy::round_layout`.

use taffy::{Layout, NodeId, RoundTree};

/// Round `v` (CSS px) to the nearest device pixel, returned in CSS px.
#[inline(always)]
pub(crate) fn round_to_device(v: f32, scale: f32) -> f32 {
    (v * scale).round() / scale
}

pub(crate) fn round_layout_to_device_pixels(tree: &mut impl RoundTree, root: NodeId, scale: f32) {
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    round_inner(tree, root, 0.0, 0.0, scale);
}

fn round_inner(tree: &mut impl RoundTree, node_id: NodeId, parent_x: f32, parent_y: f32, scale: f32) {
    let round = |v: f32| round_to_device(v, scale);
    let unrounded = tree.get_unrounded_layout(node_id);
    let mut layout: Layout = unrounded;

    let x = parent_x + unrounded.location.x;
    let y = parent_y + unrounded.location.y;
    let w = unrounded.size.width;
    let h = unrounded.size.height;

    // Outer edges are rounded in absolute coordinates, inner edges relative to
    // them (as in `taffy::round_layout`), so adjacent boxes never overlap or gap.
    layout.location.x = round(x) - round(parent_x);
    layout.location.y = round(y) - round(parent_y);
    layout.size.width = round(x + w) - round(x);
    layout.size.height = round(y + h) - round(y);
    layout.scrollbar_size.width = round(unrounded.scrollbar_size.width);
    layout.scrollbar_size.height = round(unrounded.scrollbar_size.height);
    layout.border.left = round(x + unrounded.border.left) - round(x);
    layout.border.right = round(x + w) - round(x + w - unrounded.border.right);
    layout.border.top = round(y + unrounded.border.top) - round(y);
    layout.border.bottom = round(y + h) - round(y + h - unrounded.border.bottom);
    layout.padding.left = round(x + unrounded.padding.left) - round(x);
    layout.padding.right = round(x + w) - round(x + w - unrounded.padding.right);
    layout.padding.top = round(y + unrounded.padding.top) - round(y);
    layout.padding.bottom = round(y + h) - round(y + h - unrounded.padding.bottom);
    let overflow = unrounded.scrollable_overflow_rect;
    layout.scrollable_overflow_rect.left = round(x + overflow.left) - round(x);
    layout.scrollable_overflow_rect.right = round(x + overflow.right) - round(x);
    layout.scrollable_overflow_rect.top = round(y + overflow.top) - round(y);
    layout.scrollable_overflow_rect.bottom = round(y + overflow.bottom) - round(y);

    tree.set_final_layout(node_id, &layout);

    // In-flow children here; out-of-flow boxes via their containing block's
    // hoisted list, so each node is visited once with the right offset.
    for index in 0..tree.child_count(node_id) {
        let child = tree.get_child_id(node_id, index);
        if !tree.is_out_of_flow(child) {
            round_inner(tree, child, x, y, scale);
        }
    }
    for index in 0..tree.hoisted_child_count(node_id) {
        let child = tree.get_hoisted_child_id(node_id, index);
        round_inner(tree, child, x, y, scale);
    }
}

#[cfg(test)]
mod tests {
    use super::round_to_device;

    #[test]
    fn snaps_to_device_pixels() {
        // 125%: a device-snapped 1px border (0.8 CSS px) keeps its width
        // wherever the box sits.
        for x in [0.0f32, 0.4, 10.0, 10.3, 540.4, 541.6] {
            let width = round_to_device(x + 0.8, 1.25) - round_to_device(x, 1.25);
            assert!((width * 1.25 - 1.0).abs() < 1e-4, "x={x}: {width}");
        }
        // At a scale of 1 it is plain rounding.
        assert_eq!(round_to_device(10.4, 1.0), 10.0);
        assert_eq!(round_to_device(10.6, 1.0), 11.0);
    }
}
