extern crate mach;

use std::ffi::CStr;
use std::ptr;

use mach::kern_return::*;
use mach::{
    mach_types, memory_object_types, traps, vm, vm_inherit, vm_prot, vm_statistics, vm_types,
};

use super::VirtualRegion;
use crate::Error;

#[repr(C)]
pub struct MachVMRegion {
    begin: *mut u8,
    size: usize,
}

fn mach_error_to_string(err: kern_return_t) -> String {
    let strp = unsafe { mach::bootstrap::bootstrap_strerror(err) };
    if strp.is_null() {
        panic!("couldn't get error string for Mach error code {:#?}", err);
    }
    match unsafe { CStr::from_ptr(strp) }.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => panic!("couldn't get valid UTF-8 error string for Mach error code {:#?}", err),
    }
}

impl MachVMRegion {
    unsafe fn _allocate(
        size: usize,
        target: Option<*mut u8>,
        align: usize,
    ) -> Result<(*mut u8, vm_prot::vm_prot_t), Error> {
        let mut addr = target.unwrap_or(ptr::null_mut()) as vm_types::mach_vm_address_t;
        let prot = vm_prot::VM_PROT_READ | vm_prot::VM_PROT_WRITE;
        let flags = if target.is_none() {
            vm_statistics::VM_FLAGS_ANYWHERE
        } else {
            vm_statistics::VM_FLAGS_FIXED | vm_statistics::VM_FLAGS_OVERWRITE
        };
        let mask = if target.is_none() && align != 0 { align - 1 } else { 0 };
        let kr = vm::mach_vm_map(
            traps::mach_task_self(),
            &mut addr as *mut vm_types::mach_vm_address_t,
            size as vm_types::mach_vm_size_t,
            mask as vm_types::mach_vm_offset_t,
            flags,
            0 as mach_types::mem_entry_name_port_t,
            0 as memory_object_types::memory_object_offset_t,
            false as mach::boolean::boolean_t,
            prot,
            prot,
            vm_inherit::VM_INHERIT_NONE,
        );
        match kr {
            KERN_SUCCESS => Ok((addr as *mut u8, prot)),
            KERN_NO_SPACE => Err(Error::OutOfMemory),
            _ => Err(Error::Generic(mach_error_to_string(kr))),
        }
    }

    unsafe fn _deallocate(begin: *mut u8, size: usize) -> Result<(), Error> {
        let kr = vm::mach_vm_deallocate(
            traps::mach_task_self(),
            begin as vm_types::mach_vm_address_t,
            size as vm_types::mach_vm_size_t,
        );
        match kr {
            KERN_SUCCESS => Ok(()),
            _ => Err(Error::Generic(mach_error_to_string(kr))),
        }
    }

    unsafe fn _remap(
        begin: *mut u8,
        size: usize,
        copy: bool,
        target: Option<*mut u8>,
        align: usize,
    ) -> Result<(*mut u8, vm_prot::vm_prot_t, vm_prot::vm_prot_t), Error> {
        debug_assert!(!begin.is_null());
        debug_assert!(if target.is_some() { !target.as_ref().unwrap().is_null() } else { true });
        debug_assert_ne!(size, 0);
        let mask = if target.is_none() && align != 0 {
            debug_assert!(align.is_power_of_two());
            align - 1
        } else {
            0
        };
        let flags: libc::c_int = if target.is_none() {
            vm_statistics::VM_FLAGS_ANYWHERE
        } else {
            vm_statistics::VM_FLAGS_FIXED | vm_statistics::VM_FLAGS_OVERWRITE
        };
        let mut addr = target.unwrap_or(ptr::null_mut()) as vm_types::mach_vm_address_t;
        let mut prot: vm_prot::vm_prot_t = vm_prot::VM_PROT_NONE;
        let mut max_prot: vm_prot::vm_prot_t = vm_prot::VM_PROT_NONE;
        let kr = vm::mach_vm_remap(
            traps::mach_task_self(),
            &mut addr as *mut vm_types::mach_vm_address_t,
            size as vm_types::mach_vm_size_t,
            mask as vm_types::mach_vm_offset_t,
            flags,
            traps::mach_task_self(),
            begin as vm_types::mach_vm_address_t,
            copy as mach::boolean::boolean_t,
            &mut prot as *mut vm_prot::vm_prot_t,
            &mut max_prot as *mut vm_prot::vm_prot_t,
            vm_inherit::VM_INHERIT_NONE,
        );
        match kr {
            KERN_SUCCESS => Ok((addr as *mut u8, prot, max_prot)),
            KERN_NO_SPACE => Err(Error::OutOfMemory),
            _ => Err(Error::Generic(mach_error_to_string(kr))),
        }
    }
}

impl VirtualRegion for MachVMRegion {
    fn new(size: usize, align: usize) -> Result<MachVMRegion, Error> {
        debug_assert!(size.is_power_of_two());
        debug_assert!(align.is_power_of_two());

        let (addr, _) = unsafe { Self::_allocate(size, None, align)? };
        Ok(MachVMRegion { begin: addr, size })
    }

