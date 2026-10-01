use eframe::egui_wgpu::wgpu;

use crate::renderer::features::brush::types::{
    BrushDabInstance, CurrentDabBatchHeader, SurfaceDabGpu,
};

const INITIAL_DAB_CAPACITY: u32 = 64;

pub(crate) struct BrushGpuResources {
    instances: wgpu::Buffer,
    instance_capacity: u32,
    surface_dabs: wgpu::Buffer,
    surface_dab_capacity: u32,
    current_dab_batch: wgpu::Buffer,
    current_dab_batch_capacity_bytes: u64,
    reallocation_count: usize,
}

pub(crate) struct BrushResourceContext<'a> {
    resources: &'a mut BrushGpuResources,
    device: &'a wgpu::Device,
}

impl<'a> BrushResourceContext<'a> {
    pub(crate) fn new(resources: &'a mut BrushGpuResources, device: &'a wgpu::Device) -> Self {
        Self { resources, device }
    }

    pub(crate) fn ensure_instance_capacity(&mut self, needed: u32) {
        self.resources.ensure_instance_capacity(self.device, needed);
    }

    pub(crate) fn ensure_surface_dab_capacity(&mut self, needed: u32) {
        self.resources
            .ensure_surface_dab_capacity(self.device, needed);
    }

    pub(crate) fn ensure_current_dab_batch_capacity(&mut self, item_size: u64, item_count: u32) {
        self.resources
            .ensure_current_dab_batch_capacity(self.device, item_size, item_count);
    }

    pub(crate) fn instances(&self) -> &wgpu::Buffer {
        self.resources.instances()
    }

    pub(crate) fn surface_dabs(&self) -> &wgpu::Buffer {
        self.resources.surface_dabs()
    }

    pub(crate) fn current_dab_batch(&self) -> &wgpu::Buffer {
        self.resources.current_dab_batch()
    }
}

impl BrushGpuResources {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let instances = create_stroke_instance_buffer(device, INITIAL_DAB_CAPACITY);
        let surface_dabs = create_surface_dab_buffer(device, INITIAL_DAB_CAPACITY);
        let current_dab_batch_capacity_bytes = initial_current_dab_batch_capacity_bytes();
        let current_dab_batch =
            create_current_dab_batch_buffer(device, current_dab_batch_capacity_bytes);

        Self {
            instances,
            instance_capacity: INITIAL_DAB_CAPACITY,
            surface_dabs,
            surface_dab_capacity: INITIAL_DAB_CAPACITY,
            current_dab_batch,
            current_dab_batch_capacity_bytes,
            reallocation_count: 0,
        }
    }

    pub(crate) fn ensure_instance_capacity(&mut self, device: &wgpu::Device, needed: u32) {
        if needed <= self.instance_capacity {
            return;
        }
        self.instance_capacity = needed.next_power_of_two();
        self.instances = create_stroke_instance_buffer(device, self.instance_capacity);
        self.reallocation_count = self.reallocation_count.saturating_add(1);
    }

    pub(crate) fn ensure_surface_dab_capacity(&mut self, device: &wgpu::Device, needed: u32) {
        if needed <= self.surface_dab_capacity {
            return;
        }
        self.surface_dab_capacity = needed.next_power_of_two();
        self.surface_dabs = create_surface_dab_buffer(device, self.surface_dab_capacity);
        self.reallocation_count = self.reallocation_count.saturating_add(1);
    }

    pub(crate) fn ensure_current_dab_batch_capacity(
        &mut self,
        device: &wgpu::Device,
        item_size: u64,
        item_count: u32,
    ) {
        let needed = std::mem::size_of::<CurrentDabBatchHeader>() as u64
            + item_size * u64::from(item_count.max(1));
        if needed <= self.current_dab_batch_capacity_bytes {
            return;
        }
        self.current_dab_batch_capacity_bytes = needed.next_power_of_two();
        self.current_dab_batch =
            create_current_dab_batch_buffer(device, self.current_dab_batch_capacity_bytes);
        self.reallocation_count = self.reallocation_count.saturating_add(1);
    }

    pub(crate) fn instances(&self) -> &wgpu::Buffer {
        &self.instances
    }

    pub(crate) fn surface_dabs(&self) -> &wgpu::Buffer {
        &self.surface_dabs
    }

    pub(crate) fn current_dab_batch(&self) -> &wgpu::Buffer {
        &self.current_dab_batch
    }

    pub(crate) fn take_reallocation_count(&mut self) -> usize {
        std::mem::take(&mut self.reallocation_count)
    }
}

fn initial_current_dab_batch_capacity_bytes() -> u64 {
    std::mem::size_of::<CurrentDabBatchHeader>() as u64
        + std::mem::size_of::<SurfaceDabGpu>() as u64 * u64::from(INITIAL_DAB_CAPACITY)
}

fn create_stroke_instance_buffer(device: &wgpu::Device, capacity: u32) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("stroke_instances"),
        size: (capacity as usize * std::mem::size_of::<BrushDabInstance>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn create_surface_dab_buffer(device: &wgpu::Device, capacity: u32) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("surface_dabs"),
        size: (capacity as usize * std::mem::size_of::<SurfaceDabGpu>()) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn create_current_dab_batch_buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("brush_current_dab_batch"),
        size: size.max(1),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_current_batch_supports_surface_dabs_without_growth() {
        let needed = std::mem::size_of::<CurrentDabBatchHeader>() as u64
            + std::mem::size_of::<SurfaceDabGpu>() as u64 * u64::from(INITIAL_DAB_CAPACITY);
        assert_eq!(initial_current_dab_batch_capacity_bytes(), needed);
    }
}
