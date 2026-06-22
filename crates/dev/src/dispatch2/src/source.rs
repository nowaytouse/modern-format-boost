#![allow(non_camel_case_types)]
use core::ffi::{c_ulong, c_void};

use alloc::boxed::Box;

use crate::generated::{
    _dispatch_source_type_data_add, _dispatch_source_type_data_or,
    _dispatch_source_type_data_replace, _dispatch_source_type_mach_recv,
    _dispatch_source_type_mach_send, _dispatch_source_type_memorypressure,
    _dispatch_source_type_proc, _dispatch_source_type_read, _dispatch_source_type_signal,
    _dispatch_source_type_timer, _dispatch_source_type_vnode, _dispatch_source_type_write,
    dispatch_set_target_queue,
};
use crate::numeric_cast;
use crate::{DispatchObject, DispatchQueue, DispatchRetained, DispatchTime};

dispatch_object!(
    /// Dispatch source.
    #[doc(alias = "dispatch_source_t")]
    #[doc(alias = "dispatch_source_s")]
    pub struct DispatchSource;
);

dispatch_object_not_data!(unsafe DispatchSource);

#[repr(C)]
#[derive(Debug)]
/// Opaque dispatch source type descriptor (see `dispatch_source_type_t`).
pub struct dispatch_source_type_s {
    /// Reserved; do not access.
    _inner: [u8; 0],
    /// Opaque marker field.
    _p: crate::OpaqueData,
}

#[cfg(feature = "objc2")]
// SAFETY: Dispatch types are internally objects.
unsafe impl objc2::encode::RefEncode for dispatch_source_type_s {
    const ENCODING_REF: objc2::encode::Encoding = objc2::encode::Encoding::Object;
}

/// Opaque dispatch source type token (see `dispatch_source_type_t`).
pub type dispatch_source_type_t = *mut dispatch_source_type_s;

enum_with_val! {
    /// Mach send-right flags.
    #[derive(PartialEq, Eq, Clone, Copy)]
    pub struct dispatch_source_mach_send_flags_t(pub c_ulong) {
        /// The send right is dead.
        DISPATCH_MACH_SEND_DEAD = 0x1
    }
}

enum_with_val! {
    /// Mach receive-right flags (reserved; no public constants defined).
    #[derive(PartialEq, Eq, Clone, Copy)]
    pub struct dispatch_source_mach_recv_flags_t(pub c_ulong) {
        // no definition
    }
}

enum_with_val! {
    /// Memory pressure events.
    #[derive(PartialEq, Eq, Clone, Copy)]
    pub struct dispatch_source_memorypressure_flags_t(pub c_ulong) {
        /// Normal memory pressure.
        DISPATCH_MEMORYPRESSURE_NORMAL = 0x1,
        /// Warning memory pressure.
        DISPATCH_MEMORYPRESSURE_WARN = 0x2,
        /// Critical memory pressure.
        DISPATCH_MEMORYPRESSURE_CRITICAL = 0x4,
    }
}

enum_with_val! {
    /// Events related to a process.
    #[derive(PartialEq, Eq, Clone, Copy)]
    pub struct dispatch_source_proc_flags_t(pub c_ulong) {
        /// The process exited.
        DISPATCH_PROC_EXIT = 0x80000000,
        /// The process forked.
        DISPATCH_PROC_FORK = 0x40000000,
        /// The process exec'd.
        DISPATCH_PROC_EXEC = 0x20000000,
        /// A signal was delivered to the process.
        DISPATCH_PROC_SIGNAL = 0x08000000,
    }
}

enum_with_val! {
    /// Events involving a change to a file system object.
    #[derive(PartialEq, Eq, Clone, Copy)]
    pub struct dispatch_source_vnode_flags_t(pub c_ulong) {
        /// The file was deleted.
        DISPATCH_VNODE_DELETE = 0x1,
        /// The file was written.
        DISPATCH_VNODE_WRITE = 0x2,
        /// The file was extended.
        DISPATCH_VNODE_EXTEND = 0x4,
        /// File attributes changed.
        DISPATCH_VNODE_ATTRIB = 0x8,
        /// A hard link was created.
        DISPATCH_VNODE_LINK = 0x10,
        /// The file was renamed.
        DISPATCH_VNODE_RENAME = 0x20,
        /// The file was revoked.
        DISPATCH_VNODE_REVOKE = 0x40,
        /// The file was unlocked by funlock.
        DISPATCH_VNODE_FUNLOCK = 0x100,
    }
}

