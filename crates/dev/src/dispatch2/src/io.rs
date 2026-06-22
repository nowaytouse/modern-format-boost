//! Dispatch I/O flag types (constants mirror `dispatch/io.h`).
use core::ffi::{c_long, c_ulong};

dispatch_object!(
    /// Dispatch I/O channel bound to a file descriptor or pipe.
    #[doc(alias = "dispatch_io_t")]
    #[doc(alias = "dispatch_io_s")]
    pub struct DispatchIO;
);

dispatch_object_not_data!(unsafe DispatchIO);

enum_with_val! {
    /// Stream access pattern for [`DispatchIO`].
    #[doc(alias = "dispatch_io_type_t")]
    #[derive(PartialEq, Eq, Clone, Copy)]
    pub struct DispatchIOStreamType(pub c_ulong) {
        /// Sequential I/O.
        DISPATCH_IO_STREAM = 0,
        /// Random-access I/O.
        DISPATCH_IO_RANDOM = 1,
    }
}

enum_with_val! {
    /// Flags passed when closing a [`DispatchIO`] channel.
    #[doc(alias = "dispatch_io_close_flags_t")]
    #[derive(PartialEq, Eq, Clone, Copy)]
    pub struct DispatchIOCloseFlags(pub c_ulong) {
        /// Stop I/O after pending operations complete.
        DISPATCH_IO_STOP = 0x1,
    }
}

enum_with_val! {
    /// Flags for interval-based I/O delivery.
    #[doc(alias = "dispatch_io_interval_flags_t")]
    #[derive(PartialEq, Eq, Clone, Copy)]
    pub struct DispatchIOIntervalFlags(pub c_long) {
        /// Enforce the configured interval strictly.
        DISPATCH_IO_STRICT_INTERVAL = 0x1,
    }
}
