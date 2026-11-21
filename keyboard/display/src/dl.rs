//! Function pointers resolved with `dlsym`.

use std::ffi::c_void;

/// Reinterprets a non-null `dlsym` address as the function pointer type `F`. The caller's
/// declared type (typically a struct field holding the symbol) is the symbol's signature; a type
/// that isn't pointer-sized fails to compile.
///
/// # Safety
///
/// `p` is the address of a function whose signature is `F`.
pub(crate) unsafe fn fn_ptr<F: Copy>(p: *mut c_void) -> F {
    const { assert!(std::mem::size_of::<F>() == std::mem::size_of::<*mut c_void>()) };
    // SAFETY: `F` is pointer-sized (asserted above) and the caller guarantees it's `p`'s signature.
    unsafe { std::mem::transmute_copy::<*mut c_void, F>(&p) }
}