    unsafe fn from_raw_parts(addr: *mut u8, size: usize) -> MachVMRegion {
        debug_assert!(size.is_power_of_two());

        MachVMRegion { begin: addr, size }
    }

    fn base(&self) -> *mut u8 { self.begin }
    fn size(&self) -> usize { self.size }

    fn map_to(&self, offset: usize, size: usize, target: *mut u8) -> Result<Self, Error> {
        let (addr, _, _) = unsafe {
            Self::_remap(self.begin.offset(offset as isize), size, false, Some(target), 0)?
        };
        Ok(unsafe { MachVMRegion::from_raw_parts(addr, size) })
    }
    fn map_aligned(&self, offset: usize, size: usize, target_align: usize) -> Result<Self, Error> {
        let (addr, _, _) = unsafe {
            Self::_remap(self.begin.offset(offset as isize), size, false, None, target_align)?
        };
        Ok(unsafe { MachVMRegion::from_raw_parts(addr, size) })
    }

    fn dup_to(&self, offset: usize, size: usize, target: *mut u8) -> Result<Self, Error> {
        let (addr, _, _) = unsafe {
            Self::_remap(self.begin.offset(offset as isize), size, true, Some(target), 0)?
        };
        Ok(unsafe { MachVMRegion::from_raw_parts(addr, size) })
    }
    fn dup_aligned(&self, offset: usize, size: usize, target_align: usize) -> Result<Self, Error> {
        let (addr, _, _) = unsafe {
            Self::_remap(self.begin.offset(offset as isize), size, true, None, target_align)?
        };
        Ok(unsafe { MachVMRegion::from_raw_parts(addr, size) })
    }

    fn detach(&mut self) -> Result<(), Error> {
        let (addr, _) = unsafe { Self::_allocate(self.size, Some(self.begin), 0)? };
        if addr != self.begin {
            panic!("detach failed: separated address {:#?} (should be {:#?})", addr, self.begin);
        }
        Ok(())
    }

    fn prot(&mut self, read: bool, write: bool) -> Result<(bool, bool), Error> {
        let flags = match (read, write) {
            (true, true) => vm_prot::VM_PROT_READ | vm_prot::VM_PROT_WRITE,
            (true, false) => vm_prot::VM_PROT_READ,
            (false, true) => vm_prot::VM_PROT_WRITE,
            (false, false) => vm_prot::VM_PROT_NONE,
        };
        let kr = unsafe {
            vm::mach_vm_protect(
                traps::mach_task_self(),
                self.begin as vm_types::mach_vm_address_t,
                self.size as vm_types::mach_vm_size_t,
                /* set_maximum= */ false as mach::boolean::boolean_t,
                flags,
            )
        };
        match kr {
            KERN_SUCCESS => Ok((read, write)),
            _ => Err(Error::Generic(mach_error_to_string(kr))),
        }
    }

    fn consume(self) -> (*mut u8, usize) { (self.begin, self.size) }

    fn free(self) -> Result<(), Error> { unsafe { Self::_deallocate(self.begin, self.size) } }
}

#[cfg(test)]
mod test {
    use super::super::VirtualRegion;
    use super::MachVMRegion;

    const TEST_SIZE: usize = 4 * crate::constants::MB;

    #[test]
    fn test_alloc_free() {
        let r = MachVMRegion::new(TEST_SIZE, TEST_SIZE).unwrap();
        unsafe {
            for i in 0..(TEST_SIZE / 0x1000) {
                *r.base().offset(i as isize * 0x1000) = 1;
            }
        }
        r.free().unwrap();
    }

    #[test]
    fn test_map() {
        let r1 = MachVMRegion::new(0x4000usize, 0x4000usize).unwrap();
        unsafe { *r1.base() = 1 };
        let r2 = r1.map_aligned(0, r1.size(), r1.size()).unwrap();
        unsafe { *r2.base() = 2 };

        assert_eq!(unsafe { *r1.base() }, unsafe { *r2.base() });

        r1.free().unwrap();

        assert_eq!(unsafe { *r2.base() }, 2);

        r2.free().unwrap();
    }

    #[test]
    fn test_map_detach() {
        let r1 = MachVMRegion::new(0x4000usize, 0x4000usize).unwrap();
        unsafe { *r1.base() = 1 };
        let mut r2 = MachVMRegion::new(0x4000usize, 0x4000usize).unwrap();
        unsafe { *r2.base() = 2 };

        assert_ne!(r1.base(), r2.base());

        let r3 = r1.map_to(0, r1.size(), r2.base()).unwrap();
        assert_eq!(r3.base(), r2.base());

        assert_eq!(unsafe { *r1.base() }, unsafe { *r2.base() });
        assert_eq!(unsafe { *r2.base() }, 1);
        unsafe { *r2.base() = 3 };
        assert_eq!(unsafe { *r1.base() }, 3);

        r2.detach().unwrap();

        unsafe { *r2.base() = 4 };
        assert_ne!(unsafe { *r1.base() }, unsafe { *r2.base() });

        r1.free().unwrap();
        r2.free().unwrap();
    }
}
