use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapturedInput {
    Text(opennow_streamer_protocol::text_input::UnicodeText),
    Key {
        virtual_key: u16,
        modifiers: u16,
        pressed: bool,
    },
    MouseMove {
        delta_x: i16,
        delta_y: i16,
    },
    MouseAbsolute {
        x: u16,
        y: u16,
        width: u16,
        height: u16,
    },
    MouseButton {
        button: u8,
        pressed: bool,
    },
    MouseWheel {
        delta_x: i16,
        delta_y: i16,
    },
    Gamepad {
        controller_id: u8,
        bitmap: u16,
        buttons: u16,
        left_trigger: u8,
        right_trigger: u8,
        left_stick_x: i16,
        left_stick_y: i16,
        right_stick_x: i16,
        right_stick_y: i16,
    },
}

const CAPTURED_INPUT_CAPACITY: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedInputSample {
    pub input: CapturedInput,
    pub captured_at: Instant,
}

#[derive(Debug, Default)]
pub struct CapturedInputQueue {
    pending: Mutex<VecDeque<CapturedInputSample>>,
    overflowed: AtomicBool,
    text_ready: AtomicBool,
    text_generation: AtomicU64,
    text_slot: opennow_streamer_protocol::text_input::TextInputSlot,
}

impl CapturedInputQueue {
    pub fn set_text_ready(&self, generation: u64, ready: bool) {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = self.text_generation.load(Ordering::Relaxed);
        if generation.wrapping_sub(previous) > u64::MAX / 2 {
            return;
        }
        self.text_generation.store(generation, Ordering::Relaxed);
        self.text_ready.store(ready, Ordering::Release);
        if !ready || generation != previous {
            self.text_slot.cancel();
            pending.retain(|sample| !matches!(sample.input, CapturedInput::Text(_)));
        }
    }

    pub fn submit_text(
        &self,
        bytes: &[u8],
    ) -> Result<(), opennow_streamer_protocol::text_input::TextInputError> {
        use opennow_streamer_protocol::text_input::TextInputError;
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.text_ready.load(Ordering::Acquire) {
            return Err(TextInputError::Unavailable);
        }
        if pending.len() >= CAPTURED_INPUT_CAPACITY {
            return Err(TextInputError::Busy);
        }
        let text = self.text_slot.submit(bytes)?;
        pending.push_back(CapturedInputSample {
            input: CapturedInput::Text(text),
            captured_at: Instant::now(),
        });
        Ok(())
    }

    pub fn discard_text(&self) {
        self.text_slot.cancel();
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|sample| !matches!(sample.input, CapturedInput::Text(_)));
    }

    pub fn push(&self, input: CapturedInput) {
        self.push_sample(CapturedInputSample {
            input,
            captured_at: Instant::now(),
        });
    }

    pub fn push_sample(&self, sample: CapturedInputSample) {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(&sample.input, CapturedInput::MouseAbsolute { .. })
            && matches!(
                pending.back(),
                Some(CapturedInputSample {
                    input: CapturedInput::MouseAbsolute { .. },
                    ..
                })
            )
        {
            pending.pop_back();
        }
        if pending.len() >= CAPTURED_INPUT_CAPACITY {
            if let Some(index) = pending.iter().position(|event| {
                matches!(
                    &event.input,
                    CapturedInput::MouseMove { .. } | CapturedInput::MouseAbsolute { .. }
                )
            }) {
                pending.remove(index);
            } else {
                self.overflowed.store(true, Ordering::Release);
                return;
            }
        }
        pending.push_back(sample);
    }

    pub fn release_gamepad(&self, controller_id: u8, bitmap: u16) {
        assert!(controller_id < 4);
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        pending.retain(|sample| {
            !matches!(sample.input,
            CapturedInput::Gamepad { controller_id: id, .. } if id == controller_id)
        });
        pending.push_back(CapturedInputSample {
            input: CapturedInput::Gamepad {
                controller_id,
                bitmap,
                buttons: 0,
                left_trigger: 0,
                right_trigger: 0,
                left_stick_x: 0,
                left_stick_y: 0,
                right_stick_x: 0,
                right_stick_y: 0,
            },
            captured_at: Instant::now(),
        });
        debug_assert!(pending.len() <= CAPTURED_INPUT_CAPACITY + 4);
    }

    pub fn take(&self) -> Option<CapturedInput> {
        self.take_sample().map(|sample| sample.input)
    }

    pub fn take_sample(&self) -> Option<CapturedInputSample> {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
    }

    pub fn take_overflowed(&self) -> bool {
        self.overflowed.swap(false, Ordering::AcqRel)
    }

    pub fn clear(&self) {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.overflowed.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captured_input_queue_preserves_raw_motion_and_fails_closed_on_control_overflow() {
        let queue = CapturedInputQueue::default();
        for _ in 0..3 {
            queue.push(CapturedInput::MouseMove {
                delta_x: 1,
                delta_y: -1,
            });
        }
        for _ in 0..3 {
            assert_eq!(
                queue.take(),
                Some(CapturedInput::MouseMove {
                    delta_x: 1,
                    delta_y: -1,
                })
            );
        }

        for virtual_key in 0..=u16::try_from(CAPTURED_INPUT_CAPACITY).unwrap() {
            queue.push(CapturedInput::Key {
                virtual_key,
                modifiers: 0,
                pressed: true,
            });
        }
        assert!(queue.take_overflowed());
        assert_eq!(
            queue.take(),
            Some(CapturedInput::Key {
                virtual_key: 0,
                modifiers: 0,
                pressed: true,
            })
        );
    }
}
