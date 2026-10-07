//! Introspection hooks from `dispatch/introspection.h`.
//!
//! These symbols are only present in the **introspection** build of libdispatch
//! (e.g. when using `DYLD_LIBRARY_PATH=/usr/lib/system/introspection` on macOS).
//! Tools may interpose or replace them to observe queue lifecycle and callouts.
//!
//! See [Apple's documentation](https://developer.apple.com/documentation/dispatch/dispatch_introspection).

use core::ffi::c_void;
use core::ptr::NonNull;

use crate::object::dispatch_object_s;
use crate::{dispatch_function_t, DispatchQueue};

extern "C" {
    /// Called when a dispatch queue is created.
    pub fn dispatch_introspection_hook_queue_create(queue: &DispatchQueue);

    /// Called when a dispatch queue is about to be destroyed.
    pub fn dispatch_introspection_hook_queue_destroy(queue: &DispatchQueue);

    /// Called when an item is about to be enqueued onto a queue.
    pub fn dispatch_introspection_hook_queue_item_enqueue(
        queue: &DispatchQueue,
        item: NonNull<dispatch_object_s>,
    );

    /// Called when an item was dequeued from a queue.
    pub fn dispatch_introspection_hook_queue_item_dequeue(
        queue: &DispatchQueue,
        item: NonNull<dispatch_object_s>,
    );

    /// Called when a dequeued item has completed processing.
    ///
    /// The `item` pointer is an opaque identifier and must not be dereferenced.
    pub fn dispatch_introspection_hook_queue_item_complete(item: NonNull<dispatch_object_s>);

    /// Called immediately before a client function runs on a queue.
    pub fn dispatch_introspection_hook_queue_callout_begin(
        queue: &DispatchQueue,
        context: *mut c_void,
        function: dispatch_function_t,
    );

    /// Called immediately after a client function returns on a queue.
    pub fn dispatch_introspection_hook_queue_callout_end(
        queue: &DispatchQueue,
        context: *mut c_void,
        function: dispatch_function_t,
    );
}
