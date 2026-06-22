use core::ffi::{c_uint, c_void};
use core::ptr::NonNull;

use alloc::boxed::Box;

use crate::dispatch_function_t;
use crate::generated::dispatch_get_context;
use crate::{
    generated::{
        dispatch_activate, dispatch_resume, dispatch_set_context, dispatch_set_finalizer_f,
        dispatch_set_qos_class_floor, dispatch_set_target_queue, dispatch_suspend,
    },
    DispatchRetained,
};

use super::{utils::function_wrapper, DispatchQueue};

enum_with_val! {
    /// Quality-of-service classes that specify the priorities for executing tasks.
    #[doc(alias = "dispatch_qos_class_t")]
    #[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
    pub struct DispatchQoS(pub c_uint) {
        /// Quality of service for user-interactive tasks.
        #[doc(alias = "QOS_CLASS_USER_INTERACTIVE")]
        UserInteractive = 0x21,
        /// Quality of service for tasks that prevent the user from actively using your app.
        #[doc(alias = "QOS_CLASS_USER_INITIATED")]
        UserInitiated = 0x19,
        /// Default Quality of service.
        #[doc(alias = "QOS_CLASS_DEFAULT")]
        Default = 0x15,
        /// Quality of service for tasks that the user does not track actively.
        #[doc(alias = "QOS_CLASS_UTILITY")]
        Utility = 0x11,
        /// Quality of service for maintenance or cleanup tasks.
        #[doc(alias = "QOS_CLASS_BACKGROUND")]
        Background = 0x09,
        /// The absence of a Quality of service.
        #[doc(alias = "QOS_CLASS_UNSPECIFIED")]
        Unspecified = 0x00,
    }
}

/// Minimum relative priority for [`DispatchObject::set_qos_class_floor`].
pub const QOS_MIN_RELATIVE_PRIORITY: i32 = -15;

/// Error returned by [`DispatchObject::set_qos_class_floor`].
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum QualityOfServiceClassFloorError {
    /// The relative priority is invalid.
    InvalidRelativePriority,
}

/// Types that represent dispatch objects.
///
/// # Safety
///
/// The object must represent a dispatch object, and be usable in
/// `dispatch_retain` / `dispatch_release`.
#[doc(alias = "dispatch_object_t")]
pub unsafe trait DispatchObject {
    /// Increment the reference count of the object.
    ///
    /// This extends the duration in which the object is alive by detaching it
    /// from the lifetime information carried by the reference.
    #[doc(alias = "dispatch_retain")]
    #[inline]
    fn retain(&self) -> DispatchRetained<Self> {
        let ptr: NonNull<Self> = NonNull::from(self);
        // SAFETY:
        // - The pointer is valid since it came from `&self`.
        // - The lifetime of the pointer itself is extended, but any lifetime
        //   that the object may carry is still kept within the type itself.
        unsafe { DispatchRetained::retain(ptr) }
    }

    /// Returns the context pointer associated with this object.
    ///
    /// Prefer [`Self::set_finalizer`] for attaching Rust-owned data that should
    /// be released when the object is destroyed.
    #[doc(alias = "dispatch_get_context")]
    #[inline]
    fn context(&self) -> *mut c_void {
        dispatch_get_context(self.as_raw())
    }

    /// Set the context pointer associated with this object.
    ///
    /// # Safety
    ///
    /// The caller must ensure `context` remains valid for as long as the object
    /// may read it, and that any necessary cleanup is performed (for example via
    /// [`Self::set_finalizer`]).
    #[doc(alias = "dispatch_set_context")]
    #[inline]
    unsafe fn set_context(&self, context: *mut c_void) {
        // SAFETY: Upheld by the caller.
        unsafe { dispatch_set_context(self.as_raw(), context) }
    }

    /// Set the finalizer function for the object.
    ///
    /// # Safety
    ///
    /// The finalizer must be a valid C function pointer.
    #[doc(alias = "dispatch_set_finalizer_f")]
    #[inline]
    unsafe fn set_finalizer_f(&self, finalizer: dispatch_function_t) {
        // SAFETY: Upheld by the caller.
        unsafe { dispatch_set_finalizer_f(self.as_raw(), finalizer) }
    }

    /// Set the finalizer function for the object.
    #[inline]
    fn set_finalizer<F>(&self, destructor: F)
    where
        F: Send + FnOnce(),
    {
        let destructor_boxed = Box::into_raw(Box::new(destructor)).cast();

        // Safety: As this use the dispatch object's context, and because we need some way to wrap the Rust function, we set the context.
        //         Once the finalizer is executed, the context will be dangling.
        //         This isn't an issue as the context shall not be accessed after the dispatch object is destroyed.
        unsafe {
            self.set_context(destructor_boxed);
            self.set_finalizer_f(function_wrapper::<F>);
        }
    }

