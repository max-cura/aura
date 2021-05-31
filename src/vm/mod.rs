use crate::Error;

pub trait VirtualRegion: Sized {
    fn new(size: usize, align: usize) -> Result<Self, Error>;
    unsafe fn from_raw_parts(addr: *mut u8, size: usize) -> Self;

    fn base(&self) -> *mut u8;
    fn size(&self) -> usize;

    fn map_to(&self, offset: usize, size: usize, target: *mut u8) -> Result<Self, Error>;
    fn map_aligned(&self, offset: usize, size: usize, target_align: usize) -> Result<Self, Error>;

    fn dup_to(&self, offset: usize, size: usize, target: *mut u8) -> Result<Self, Error>;
    fn dup_aligned(&self, offset: usize, size: usize, target_align: usize) -> Result<Self, Error>;

    fn detach(&mut self) -> Result<(), Error>;

    fn prot(&mut self, read: bool, write: bool) -> Result<(bool, bool), Error>;

    fn consume(self) -> (*mut u8, usize);
    fn free(self) -> Result<(), Error>;
}

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "macos")]
pub type VMRegion = macos::MachVMRegion;

use page_size as extern_page_size;
use parking_lot::Once;

static mut PAGE_SIZE: usize = 0x4000usize;
static PAGE_SIZE_INIT: Once = Once::new();

pub fn page_size() -> usize {
    PAGE_SIZE_INIT.call_once(|| {
        unsafe { PAGE_SIZE = extern_page_size::get() };
    });
    unsafe { PAGE_SIZE }
}

pub const fn align_size(size: usize, align: usize) -> usize {
    if 0 != ((align - 1) & size) {
        (size + align) & (align - 1)
    } else {
        size
    }
}
