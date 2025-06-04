use bitvec::{array::BitArray, BitArr};

use crate::types::{DevInfo, PortInfo};

#[derive(Clone, Copy)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[cfg_attr(not(feature = "defmt"), derive(Debug))]
pub struct DeviceHandle {
    address: u8,
    max_packet_size: u16,
    //TODO: this is not a parent any more
    parent: DevInfo,
}

impl DeviceHandle {
    pub fn address(&self) -> u8 {
        self.address
    }

    pub fn max_packet_size(&self) -> u16 {
        self.max_packet_size
    }

    pub(crate) fn dev_info(&self) -> DevInfo {
        self.parent
    }
}

#[cfg_attr(not(feature = "defmt"), derive(Debug))]
pub struct DeviceDisconnectMask {
    mask: BitArr!(for 128, in u8),
}

#[cfg(feature = "defmt")]
impl defmt::Format for DeviceDisconnectMask {
    fn format(&self, fmt: defmt::Formatter) {
        let mut indices = self.mask.iter_ones().peekable();
        if indices.peek().is_none() {
            defmt::write!(fmt, "[]");
            return;
        }
        defmt::write!(fmt, "[");
        for (i, idx) in indices.enumerate() {
            if i > 0 {
                defmt::write!(fmt, ", ");
            }
            defmt::write!(fmt, "{}", idx);
        }
        defmt::write!(fmt, "]");
    }
}

impl DeviceDisconnectMask {
    pub fn new() -> Self {
        Self {
            mask: BitArray::ZERO,
        }
    }
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.mask.iter_ones()
    }

    pub(crate) fn remove(&mut self, addr: usize) {
        self.mask.set(addr, false);
    }
}

pub(crate) struct DeviceAddressManager<const NR_DEVICES: usize> {
    // 1-based indexing
    info: [PortInfo; NR_DEVICES],
}

impl<const NR_DEVICES: usize> DeviceAddressManager<NR_DEVICES> {
    // Construct an allocator with all addresses except 0 occupied.
    pub fn new() -> Self {
        Self {
            info: [PortInfo::invalid(); NR_DEVICES],
        }
    }

    pub fn alloc_device_address(&mut self, max_packet_size: u16, parent: DevInfo) -> DeviceHandle {
        debug_assert!(!parent.port().is_empty());
        for i in 0..NR_DEVICES {
            if self.info[i].is_empty() {
                self.info[i] = parent.port();
                return DeviceHandle {
                    address: i as u8 + 1,
                    max_packet_size,
                    parent,
                };
            }
        }
        panic!("No address available");
    }

    pub fn free_address(&mut self, device_handle: DeviceHandle) {
        debug_assert!(!self.info[device_handle.address as usize - 1].is_empty());
        self.info[device_handle.address as usize - 1] = PortInfo::invalid();
    }

    pub fn free_all_addresses(&mut self) -> DeviceDisconnectMask {
        let mut mask = DeviceDisconnectMask::new();
        for i in 0..NR_DEVICES {
            if !self.info[i].is_empty() {
                mask.mask.set(i + 1, true);
                self.info[i] = PortInfo::invalid();
            }
        }
        mask
    }

    // TODO: fix the amazing union-find to not take a DevInfo
    fn find_index(&self, dev_info: PortInfo) -> Option<usize> {
        for i in 0..NR_DEVICES {
            if self.info[i] == dev_info {
                return Some(i);
            }
        }
        None
    }

    pub fn free_subtree(&mut self, dev_info: PortInfo) -> DeviceDisconnectMask {
        let mut mask = DeviceDisconnectMask::new();
        let idx = self.find_index(dev_info);

        if let Some(idx) = idx {
            debug_assert!(!self.info[idx].is_empty());
            self.info[idx] = PortInfo::invalid();
            mask.mask.set(idx + 1, true);
        } else {
            warn!("freeing non-existent device {}", dev_info);
            return mask;
        }

        // Create a union-find data structure to track connected components
        let mut parent = [0; NR_DEVICES];
        let mut rank = [0; NR_DEVICES];

        // Initialize parent array
        for i in 0..NR_DEVICES {
            parent[i] = i;
        }

        // Find root of component containing x with path compression
        fn find(parent: &mut [usize], x: usize) -> usize {
            if parent[x] != x {
                parent[x] = find(parent, parent[x]);
            }
            parent[x]
        }

        // Union components containing x and y
        fn union(parent: &mut [usize], rank: &mut [usize], x: usize, y: usize) {
            let x_root = find(parent, x);
            let y_root = find(parent, y);

            if x_root != y_root {
                if rank[x_root] < rank[y_root] {
                    parent[x_root] = y_root;
                } else if rank[x_root] > rank[y_root] {
                    parent[y_root] = x_root;
                } else {
                    parent[y_root] = x_root;
                    rank[x_root] += 1;
                }
            }
        }

        // Build connected components by connecting devices to their parents
        for i in 0..NR_DEVICES {
            if !self.info[i].is_empty() {
                if let Some(parent_addr) = self.info[i].parent_addr() {
                    if parent_addr > 0 {
                        union(&mut parent, &mut rank, i, parent_addr as usize - 1);
                    }
                }
            }
        }

        // Find root component (connected to root hub)
        let mut root_component = None;
        for i in 0..NR_DEVICES {
            if !self.info[i].is_empty() {
                if self.info[i].parent_addr() == Some(0) {
                    root_component = Some(find(&mut parent, i));
                    break;
                }
            }
        }

        // Create mask of disconnected devices
        if let Some(root) = root_component {
            for i in 0..NR_DEVICES {
                if !self.info[i].is_empty() && find(&mut parent, i) != root {
                    trace!(
                        "freeing disconnected device {} with addr {}",
                        self.info[i],
                        i + 1
                    );
                    mask.mask.set(i + 1, true);
                    self.info[i] = PortInfo::invalid();
                }
            }
        }

        mask
    }
}