    /// Set the target [`DispatchQueue`] of this object.
    ///
    /// # Aborts
    ///
    /// Aborts if the object has been activated.
    ///
    /// # Safety
    ///
    /// - There must not be a cycle in the hierarchy of queues.
    #[doc(alias = "dispatch_set_target_queue")]
    #[inline]
    unsafe fn set_target_queue(&self, queue: &DispatchQueue) {
        // SAFETY: `object` and `queue` cannot be null, rest is upheld by caller.
        unsafe { dispatch_set_target_queue(self.as_raw(), Some(queue)) };
    }

    /// Set the QOS class floor on a dispatch queue, source or workloop.
    ///
    /// # Safety
    ///
    /// - `DispatchObject` should be a queue or queue source.
    #[inline]
    unsafe fn set_qos_class_floor(
        &self,
        qos_class: DispatchQoS,
        relative_priority: i32,
    ) -> Result<(), QualityOfServiceClassFloorError> {
        if !(QOS_MIN_RELATIVE_PRIORITY..=0).contains(&relative_priority) {
            return Err(QualityOfServiceClassFloorError::InvalidRelativePriority);
        }

        // SAFETY: Safe as relative_priority can only be valid.
        unsafe { dispatch_set_qos_class_floor(self.as_raw(), qos_class, relative_priority) };

        Ok(())
    }

    /// Activate the object.
    #[inline]
    fn activate(&self) {
        dispatch_activate(self.as_raw());
    }

    /// Suspend the invocation of functions on the object.
    #[inline]
    fn suspend(&self) {
        dispatch_suspend(self.as_raw());
    }

    /// Resume the invocation of functions on the object.
    #[inline]
    fn resume(&self) {
        dispatch_resume(self.as_raw());
    }

    #[inline]
    #[doc(hidden)]
    fn as_raw(&self) -> NonNull<dispatch_object_s> {
        NonNull::from(self).cast()
    }
}

/// Holds an extra reference on a dispatch object while it is suspended.
///
/// Resumes the object when dropped. This prevents releasing the last reference
/// while the object is still suspended.
#[derive(Debug)]
pub struct DispatchSuspendGuard<T: DispatchObject> {
    object: DispatchRetained<T>,
}

impl<T: DispatchObject> DispatchSuspendGuard<T> {
    /// Suspend the object and retain it until this guard is dropped.
    #[inline]
    #[must_use]
    pub fn new(object: &DispatchRetained<T>) -> Self {
        object.suspend();
        Self {
            object: object.retain(),
        }
    }
}

impl<T: DispatchObject> Drop for DispatchSuspendGuard<T> {
    #[inline]
    fn drop(&mut self) {
        self.object.resume();
    }
}

/// Holds an extra reference on an inactive dispatch object until activation.
///
/// Releasing the last reference on an inactive object is undefined; keep this
/// guard until [`Self::activate`] is called.
#[derive(Debug)]
pub struct DispatchActivationGuard<T: DispatchObject> {
    object: DispatchRetained<T>,
}

impl<T: DispatchObject> DispatchActivationGuard<T> {
    /// Retain an inactive object until it is activated.
    #[inline]
    #[must_use]
    pub const fn new(object: DispatchRetained<T>) -> Self {
        Self { object }
    }

    /// Activate the object and release the guard's extra retain.
    #[inline]
    pub fn activate(self) {
        self.object.activate();
        core::mem::forget(self);
    }
}

impl<T: DispatchObject> Drop for DispatchActivationGuard<T> {
    #[inline]
    fn drop(&mut self) {
        // Avoid undefined behavior if the guard is dropped without activation.
        self.object.activate();
    }
}

// Helper to allow generated/mod.rs to emit the functions it needs to.
//
// This is private because we do not want users to hold a `dispatch_object_t`
// directly, as there's no way to safely down- nor upcast to the other types.
// Instead, they should use `DispatchObject`.
mod private {
    #[allow(non_camel_case_types)]
    #[repr(C)]
    #[derive(Debug)]
    pub struct dispatch_object_s {
        /// opaque value
        _inner: [u8; 0],
        _p: crate::OpaqueData,
    }

    #[cfg(feature = "objc2")]
    // SAFETY: Dispatch types are internally objects.
    unsafe impl objc2::encode::RefEncode for dispatch_object_s {
        const ENCODING_REF: objc2::encode::Encoding = objc2::encode::Encoding::Object;
    }
}

pub(crate) use private::dispatch_object_s;