enum_with_val! {
    /// Flags to use when configuring a timer dispatch source.
    #[derive(PartialEq, Eq, Clone, Copy)]
    pub struct dispatch_source_timer_flags_t(pub c_ulong) {
        /// Use strict timer deadlines.
        DISPATCH_TIMER_STRICT = 0x1,
    }
}

#[inline]
fn source_type(source_type: &dispatch_source_type_s) -> dispatch_source_type_t {
    (source_type as *const dispatch_source_type_s).cast_mut()
}

impl DispatchSource {
    /// Create a timer dispatch source.
    #[inline]
    #[must_use]
    pub fn timer(
        queue: Option<&DispatchQueue>,
        flags: dispatch_source_timer_flags_t,
    ) -> DispatchRetained<Self> {
        // SAFETY: `_dispatch_source_type_timer` is a valid source type constant.
        unsafe {
            Self::new(
                source_type(&_dispatch_source_type_timer),
                0,
                flags.0 as usize,
                queue,
            )
        }
    }

    /// Create a signal dispatch source.
    #[inline]
    #[must_use]
    pub fn signal(queue: Option<&DispatchQueue>, signal: i32) -> DispatchRetained<Self> {
        // SAFETY: `_dispatch_source_type_signal` is a valid source type constant.
        unsafe {
            Self::new(
                source_type(&_dispatch_source_type_signal),
                numeric_cast::i32_to_usize(signal),
                0,
                queue,
            )
        }
    }

    /// Create a read dispatch source for a file descriptor.
    #[inline]
    #[must_use]
    pub fn read(queue: Option<&DispatchQueue>, fd: i32) -> DispatchRetained<Self> {
        // SAFETY: `_dispatch_source_type_read` is a valid source type constant.
        unsafe {
            Self::new(
                source_type(&_dispatch_source_type_read),
                numeric_cast::i32_to_usize(fd),
                0,
                queue,
            )
        }
    }

    /// Create a write dispatch source for a file descriptor.
    #[inline]
    #[must_use]
    pub fn write(queue: Option<&DispatchQueue>, fd: i32) -> DispatchRetained<Self> {
        // SAFETY: `_dispatch_source_type_write` is a valid source type constant.
        unsafe {
            Self::new(
                source_type(&_dispatch_source_type_write),
                numeric_cast::i32_to_usize(fd),
                0,
                queue,
            )
        }
    }

    /// Create a vnode dispatch source.
    #[inline]
    #[must_use]
    pub fn vnode(
        queue: Option<&DispatchQueue>,
        fd: i32,
        mask: dispatch_source_vnode_flags_t,
    ) -> DispatchRetained<Self> {
        // SAFETY: `_dispatch_source_type_vnode` is a valid source type constant.
        unsafe {
            Self::new(
                source_type(&_dispatch_source_type_vnode),
                numeric_cast::i32_to_usize(fd),
                mask.0 as usize,
                queue,
            )
        }
    }

    /// Create a process dispatch source.
    #[inline]
    #[must_use]
    pub fn proc(
        queue: Option<&DispatchQueue>,
        pid: i32,
        mask: dispatch_source_proc_flags_t,
    ) -> DispatchRetained<Self> {
        // SAFETY: `_dispatch_source_type_proc` is a valid source type constant.
        unsafe {
            Self::new(
                source_type(&_dispatch_source_type_proc),
                numeric_cast::i32_to_usize(pid),
                mask.0 as usize,
                queue,
            )
        }
    }

    /// Create a memory pressure dispatch source.
    #[inline]
    #[must_use]
    pub fn memorypressure(
        queue: Option<&DispatchQueue>,
        mask: dispatch_source_memorypressure_flags_t,
    ) -> DispatchRetained<Self> {
        // SAFETY: `_dispatch_source_type_memorypressure` is a valid source type constant.
        unsafe {
            Self::new(
                source_type(&_dispatch_source_type_memorypressure),
                0,
                mask.0 as usize,
                queue,
            )
        }
    }

    /// Create a mach send dispatch source.
    #[inline]
    #[must_use]
    pub fn mach_send(
        queue: Option<&DispatchQueue>,
        port: u32,
        mask: dispatch_source_mach_send_flags_t,
    ) -> DispatchRetained<Self> {
        // SAFETY: `_dispatch_source_type_mach_send` is a valid source type constant.
        unsafe {
            Self::new(
                source_type(&_dispatch_source_type_mach_send),
                numeric_cast::u32_to_usize(port),
                mask.0 as usize,
                queue,
            )
        }
    }

