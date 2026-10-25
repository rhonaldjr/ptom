//! The **only** crate in Farsight allowed to use `unsafe` (ADR-0021, user
//! decisions U-13 and U-15). It wraps what safe Rust can't reach:
//! synchronising CPU reads of a DMA-BUF (`DMA_BUF_IOCTL_SYNC`), needed when
//! the compositor shares the screen as a GPU buffer, and H.264 encoding on
//! the GPU: VA-API for AMD and Intel (feature `va`), NVENC for NVIDIA
//! (feature `nvenc`). Every unsafe block has a `// SAFETY:` comment,
//! and anything that can be written safely lives in the calling crates.

#[cfg(feature = "nvenc")]
pub mod nvenc;
#[cfg(feature = "va")]
pub mod va;

use std::io;
use std::os::fd::{BorrowedFd, RawFd};

use rustix::ioctl::{Setter, opcode};

/// `DMA_BUF_SYNC_READ`: the CPU is about to read (or has read) the buffer.
const SYNC_READ: u64 = 1 << 0;
/// `DMA_BUF_SYNC_END`: the CPU access is over (otherwise it begins).
const SYNC_END: u64 = 1 << 2;
/// `DMA_BUF_IOCTL_SYNC` = `_IOW('b', 0, struct dma_buf_sync { __u64 flags; })`.
const DMA_BUF_IOCTL_SYNC: rustix::ioctl::Opcode = opcode::write::<u64>(b'b', 0);

/// Which side of a CPU read the sync is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Before reading: make the GPU's writes visible to the CPU.
    BeginRead,
    /// After reading: hand the buffer back.
    EndRead,
}

/// Brackets a CPU read of the DMA-BUF `fd` (`DMA_BUF_IOCTL_SYNC`). Call it
/// with [`Access::BeginRead`] before reading the mapping and
/// [`Access::EndRead`] after.
///
/// `fd` must be an open DMA-BUF file descriptor that stays open for the
/// duration of the call (PipeWire guarantees that for the buffer being
/// processed). Negative descriptors are refused.
///
/// # Errors
/// The kernel's error, e.g. when `fd` isn't a DMA-BUF.
pub fn sync(fd: RawFd, access: Access) -> io::Result<()> {
    if fd < 0 {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    #[allow(unsafe_code)]
    // SAFETY: `fd` is non-negative (checked above) and, per this function's
    // contract, an open descriptor that outlives the borrow, which ends when
    // this function returns.
    let fd = unsafe { BorrowedFd::borrow_raw(fd) };
    let flags = SYNC_READ
        | match access {
            Access::BeginRead => 0,
            Access::EndRead => SYNC_END,
        };
    #[allow(unsafe_code)]
    // SAFETY: DMA_BUF_IOCTL_SYNC takes a pointer to one `u64` of flags, which
    // is exactly the argument type this `Setter` passes.
    let request = unsafe { Setter::<DMA_BUF_IOCTL_SYNC, u64>::new(flags) };
    #[allow(unsafe_code)]
    // SAFETY: `fd` is valid for the call (see above) and `request` matches
    // the ioctl's definition; the kernel only reads the flags.
    unsafe { rustix::ioctl::ioctl(fd, request) }?;
    Ok(())
}

/// Reads `len` bytes of the DMA-BUF `fd`, starting at `offset`, with the
/// CPU: brackets the access with [`sync`], copies the bytes out of a
/// read-only mapping, and hands the copy to `read`. Only buffers with a
/// linear layout read as plain rows of pixels; the caller negotiates that.
///
/// `fd` must be an open DMA-BUF that stays open for the call (PipeWire
/// guarantees that for the buffer being processed).
///
/// # Errors
/// When `fd` isn't a DMA-BUF or can't be mapped.
pub fn read_dmabuf<R>(
    fd: RawFd,
    offset: u64,
    len: usize,
    read: impl FnOnce(&[u8]) -> R,
) -> io::Result<R> {
    if fd < 0 || len == 0 {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    // The sync comes FIRST: it fails for anything that isn't a DMA-BUF, so a
    // shrinkable file (e.g. a memfd) is never mapped, and the kernel's
    // dma-buf mmap then refuses any range past the buffer's real size.
    sync(fd, Access::BeginRead)?;
    let copied = copy_mapping(fd, offset, len);
    // End the access even when mapping failed, then report the first error.
    let ended = sync(fd, Access::EndRead);
    let bytes = copied?;
    ended?;
    Ok(read(&bytes))
}

/// Copies `len` bytes from a read-only mapping of `fd`. A copy, because the
/// producer (compositor or GPU) may still write the buffer while we read:
/// raw copying is fine with that, a `&[u8]` over the mapping would not be.
fn copy_mapping(fd: RawFd, offset: u64, len: usize) -> io::Result<Vec<u8>> {
    use rustix::mm::{MapFlags, ProtFlags};
    #[allow(unsafe_code)]
    // SAFETY: `fd` is non-negative (checked by the caller) and open for this
    // call (its contract); the borrow ends when this function returns.
    let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
    #[allow(unsafe_code)]
    // SAFETY: a new, read-only, shared mapping at an address the kernel
    // picks; nothing else in this process refers to it.
    let mapping = unsafe {
        rustix::mm::mmap(
            std::ptr::null_mut(),
            len,
            ProtFlags::READ,
            MapFlags::SHARED,
            borrowed,
            offset,
        )
    }?;
    let mut bytes = Vec::<u8>::with_capacity(len);
    #[allow(unsafe_code)]
    // SAFETY: the mapping holds `len` readable bytes; `bytes` has capacity
    // for `len` and doesn't overlap it. A raw copy tolerates concurrent
    // writes by the producer (no reference into the mapping is formed).
    unsafe {
        std::ptr::copy_nonoverlapping(mapping.cast::<u8>().cast_const(), bytes.as_mut_ptr(), len);
    }
    #[allow(unsafe_code)]
    // SAFETY: the `len` bytes were all written by the copy above.
    unsafe {
        bytes.set_len(len);
    }
    #[allow(unsafe_code)]
    // SAFETY: exactly the mapping made above, no longer used.
    unsafe { rustix::mm::munmap(mapping, len) }?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_a_non_dmabuf_is_an_error_not_a_crash() {
        let file = tempfile_with(b"not a dma-buf");
        let fd = std::os::fd::AsRawFd::as_raw_fd(&file);
        assert!(read_dmabuf(fd, 0, 16, <[u8]>::len).is_err());
        assert!(read_dmabuf(-1, 0, 16, <[u8]>::len).is_err());
        assert!(read_dmabuf(fd, 0, 0, <[u8]>::len).is_err());
    }

    #[test]
    fn mapping_reads_the_bytes_at_the_offset() {
        // Any mappable file exercises the mapping half (the sync is for DMA-BUFs).
        let file = tempfile_with(b"0123456789");
        let fd = std::os::fd::AsRawFd::as_raw_fd(&file);
        assert_eq!(copy_mapping(fd, 0, 10).unwrap(), b"0123456789");
    }

    fn tempfile_with(bytes: &[u8]) -> std::fs::File {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("farsight-gpu-sys-{}-{n}", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        let file = std::fs::File::open(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        file
    }

    #[test]
    fn non_dmabuf_descriptors_are_refused_without_harm() {
        assert!(sync(-1, Access::BeginRead).is_err());
        // A regular file isn't a DMA-BUF: the kernel says so.
        let file = tempfile_with(b"not a dma-buf");
        let fd = std::os::fd::AsRawFd::as_raw_fd(&file);
        assert!(sync(fd, Access::BeginRead).is_err());
    }
}
