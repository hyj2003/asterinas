use core::hint::spin_loop;

use alloc::{
    boxed::Box,
    sync::Arc,
};
use log::{debug, info};
use ostd::early_println;
use ostd::mm::VmIo;
use ostd::task::scheduler::info;
use ostd::{
    sync::SpinLock,
    mm::{DmaDirection, DmaStream, DmaStreamSlice, FrameAllocOptions},
    trap::TrapFrame,
};
use crate::{
    device::VirtioDeviceError, 
    queue::VirtQueue, 
    transport::{ConfigManager, VirtioTransport}
};

use super::{
    config::{GPUFeatures, VirtioGPUConfig},
    header::{VirtioGPUCtrlHdr, VirtioGPUCtrlType},
    control::{VirtioGPURespDisplayInfo, VirtioGPUGetEdid},
    cursor::{VirtioGPUCursorPos, VirtioGPUUpdateCursor, VirtioGPURespUpdateCursor},
};

const REQ_SIZE: usize = 16;
const RESP_SIZE: usize = 1;

pub struct GPUDevice {
    config_manager: ConfigManager<VirtioGPUConfig>,
    transport: SpinLock<Box<dyn VirtioTransport>>,
    control_queue: SpinLock<VirtQueue>,
    cursor_queue: SpinLock<VirtQueue>,
    controlq_request: DmaStream,
    controlq_response: DmaStream,
    cursorq_request: DmaStream,          // TODO: ?
    cursorq_response: DmaStream,
    // callback                             // FIXME: necessary?
}

impl GPUDevice {
    const QUEUE_SIZE: u16 = 64;

    pub fn negotiate_features(features: u64) -> u64 {
        let mut features = GPUFeatures::from_bits_truncate(features);
        debug!("GPUFeature negotiate: {:?}", features);
        // tmep: not support 3D mode
        features.remove(GPUFeatures::VIRTIO_GPU_F_VIRGL);
        features.remove(GPUFeatures::VIRTIO_GPU_F_CONTEXT_INIT);
        features.bits()
    }

    pub fn init(mut transport: Box<dyn VirtioTransport>) -> Result<(), VirtioDeviceError> {
        let config_manager = VirtioGPUConfig::new_manager(transport.as_ref());
        early_println!("[INFO] GPU Config = {:?}", config_manager.read_config());

        // init queue
        const CONTROL_QUEUE_INDEX: u16 = 0;
        const CURSOR_QUEUE_INDEX: u16 = 1;
        let control_queue =
            SpinLock::new(VirtQueue::new(CONTROL_QUEUE_INDEX, Self::QUEUE_SIZE, transport.as_mut()).unwrap());
        let cursor_queue =
            SpinLock::new(VirtQueue::new(CURSOR_QUEUE_INDEX, Self::QUEUE_SIZE, transport.as_mut()).unwrap());

        // init buffer
        let controlq_request = {
            let segment = FrameAllocOptions::new().alloc_segment(1).unwrap();
            DmaStream::map(segment.into(), DmaDirection::Bidirectional, false).unwrap()
        };
        let controlq_response = {
            let segment = FrameAllocOptions::new().alloc_segment(1).unwrap();
            DmaStream::map(segment.into(), DmaDirection::Bidirectional, false).unwrap()
        };
        let cursorq_request = {
            let segment = FrameAllocOptions::new().alloc_segment(1).unwrap();
            DmaStream::map(segment.into(), DmaDirection::Bidirectional, false).unwrap()
        };
        let cursorq_response = {
            let segment = FrameAllocOptions::new().alloc_segment(1).unwrap();
            DmaStream::map(segment.into(), DmaDirection::Bidirectional, false).unwrap()
        };

        // init device
        let device = Arc::new(Self {
            config_manager,
            transport: SpinLock::new(transport),
            control_queue,
            cursor_queue,
            controlq_request,
            controlq_response,
            cursorq_request,
            cursorq_response,
            // TODO: ...
        });

        // Handle interrupt (ref. block device)
        let cloned_device = device.clone();
        let handle_irq = move |_: &TrapFrame| {
            cloned_device.handle_irq();
        };

        let cloned_device = device.clone();
        let handle_config_change = move |_: &TrapFrame| {
            cloned_device.handle_config_change();
        };

        // Register callback
        let mut transport = device.transport.lock();
        transport
            .register_cfg_callback(Box::new(handle_config_change))
            .unwrap();
        transport
            .register_queue_callback(0, Box::new(handle_irq), false)
            .unwrap();
        transport.finish_init();

        Ok(())
    }

    fn handle_irq(&self) {
        info!("Virtio-GPU handle irq");
        // TODO
    }

    fn handle_config_change(&self) {
        info!("Virtio-GPU handle config change");
        // TODO
    }

