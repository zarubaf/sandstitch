// SPDX-License-Identifier: Apache-2.0

use parking_lot::RwLock;
use std::fmt::{self, Debug};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::model::connection::port_slice::PortSliceConnections;
use crate::model::mod_inst::HierPathElem;
use crate::model::port::PortDirectionality;
use crate::{ModDef, ModDefCore, Port};

mod connect;
mod export;
mod feedthrough;
mod select;
mod tieoff;
mod trace;

pub use select::Select;
pub(crate) use select::{path_string, selections_covering};

/// Represents a slice of a port, which may be on a module definition or on a
/// module instance.
///
/// A slice is a defined as a contiguous range of bits from `msb` down to `lsb`,
/// inclusive. A slice can be a single bit on the port (`msb` equal to `lsb`),
/// the entire port, or any range in between.
///
/// A slice obtained from a whole port, [`PortSlice::field`] or
/// [`PortSlice::index`] also remembers its typed selection, from which
/// [`PortSlice::port_type`] is derived. Equality and hashing compare the
/// covered bits only, not the selection.
#[derive(Clone)]
pub struct PortSlice {
    pub(crate) port: Port,
    pub(crate) msb: usize,
    pub(crate) lsb: usize,
    /// Typed selection path from the root of the port's type (empty for the
    /// whole port); `None` for a plain bit slice.
    pub(crate) select: Option<Arc<[Select]>>,
}

impl PartialEq for PortSlice {
    fn eq(&self, other: &Self) -> bool {
        self.port == other.port && self.msb == other.msb && self.lsb == other.lsb
    }
}

impl Eq for PortSlice {}

impl Hash for PortSlice {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.port.hash(state);
        self.msb.hash(state);
        self.lsb.hash(state);
    }
}

impl Debug for PortSlice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.debug_string())
    }
}

impl PortSlice {
    /// Divides a port slice into `n` parts of equal bit width, return a vector
    /// of `n` port slices. For example, if a port is 8 bits wide and `n` is 2,
    /// the port will be divided into 2 slices of 4 bits each: `port[3:0]` and
    /// `port[7:4]`. This method panics if the port width is not divisible by
    /// `n`.
    pub fn subdivide(&self, n: usize) -> Vec<Self> {
        let width = self.msb - self.lsb + 1;
        if !width.is_multiple_of(n) {
            panic!(
                "Cannot subdivide {} into {} equal parts.",
                self.debug_string(),
                n
            );
        }
        (0..n)
            .map(move |i| {
                let sub_width = width / n;
                PortSlice {
                    port: self.port.clone(),
                    msb: ((i + 1) * sub_width) - 1 + self.lsb,
                    lsb: (i * sub_width) + self.lsb,
                    select: None,
                }
            })
            .collect()
    }

    /// A sub-range of this slice. The typed selection is kept only when the
    /// whole slice is taken.
    pub(crate) fn slice_with_offset_and_width(&self, offset: usize, width: usize) -> Self {
        assert!(offset + width <= self.width());

        PortSlice {
            port: self.port.clone(),
            msb: self.lsb + offset + width - 1,
            lsb: self.lsb + offset,
            select: if offset == 0 && width == self.width() {
                self.select.clone()
            } else {
                None
            },
        }
    }

    /// Returns the least significant bit of the port slice.
    pub fn lsb(&self) -> usize {
        self.lsb
    }

    /// Returns the most significant bit of the port slice.
    pub fn msb(&self) -> usize {
        self.msb
    }

    /// Returns the width of the port slice.
    pub fn width(&self) -> usize {
        self.msb - self.lsb + 1
    }

    /// Returns the port this slice is derived from.
    pub fn get_port(&self) -> Port {
        self.port.clone()
    }

    pub(crate) fn debug_string(&self) -> String {
        format!("{}[{}:{}]", self.port.debug_string(), self.msb, self.lsb)
    }

    pub(crate) fn get_mod_def_core(&self) -> Arc<RwLock<ModDefCore>> {
        self.port.get_mod_def_core()
    }

    pub(crate) fn get_mod_def(&self) -> ModDef {
        ModDef {
            core: self.get_mod_def_core(),
        }
    }

    pub(crate) fn get_mod_def_where_declared(&self) -> ModDef {
        self.port.get_mod_def_where_declared()
    }

