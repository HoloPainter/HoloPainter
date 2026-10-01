use std::{
    collections::VecDeque,
    sync::mpsc::{self, Receiver, TryRecvError},
};

use eframe::egui_wgpu::wgpu;

use crate::renderer::{ColorSampleRequest, CompletedColorSample, gpu::frame::GpuFrame};

const READBACK_SLOT_COUNT: usize = 4;
const READBACK_BYTES_PER_ROW: u32 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
const READBACK_BUFFER_SIZE: u64 = READBACK_BYTES_PER_ROW as u64;

pub(crate) struct ColorSampler {
    slots: Vec<ColorSampleSlot>,
    completed: VecDeque<CompletedColorSample>,
}

struct ColorSampleSlot {
    buffer: wgpu::Buffer,
    state: ColorSampleSlotState,
}

enum ColorSampleSlotState {
    Idle,
    Waiting {
        request: ColorSampleRequest,
        receiver: Receiver<Result<(), wgpu::BufferAsyncError>>,
    },
}

impl ColorSampler {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let slots = (0..READBACK_SLOT_COUNT)
            .map(|_| ColorSampleSlot {
                buffer: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("color_sample_readback_buffer"),
                    size: READBACK_BUFFER_SIZE,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
                state: ColorSampleSlotState::Idle,
            })
            .collect();
        Self {
            slots,
            completed: VecDeque::new(),
        }
    }

    pub(crate) fn enqueue(
        &mut self,
        frame: &mut GpuFrame,
        source: &wgpu::Texture,
        source_size: [u32; 2],
        position: [u32; 2],
        request: ColorSampleRequest,
    ) {
        if position[0] >= source_size[0] || position[1] >= source_size[1] {
            self.completed.push_back(CompletedColorSample {
                request,
                result: Err("color sample position is outside the source texture".to_owned()),
            });
            return;
        }

        let Some(slot) = self
            .slots
            .iter_mut()
            .find(|slot| matches!(slot.state, ColorSampleSlotState::Idle))
        else {
            self.completed.push_back(CompletedColorSample {
                request,
                result: Err("color sample readback queue is full".to_owned()),
            });
            return;
        };

        frame.encoder().copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: source,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: position[0],
                    y: position[1],
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &slot.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(READBACK_BYTES_PER_ROW),
                    rows_per_image: Some(1),
                },
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let (sender, receiver) = mpsc::channel();
        frame.map_buffer_on_submit(&slot.buffer, wgpu::MapMode::Read, .., move |result| {
            let _ = sender.send(result);
        });
        slot.state = ColorSampleSlotState::Waiting { request, receiver };
    }

    pub(crate) fn complete_with_error(&mut self, request: ColorSampleRequest, error: String) {
        self.completed.push_back(CompletedColorSample {
            request,
            result: Err(error),
        });
    }

    pub(crate) fn poll(&mut self, device: &wgpu::Device) -> Vec<CompletedColorSample> {
        self.poll_with(device, wgpu::PollType::Poll)
    }

    pub(crate) fn wait(&mut self, device: &wgpu::Device) -> Vec<CompletedColorSample> {
        self.poll_with(device, wgpu::PollType::wait_indefinitely())
    }

    fn poll_with(
        &mut self,
        device: &wgpu::Device,
        poll_type: wgpu::PollType,
    ) -> Vec<CompletedColorSample> {
        if self
            .slots
            .iter()
            .any(|slot| matches!(slot.state, ColorSampleSlotState::Waiting { .. }))
            && let Err(error) = device.poll(poll_type)
        {
            let message = format!("GPU poll failed during color sampling: {error:?}");
            for slot in &mut self.slots {
                let state = std::mem::replace(&mut slot.state, ColorSampleSlotState::Idle);
                if let ColorSampleSlotState::Waiting { request, .. } = state {
                    self.completed.push_back(CompletedColorSample {
                        request,
                        result: Err(message.clone()),
                    });
                }
            }
        }

        for slot in &mut self.slots {
            let outcome = match &slot.state {
                ColorSampleSlotState::Idle => continue,
                ColorSampleSlotState::Waiting { receiver, .. } => receiver.try_recv(),
            };
            match outcome {
                Ok(Ok(())) => {
                    let state = std::mem::replace(&mut slot.state, ColorSampleSlotState::Idle);
                    let ColorSampleSlotState::Waiting { request, .. } = state else {
                        unreachable!("waiting color sample state was just observed")
                    };
                    let result = match slot.buffer.slice(0..4).get_mapped_range() {
                        Ok(mapped) if mapped.len() >= 4 => {
                            Ok([mapped[0], mapped[1], mapped[2], mapped[3]])
                        }
                        Ok(_) => Err("mapped color sample buffer is too small".to_owned()),
                        Err(error) => Err(format!("color sample mapped range failed: {error}")),
                    };
                    slot.buffer.unmap();
                    self.completed
                        .push_back(CompletedColorSample { request, result });
                }
                Ok(Err(error)) => {
                    let state = std::mem::replace(&mut slot.state, ColorSampleSlotState::Idle);
                    let ColorSampleSlotState::Waiting { request, .. } = state else {
                        unreachable!("waiting color sample state was just observed")
                    };
                    self.completed.push_back(CompletedColorSample {
                        request,
                        result: Err(format!("color sample mapping failed: {error:?}")),
                    });
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    let state = std::mem::replace(&mut slot.state, ColorSampleSlotState::Idle);
                    let ColorSampleSlotState::Waiting { request, .. } = state else {
                        unreachable!("waiting color sample state was just observed")
                    };
                    self.completed.push_back(CompletedColorSample {
                        request,
                        result: Err("color sample callback was dropped".to_owned()),
                    });
                }
            }
        }

        self.completed.drain(..).collect()
    }
}
