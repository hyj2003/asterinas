use ostd::Pod;
use super::header::{VirtioGPUCtrlHdr, VirtioGPUCtrlType};

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Pod)]
pub struct VirtioGPUCursorPos {
    pub scanout_id: u32,
    pub x: u32,
    pub y: u32,
    pub padding: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod)]
pub struct VirtioGPUUpdateCursor {
    pub hdr: VirtioGPUCtrlHdr,
    pub pos: VirtioGPUCursorPos,
    pub resource_id: u32,
    pub hot_x: u32,
    pub hot_y: u32,
    pub padding: u32,
}

/* Update cursor with new resources */
impl VirtioGPUUpdateCursor {
    pub fn update_cursor(pos: VirtioGPUCursorPos, resource_id: u32, padding: u32) -> Self {
        VirtioGPUUpdateCursor {
            hdr: VirtioGPUCtrlHdr::from_type(VirtioGPUCtrlType::VIRTIO_GPU_CMD_UPDATE_CURSOR),
            pos, resource_id,
            hot_x: 0, 
            hot_y: 0, 
            padding,
        }
    }

    pub fn move_cursor(hot_x: u32, hot_y: u32, padding: u32) -> Self {
        VirtioGPUUpdateCursor {
            hdr: VirtioGPUCtrlHdr::from_type(VirtioGPUCtrlType::VIRTIO_GPU_CMD_MOVE_CURSOR),
            pos: VirtioGPUCursorPos::default(),
            resource_id: 0,
            hot_x, hot_y, padding,
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod)]
pub struct VirtioGPURespUpdateCursor {
    hdr: VirtioGPUCtrlHdr,
}

impl VirtioGPURespUpdateCursor {
    pub fn new() -> Self {
        VirtioGPURespUpdateCursor {
            hdr: VirtioGPUCtrlHdr::from_type(VirtioGPUCtrlType::VIRTIO_GPU_RESP_OK_NODATA),
        }
    }

    pub fn header(&self) -> VirtioGPUCtrlHdr {
        self.hdr
    }
}
