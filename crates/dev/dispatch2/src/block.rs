//! Safe wrappers for `dispatch_block_*` APIs.
#![cfg(feature = "block2")]

use core::cell::RefCell;

use crate::generated::{
    dispatch_block_cancel, dispatch_block_create, dispatch_block_create_with_qos_class,
    dispatch_block_flags_t, dispatch_block_notify, dispatch_block_perform, dispatch_block_t,
    dispatch_block_testcancel, dispatch_block_wait,
};
use crate::{
    DispatchQoS, DispatchTime, QualityOfServiceClassFloorError, QOS_MIN_RELATIVE_PRIORITY,
};
use crate::{DispatchQueue, WaitError};

/// A heap-allocated dispatch block object.
///
/// Wraps the raw `dispatch_block_t` returned by `dispatch_block_create*`.
#[derive(Debug)]
pub struct DispatchBlock {
    inner: dispatch_block_t,
}

impl DispatchBlock {
    /// Create a new dispatch block with the given flags.
    #[inline]
    #[must_use]
    pub fn create<F>(flags: dispatch_block_flags_t, work: F) -> Option<Self>
    where
        F: Send + Fn() + 'static,
    {
        let block = block2::RcBlock::new(work);
        let block_ptr = block2::RcBlock::as_ptr(&block);
        // SAFETY: libdispatch copies and retains the block.
        let inner = unsafe { dispatch_block_create(flags, block_ptr) };
        if inner.is_null() {
            None
        } else {
            Some(Self { inner })
        }
    }

    /// Create a new dispatch block with an assigned QOS class.
    #[inline]
    pub fn create_with_qos<F>(
        flags: dispatch_block_flags_t,
        qos_class: DispatchQoS,
        relative_priority: i32,
        work: F,
    ) -> Result<Option<Self>, QualityOfServiceClassFloorError>
    where
        F: Send + Fn() + 'static,
    {
        if !(QOS_MIN_RELATIVE_PRIORITY..=0).contains(&relative_priority) {
            return Err(QualityOfServiceClassFloorError::InvalidRelativePriority);
        }

        let block = block2::RcBlock::new(work);
        let block_ptr = block2::RcBlock::as_ptr(&block);
        // SAFETY: libdispatch copies and retains the block.
        let inner = unsafe {
            dispatch_block_create_with_qos_class(flags, qos_class, relative_priority, block_ptr)
        };
        Ok(if inner.is_null() {
            None
        } else {
            Some(Self { inner })
        })
    }

    /// Returns the underlying block handle.
    #[inline]
    #[must_use]
    pub fn as_block(&self) -> dispatch_block_t {
        self.inner
    }

    /// Execute the block synchronously with the given flags.
    #[inline]
    pub fn perform(&self, flags: dispatch_block_flags_t) {
        // SAFETY: `inner` is a valid dispatch block created by this wrapper.
        unsafe { dispatch_block_perform(flags, self.inner) };
    }

    /// Wait for the block to finish executing.
    #[inline]
    pub fn wait(&self, timeout: DispatchTime) -> Result<(), WaitError> {
        // SAFETY: `inner` is a valid dispatch block created by this wrapper.
        match unsafe { dispatch_block_wait(self.inner, timeout) } {
            0 => Ok(()),
            _ => Err(WaitError::Timeout),
        }
    }

    /// Cancel the block.
    #[inline]
    pub fn cancel(&self) {
        // SAFETY: `inner` is a valid dispatch block created by this wrapper.
        unsafe { dispatch_block_cancel(self.inner) };
    }

    /// Returns whether the block has been canceled.
    #[inline]
    #[must_use]
    pub fn is_canceled(&self) -> bool {
        // SAFETY: `inner` is a valid dispatch block created by this wrapper.
        unsafe { dispatch_block_testcancel(self.inner) != 0 }
    }

    /// Submit a notification handler when the block completes.
    #[inline]
    pub fn notify<F>(&self, queue: &DispatchQueue, handler: F)
    where
        F: Send + FnOnce() + 'static,
    {
        let handler = RefCell::new(Some(handler));
        let block = block2::RcBlock::new(move || {
            if let Some(handler) = handler.take() {
                handler();
            }
        });
        let block_ptr = block2::RcBlock::as_ptr(&block);
        // SAFETY: libdispatch copies and retains the notification block.
        unsafe { dispatch_block_notify(self.inner, queue, block_ptr) };
    }
}
