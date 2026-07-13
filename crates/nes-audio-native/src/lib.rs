//! Tiny safe wrapper around the vendored miniaudio C backend.
//!
//! All native/unsafe code is deliberately isolated in this crate. Emulator
//! timing, APU synthesis, UI state, and buffering policy remain safe Rust.

use std::{
    ffi::{CStr, c_char, c_float, c_void},
    ptr::NonNull,
};

unsafe extern "C" {
    fn nes_audio_create(
        sample_rate: u32,
        target_frames: u32,
        capacity_frames: u32,
        error: *mut c_char,
        error_capacity: usize,
    ) -> *mut c_void;
    fn nes_audio_destroy(audio: *mut c_void);
    fn nes_audio_push(audio: *mut c_void, samples: *const c_float, frames: u32) -> u32;
    fn nes_audio_clear(audio: *mut c_void);
    fn nes_audio_queued(audio: *const c_void) -> u32;
    fn nes_audio_underflows(audio: *const c_void) -> u32;
    fn nes_audio_overflows(audio: *const c_void) -> u32;
    fn nes_audio_device_rate(audio: *const c_void) -> u32;
    fn nes_audio_device_name(audio: *const c_void) -> *const c_char;
}

pub struct NativeAudio {
    handle: NonNull<c_void>,
    device_name: String,
    device_rate: u32,
}

impl NativeAudio {
    pub fn new(sample_rate: u32, target_frames: u32, capacity_frames: u32) -> Result<Self, String> {
        let mut error = [0_i8; 512];
        let handle = unsafe {
            nes_audio_create(
                sample_rate,
                target_frames,
                capacity_frames,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        let handle = NonNull::new(handle).ok_or_else(|| unsafe {
            CStr::from_ptr(error.as_ptr())
                .to_string_lossy()
                .into_owned()
        })?;
        let device_name = unsafe {
            CStr::from_ptr(nes_audio_device_name(handle.as_ptr()))
                .to_string_lossy()
                .into_owned()
        };
        let device_rate = unsafe { nes_audio_device_rate(handle.as_ptr()) };
        Ok(Self {
            handle,
            device_name,
            device_rate,
        })
    }

    pub fn push(&self, samples: &[f32]) -> usize {
        let frames = samples.len().min(u32::MAX as usize) as u32;
        unsafe { nes_audio_push(self.handle.as_ptr(), samples.as_ptr(), frames) as usize }
    }

    pub fn clear(&self) {
        unsafe { nes_audio_clear(self.handle.as_ptr()) }
    }

    pub fn queued_frames(&self) -> usize {
        unsafe { nes_audio_queued(self.handle.as_ptr()) as usize }
    }

    pub fn underflows(&self) -> u32 {
        unsafe { nes_audio_underflows(self.handle.as_ptr()) }
    }

    pub fn overflows(&self) -> u32 {
        unsafe { nes_audio_overflows(self.handle.as_ptr()) }
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    pub fn device_rate(&self) -> u32 {
        self.device_rate
    }
}

impl Drop for NativeAudio {
    fn drop(&mut self) {
        unsafe { nes_audio_destroy(self.handle.as_ptr()) }
    }
}