    fn request_display_info(&self) -> Result<VirtioGPURespDisplayInfo, VirtioDeviceError> {
        let req_slice = {
            let req_slice = DmaStreamSlice::new(&self.controlq_request, 0, REQ_SIZE);
            let req: VirtioGPUCtrlHdr = VirtioGPUCtrlHdr {
                ctrl_type: VirtioGPUCtrlType::VIRTIO_GPU_CMD_GET_DISPLAY_INFO as u32,
                ..VirtioGPUCtrlHdr::default()
            };
            req_slice.write_val(0, &req).unwrap();
            req_slice.sync().unwrap();
            req_slice
        };

        let resp_slice = {
            let resp_slice = DmaStreamSlice::new(&self.controlq_response, 0, RESP_SIZE);
            resp_slice.write_val(0, &VirtioGPURespDisplayInfo::default()).unwrap();
            resp_slice
        };
        
        let mut control_queue = self.control_queue.disable_irq().lock();
        control_queue
            .add_dma_buf(&[&req_slice], &[&resp_slice])
            .expect("add queue failed");

        if control_queue.should_notify() {
            control_queue.notify();
        }

        while !control_queue.can_pop() {
            spin_loop();
        }
        control_queue.pop_used().expect("pop used failed");

        resp_slice.sync().unwrap();
        let resp: VirtioGPURespDisplayInfo = resp_slice.read_val(0).unwrap();

        if resp.header().ctrl_type == VirtioGPUCtrlType::VIRTIO_GPU_RESP_OK_DISPLAY_INFO as u32 {
            Ok(resp)
        } else {
            Err(VirtioDeviceError::QueueUnknownError)
        }
    }


    /// use when cursor is updated with new resources
    fn request_cursor_update(
        &self, pos: VirtioGPUCursorPos, 
        resource_id: u32, 
        padding: u32
    ) -> Result<VirtioGPURespUpdateCursor, VirtioDeviceError> {
        info!("[CursorUpdate] Transfer cursor update with resource_id {:?}", resource_id);
        let req_slice = {
            let req_slice = DmaStreamSlice::new(&self.cursorq_request, 0, REQ_SIZE);
            let req_data: VirtioGPUUpdateCursor = VirtioGPUUpdateCursor::update_cursor(pos, resource_id, padding);
            req_slice.write_val(0, &req_data).unwrap();
            req_slice.sync().unwrap();
            req_slice
        };

        let resp_slice = {
            let resp_slice = DmaStreamSlice::new(&self.cursorq_response, 0, RESP_SIZE);
            resp_slice.write_val(0, &VirtioGPURespUpdateCursor::new()).unwrap();
            resp_slice
        };

        let mut cursor_queue = self.cursor_queue.disable_irq().lock();
        cursor_queue
            .add_dma_buf(&[&req_slice], &[&resp_slice])
            .expect("[CursorUpdate] add queue failed");

        if cursor_queue.should_notify() {
            cursor_queue.notify();
        }
        while !cursor_queue.can_pop() {
            spin_loop();
        }
        cursor_queue.pop_used().expect("[CursorUpdate] pop used failed");

        resp_slice.sync().unwrap();
        let resp: VirtioGPURespUpdateCursor = resp_slice.read_val(0).unwrap();

        if resp.header().ctrl_type == VirtioGPUCtrlType::VIRTIO_GPU_RESP_OK_NODATA as u32 {
            Ok(resp)
        } else {
            Err(VirtioDeviceError::QueueUnknownError)
        }
    }


    /// use when cursor only moves
    fn request_cursor_move(
        &self,
        hot_x: u32,
        hot_y: u32,
        padding: u32
    ) -> Result<VirtioGPURespUpdateCursor, VirtioDeviceError> {
        info!("[CursorMove] Transfer cursor move to ({:?}, {:?})", hot_x, hot_y);
        let req_slice = {
            let req_slice = DmaStreamSlice::new(&self.cursorq_request, 0, REQ_SIZE);
            let req_data: VirtioGPUUpdateCursor = VirtioGPUUpdateCursor::move_cursor(hot_x, hot_y, padding);
            req_slice.write_val(0, &req_data).unwrap();
            req_slice.sync().unwrap();
            req_slice
        };

        let resp_slice = {
            let resp_slice = DmaStreamSlice::new(&self.cursorq_response, 0, RESP_SIZE);
            resp_slice.write_val(0, &VirtioGPURespUpdateCursor::new()).unwrap();
            resp_slice
        };

        let mut cursor_queue = self.cursor_queue.disable_irq().lock();
        cursor_queue
            .add_dma_buf(&[&req_slice], &[&resp_slice])
            .expect("[CursorUpdate] add queue failed");

        if cursor_queue.should_notify() {
            cursor_queue.notify();
        }
        while !cursor_queue.can_pop() {
            spin_loop();
        }
        cursor_queue.pop_used().expect("[CursorUpdate] pop used failed");

        resp_slice.sync().unwrap();
        let resp: VirtioGPURespUpdateCursor = resp_slice.read_val(0).unwrap();

        if resp.header().ctrl_type == VirtioGPUCtrlType::VIRTIO_GPU_RESP_OK_NODATA as u32 {
            Ok(resp)
        } else {
            Err(VirtioDeviceError::QueueUnknownError)
        }
    }

}