    /// Create a mach receive dispatch source.
    #[inline]
    #[must_use]
    pub fn mach_recv(queue: Option<&DispatchQueue>, port: u32) -> DispatchRetained<Self> {
        // SAFETY: `_dispatch_source_type_mach_recv` is a valid source type constant.
        unsafe {
            Self::new(
                source_type(&_dispatch_source_type_mach_recv),
                numeric_cast::u32_to_usize(port),
                0,
                queue,
            )
        }
    }

    /// Create a data-add dispatch source.
    #[inline]
    #[must_use]
    pub fn data_add(queue: Option<&DispatchQueue>) -> DispatchRetained<Self> {
        // SAFETY: `_dispatch_source_type_data_add` is a valid source type constant.
        unsafe { Self::new(source_type(&_dispatch_source_type_data_add), 0, 0, queue) }
    }

    /// Create a data-or dispatch source.
    #[inline]
    #[must_use]
    pub fn data_or(queue: Option<&DispatchQueue>) -> DispatchRetained<Self> {
        // SAFETY: `_dispatch_source_type_data_or` is a valid source type constant.
        unsafe { Self::new(source_type(&_dispatch_source_type_data_or), 0, 0, queue) }
    }

    /// Create a data-replace dispatch source.
    #[inline]
    #[must_use]
    pub fn data_replace(queue: Option<&DispatchQueue>) -> DispatchRetained<Self> {
        // SAFETY: `_dispatch_source_type_data_replace` is a valid source type constant.
        unsafe {
            Self::new(
                source_type(&_dispatch_source_type_data_replace),
                0,
                0,
                queue,
            )
        }
    }

    /// Set the target queue before activation.
    ///
    /// # Safety
    ///
    /// There must not be a cycle in the hierarchy of queues.
    #[inline]
    pub unsafe fn set_target_queue(&self, queue: Option<&DispatchQueue>) {
        // SAFETY: Upheld by the caller.
        unsafe { dispatch_set_target_queue(self.as_raw(), queue) };
    }

    /// Configure a timer source.
    #[inline]
    pub fn configure_timer(&self, start: DispatchTime, interval: u64, leeway: u64) {
        self.set_timer(start, interval, leeway);
    }

    /// Set the event handler using a Rust closure.
    #[cfg(feature = "block2")]
    #[inline]
    pub fn set_event_handler<F>(&self, handler: F)
    where
        F: Send + Fn() + 'static,
    {
        let block = block2::RcBlock::new(handler);
        let block_ptr = block2::RcBlock::as_ptr(&block);
        // SAFETY: libdispatch copies and retains the block.
        unsafe { self.set_event_handler_with_block(block_ptr) };
    }

    /// Set the event handler using a Rust closure.
    #[cfg(not(feature = "block2"))]
    #[inline]
    pub fn set_event_handler<F>(&self, handler: F)
    where
        F: Send + FnMut() + 'static,
    {
        let handler_boxed = Box::into_raw(Box::new(handler)).cast();
        // SAFETY: Context is consumed only by the event handler adapter.
        unsafe { self.set_context(handler_boxed) };
        self.set_event_handler_f(event_handler_adapter::<F>);
    }

    /// Set the cancellation handler using a Rust closure.
    #[inline]
    pub fn set_cancel_handler<F>(&self, handler: F)
    where
        F: Send + FnOnce() + 'static,
    {
        let handler_boxed = Box::into_raw(Box::new(handler)).cast();
        // SAFETY: Context is consumed only by the cancel handler adapter.
        unsafe { self.set_context(handler_boxed) };
        self.set_cancel_handler_f(cancel_handler_adapter::<F>);
    }
}

#[cfg(not(feature = "block2"))]
extern "C" fn event_handler_adapter<F: FnMut() + Send>(ctx: *mut c_void) {
    // SAFETY: Context was set to a valid `Box<F>` in `set_event_handler`.
    let handler = unsafe { &mut *ctx.cast::<F>() };
    handler();
}

extern "C" fn cancel_handler_adapter<F: FnOnce() + Send>(ctx: *mut c_void) {
    // SAFETY: Context was set to a valid `Box<F>` in `set_cancel_handler`.
    let handler = unsafe { Box::from_raw(ctx.cast::<F>()) };
    handler();
}
