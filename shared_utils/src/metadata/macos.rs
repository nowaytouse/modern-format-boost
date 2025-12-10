//! macOS native metadata preservation

use std::path::Path;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::io;

// COPYFILE_ACL (1<<0) | COPYFILE_STAT (1<<1) | COPYFILE_XATTR (1<<2)
// ⚠️ 不要添加 COPYFILE_DATA (1<<3)！那会复制文件内容，导致转换无效！
const COPYFILE_FLAGS: u32 = (1<<0) | (1<<1) | (1<<2);

pub fn copy_native_metadata(src: &Path, dst: &Path) -> io::Result<()> {
    extern "C" {
        fn copyfile(from: *const i8, to: *const i8, state: *mut std::ffi::c_void, flags: u32) -> i32;
    }
    let src_c = CString::new(src.as_os_str().as_bytes())?;
    let dst_c = CString::new(dst.as_os_str().as_bytes())?;
    let ret = unsafe { copyfile(src_c.as_ptr(), dst_c.as_ptr(), std::ptr::null_mut(), COPYFILE_FLAGS) };
    if ret < 0 { return Err(io::Error::last_os_error()); }
    Ok(())
}

#[repr(C)]
struct Attrlist { bitmapcount: u16, reserved: u16, commonattr: u32, volattr: u32, dirattr: u32, fileattr: u32, forkattr: u32 }

const ATTR_CMN_CRTIME: u32 = 0x00000200;
const ATTR_CMN_ADDEDTIME: u32 = 0x10000000;
const ATTR_BIT_MAP_COUNT: u16 = 5;

#[repr(C)]
struct Timespec { tv_sec: i64, tv_nsec: i64 }

#[repr(C)]
struct AttrBufAddedTime { length: u32, added_time: Timespec }

pub fn set_creation_time(path: &Path, time: std::time::SystemTime) -> io::Result<()> {
    set_time_attr(path, time, ATTR_CMN_CRTIME)
}

pub fn set_added_time(path: &Path, time: std::time::SystemTime) -> io::Result<()> {
    set_time_attr(path, time, ATTR_CMN_ADDEDTIME)
}

pub fn get_added_time(path: &Path) -> io::Result<std::time::SystemTime> {
    extern "C" {
        fn getattrlist(path: *const i8, attrList: *mut Attrlist, attrBuf: *mut std::ffi::c_void, attrBufSize: usize, options: u32) -> i32;
    }
    let c_path = CString::new(path.as_os_str().as_bytes())?;
    let mut attr_list = Attrlist { bitmapcount: ATTR_BIT_MAP_COUNT, reserved: 0, commonattr: ATTR_CMN_ADDEDTIME, volattr: 0, dirattr: 0, fileattr: 0, forkattr: 0 };
    let mut buf = AttrBufAddedTime { length: 0, added_time: Timespec { tv_sec: 0, tv_nsec: 0 } };
    let ret = unsafe { getattrlist(c_path.as_ptr(), &mut attr_list, &mut buf as *mut _ as *mut std::ffi::c_void, std::mem::size_of::<AttrBufAddedTime>(), 0) };
    if ret != 0 { return Err(io::Error::last_os_error()); }
    let duration = std::time::Duration::new(buf.added_time.tv_sec as u64, buf.added_time.tv_nsec as u32);
    Ok(std::time::SystemTime::UNIX_EPOCH + duration)
}

fn set_time_attr(path: &Path, time: std::time::SystemTime, attr: u32) -> io::Result<()> {
    extern "C" {
        fn setattrlist(path: *const i8, attrList: *mut Attrlist, attrBuf: *mut std::ffi::c_void, attrBufSize: usize, options: u32) -> i32;
    }
    let c_path = CString::new(path.as_os_str().as_bytes())?;
    let mut attr_list = Attrlist { bitmapcount: ATTR_BIT_MAP_COUNT, reserved: 0, commonattr: attr, volattr: 0, dirattr: 0, fileattr: 0, forkattr: 0 };
    let duration = time.duration_since(std::time::SystemTime::UNIX_EPOCH).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    let mut buf = Timespec { tv_sec: duration.as_secs() as i64, tv_nsec: duration.subsec_nanos() as i64 };
    let ret = unsafe { setattrlist(c_path.as_ptr(), &mut attr_list, &mut buf as *mut _ as *mut std::ffi::c_void, std::mem::size_of::<Timespec>(), 0) };
    if ret != 0 { return Err(io::Error::last_os_error()); }
    Ok(())
}
