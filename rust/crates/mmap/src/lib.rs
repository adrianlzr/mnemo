/// PMD-backed THP size: 32 MiB at a 16 KiB granule, 2 MiB at 4 KiB.
#[cfg(target_os = "linux")]
fn huge_page_size() -> usize {
    use std::sync::OnceLock;
    static SIZE: OnceLock<usize> = OnceLock::new();
    *SIZE.get_or_init(|| {
        std::fs::read_to_string("/sys/kernel/mm/transparent_hugepage/hpage_pmd_size")
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(2 << 20)
    })
}

/// # Safety
///
/// Caller must ensure `T` is valid when all bytes are zero. The mapping is
/// never unmapped — this is process-lifetime state.
pub unsafe fn mmap_zeroed<T>() -> &'static mut T {
    let size = std::mem::size_of::<T>();
    assert!(size > 0, "zero-sized T");

    let huge = huge_page_size();
    assert!(std::mem::align_of::<T>() <= huge);

    // Only worth aligning when the mapping can hold at least one huge page.
    // Below that, rounding up would populate tens of MiB of dead RSS.
    let want_thp = size >= huge;
    let bytes = if want_thp {
        size.next_multiple_of(huge)
    } else {
        size
    };
    let reserve = if want_thp {
        bytes.checked_add(huge).expect("mapping size overflow")
    } else {
        bytes
    };

    let base = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            reserve,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    assert!(
        base != libc::MAP_FAILED,
        "mmap of {reserve} bytes failed: {}",
        std::io::Error::last_os_error()
    );

    // Trim to a huge-page-aligned window.
    let addr = base as usize;
    let aligned = if want_thp {
        addr.next_multiple_of(huge)
    } else {
        addr
    };
    let head = aligned - addr;
    if head > 0 {
        unsafe { libc::munmap(base, head) };
    }
    let tail = reserve - head - bytes;
    if tail > 0 {
        unsafe { libc::munmap((aligned + bytes) as *mut libc::c_void, tail) };
    }

    let ptr = aligned as *mut libc::c_void;

    #[cfg(target_os = "linux")]
    unsafe {
        // THP must be requested before populating, or the pages come back
        // at base size and khugepaged has to collapse them later.
        if want_thp && libc::madvise(ptr, bytes, libc::MADV_HUGEPAGE) != 0 {
            eprintln!("MADV_HUGEPAGE failed: {}", std::io::Error::last_os_error());
        }
        if libc::madvise(ptr, bytes, libc::MADV_POPULATE_WRITE) != 0 {
            panic!(
                "MADV_POPULATE_WRITE of {bytes} bytes failed: {}",
                std::io::Error::last_os_error()
            );
        }
    }

    unsafe { &mut *ptr.cast::<T>() }
}
