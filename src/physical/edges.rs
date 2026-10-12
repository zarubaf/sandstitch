// SPDX-License-Identifier: Apache-2.0

pub const WEST_EDGE_INDEX: usize = 0;
pub const LEFT_EDGE_INDEX: usize = 0;
pub const NORTH_EDGE_INDEX: usize = 1;
pub const TOP_EDGE_INDEX: usize = 1;
pub const EAST_EDGE_INDEX: usize = 2;
pub const RIGHT_EDGE_INDEX: usize = 2;
pub const SOUTH_EDGE_INDEX: usize = 3;
pub const BOTTOM_EDGE_INDEX: usize = 3;

#[macro_export]
macro_rules! for_each_edge_direction {
    ($macro_name:ident) => {
        $macro_name!(west, $crate::physical::edges::WEST_EDGE_INDEX);
        $macro_name!(left, $crate::physical::edges::LEFT_EDGE_INDEX);
        $macro_name!(north, $crate::physical::edges::NORTH_EDGE_INDEX);
        $macro_name!(top, $crate::physical::edges::TOP_EDGE_INDEX);
        $macro_name!(east, $crate::physical::edges::EAST_EDGE_INDEX);
        $macro_name!(right, $crate::physical::edges::RIGHT_EDGE_INDEX);
        $macro_name!(south, $crate::physical::edges::SOUTH_EDGE_INDEX);
        $macro_name!(bottom, $crate::physical::edges::BOTTOM_EDGE_INDEX);
    };
}