    /// Return the `PortSliceConnections` associated with this port slice, if
    /// any.
    pub(crate) fn get_port_connections(&self) -> Option<PortSliceConnections> {
        let connections = self.port.get_port_connections()?;
        let connections_borrowed = connections.read();
        Some(connections_borrowed.slice(self.msb, self.lsb))
    }

    /// Returns a new `PortSlice` with the same port name, MSB, and LSB, but the
    /// provided hierarchy.
    pub(crate) fn as_mod_inst_port_slice(&self, hierarchy: Vec<HierPathElem>) -> PortSlice {
        PortSlice {
            port: self.port.as_mod_inst_port(hierarchy),
            msb: self.msb,
            lsb: self.lsb,
            select: self.select.clone(),
        }
    }

    /// Returns a new `PortSlice` with the same port name, MSB, and LSB, but a
    /// module definition pointer that corresponds to the module where the port
    /// is declared. For example, if `top` instantiates `a` as `a_inst` and
    /// `a_inst` has a port `x`, calling this function on `top.a_inst.x\[3:2\]`
    /// will effectively return `a.x\[3:2\]` (i.e., the port slice on the
    /// module definition of `a`).
    pub(crate) fn as_mod_def_port_slice(&self) -> PortSlice {
        PortSlice {
            port: self.port.as_mod_def_port(),
            msb: self.msb,
            lsb: self.lsb,
            select: self.select.clone(),
        }
    }

    pub(crate) fn check_validity(&self) {
        if self.msb >= self.port.io().width() {
            panic!(
                "Port slice {} is invalid: msb must be less than the width of the port.",
                self.debug_string()
            );
        } else if self.lsb > self.msb {
            panic!(
                "Port slice {} is invalid: lsb must be less than or equal to msb.",
                self.debug_string()
            );
        }
    }

    /// Returns the instance name corresponding to the port slice, if this is
    /// a port slice on an instance. Otherwise, returns `None`.
    pub(crate) fn get_inst_name(&self) -> Option<String> {
        self.port.inst_name().map(|name| name.to_string())
    }

    /// Returns `(port name, bit index)` pairs describing every bit covered by
    /// this slice, ordered from LSB to MSB.
    pub fn to_bits(&self) -> Vec<(&str, usize)> {
        (self.lsb..=self.msb)
            .map(|i| (self.port.name(), i))
            .collect()
    }

    pub(crate) fn try_merge(&self, other: &PortSlice) -> Option<PortSlice> {
        if self.port == other.port && (self.lsb == (other.msb + 1)) {
            Some(PortSlice {
                port: self.port.clone(),
                msb: self.msb,
                lsb: other.lsb,
                select: None,
            })
        } else {
            None
        }
    }

    pub(crate) fn get_directionality(&self) -> PortDirectionality {
        self.port.get_directionality()
    }
}

/// Indicates that a type can be converted to a `PortSlice`. `Port` and
/// `PortSlice` both implement this trait, which makes it easier to perform the
/// same operations on both.
pub trait ConvertibleToPortSlice {
    fn to_port_slice(&self) -> PortSlice;
}

/// Indicates that a type can be converted to an ordered list of `PortSlice`s.
/// This enables APIs that can operate on a single `Port`/`PortSlice`/`Intf`
/// or on lists of those values.
pub trait ConvertibleToPortSliceVec {
    #[track_caller]
    fn to_port_slice_vec(&self) -> Vec<PortSlice>;
}

impl ConvertibleToPortSlice for PortSlice {
    fn to_port_slice(&self) -> PortSlice {
        self.clone()
    }
}

impl<T: ConvertibleToPortSlice> ConvertibleToPortSliceVec for T {
    fn to_port_slice_vec(&self) -> Vec<PortSlice> {
        vec![self.to_port_slice()]
    }
}

impl<T: ConvertibleToPortSliceVec> ConvertibleToPortSliceVec for [T] {
    fn to_port_slice_vec(&self) -> Vec<PortSlice> {
        let mut result = Vec::new();
        for item in self {
            result.extend(item.to_port_slice_vec());
        }
        result
    }
}

impl<T: ConvertibleToPortSliceVec> ConvertibleToPortSliceVec for Vec<T> {
    fn to_port_slice_vec(&self) -> Vec<PortSlice> {
        self.as_slice().to_port_slice_vec()
    }
}

impl<T: ConvertibleToPortSliceVec, const N: usize> ConvertibleToPortSliceVec for [T; N] {
    fn to_port_slice_vec(&self) -> Vec<PortSlice> {
        self.as_slice().to_port_slice_vec()
    }
}
