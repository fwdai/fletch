//! libkrun, loaded at runtime. Signatures from libkrun.h (1.19).

use std::ffi::{c_char, c_void, CStr, CString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::launch::LaunchConfig;

const LIBKRUN: &str = "libkrun.1.dylib";
const LIBKRUNFW: &str = "libkrunfw.5.dylib";
/// Where a bundled helper (`Contents/Helpers`) finds the dylibs.
const BUNDLE_LIB_DIR: &str = "@executable_path/../Frameworks";
/// libkrun.h's `KRUN_SEMANTICS_LINUX_SIMPLIFIED`.
const KRUN_SEMANTICS_LINUX_SIMPLIFIED: u32 = 1;

type CtxFn = unsafe extern "C" fn() -> i32;
type U32Fn = unsafe extern "C" fn(u32) -> i32;
type VmConfigFn = unsafe extern "C" fn(u32, u8, u32) -> i32;
type AddDiskFn = unsafe extern "C" fn(u32, *const c_char, *const c_char, bool) -> i32;
type AddVirtiofsFn = unsafe extern "C" fn(u32, *const c_char, *const c_char, u64, bool, u32) -> i32;
type RootDiskFn = unsafe extern "C" fn(u32, *const c_char, *const c_char, *const c_char) -> i32;
type StrFn = unsafe extern "C" fn(u32, *const c_char) -> i32;
type U32U32Fn = unsafe extern "C" fn(u32, u32) -> i32;
type VsockPortFn = unsafe extern "C" fn(u32, u32, *const c_char, bool) -> i32;
type ExecFn =
    unsafe extern "C" fn(u32, *const c_char, *const *const c_char, *const *const c_char) -> i32;

pub struct Krun {
    set_log_level: U32Fn,
    create_ctx: CtxFn,
    set_vm_config: VmConfigFn,
    add_disk: AddDiskFn,
    add_virtiofs: AddVirtiofsFn,
    set_root_disk_remount: RootDiskFn,
    disable_implicit_vsock: U32Fn,
    add_vsock: U32U32Fn,
    add_vsock_port2: VsockPortFn,
    set_workdir: StrFn,
    set_exec: ExecFn,
    start_enter: U32Fn,
}

impl Krun {
    pub fn load(lib_dir: Option<&Path>) -> Result<Krun, String> {
        let dir = lib_dir.map_or_else(|| PathBuf::from(BUNDLE_LIB_DIR), Path::to_path_buf);
        // libkrun dlopens libkrunfw by leaf name. Loading it first by full path
        // makes that lookup find the already-loaded image, with no
        // DYLD_LIBRARY_PATH (which the hardened runtime strips anyway).
        dlopen(&dir.join(LIBKRUNFW))?;
        let lib = dlopen(&dir.join(LIBKRUN))?;
        // SAFETY: each symbol is cast to its signature in libkrun.h.
        unsafe {
            Ok(Krun {
                set_log_level: sym(lib, c"krun_set_log_level")?,
                create_ctx: sym(lib, c"krun_create_ctx")?,
                set_vm_config: sym(lib, c"krun_set_vm_config")?,
                add_disk: sym(lib, c"krun_add_disk")?,
                add_virtiofs: sym(lib, c"krun_add_virtiofs4")?,
                set_root_disk_remount: sym(lib, c"krun_set_root_disk_remount")?,
                disable_implicit_vsock: sym(lib, c"krun_disable_implicit_vsock")?,
                add_vsock: sym(lib, c"krun_add_vsock")?,
                add_vsock_port2: sym(lib, c"krun_add_vsock_port2")?,
                set_workdir: sym(lib, c"krun_set_workdir")?,
                set_exec: sym(lib, c"krun_set_exec")?,
                start_enter: sym(lib, c"krun_start_enter")?,
            })
        }
    }

    /// Configures a context and enters the VM. Returns only on failure.
    pub fn run(&self, config: &LaunchConfig) -> String {
        match self.configure(config) {
            // SAFETY: ctx came from krun_create_ctx and is fully configured.
            Ok(ctx) => {
                let err = unsafe { (self.start_enter)(ctx) };
                format!("krun_start_enter failed: {err}")
            }
            Err(err) => err,
        }
    }

    fn configure(&self, c: &LaunchConfig) -> Result<u32, String> {
        // SAFETY: every pointer passed below is a live, NUL-terminated CString
        // (or a NULL-terminated array of them) that outlives the call; libkrun
        // copies what it keeps.
        unsafe {
            if c.log_level > 0 {
                check("krun_set_log_level", (self.set_log_level)(c.log_level))?;
            }
            let ctx = (self.create_ctx)();
            check("krun_create_ctx", ctx)?;
            let ctx = ctx as u32;
            check(
                "krun_set_vm_config",
                (self.set_vm_config)(ctx, c.cpus, c.memory_mib),
            )?;
            for disk in &c.disks {
                let (id, path) = (cstr(&disk.id)?, cpath(&disk.path)?);
                check(
                    "krun_add_disk",
                    (self.add_disk)(ctx, id.as_ptr(), path.as_ptr(), disk.read_only),
                )?;
            }
            let (dev, fs, opts) = (
                cstr(&c.root.device)?,
                cstr(&c.root.fstype)?,
                cstr(&c.root.options)?,
            );
            check(
                "krun_set_root_disk_remount",
                (self.set_root_disk_remount)(ctx, dev.as_ptr(), fs.as_ptr(), opts.as_ptr()),
            )?;
            // The folders the user shared with this box. Simplified semantics:
            // every file reads as owned by the guest process asking (the agent),
            // and modes stay the host's own, so the user's folder is never
            // written ownership xattrs. No DAX window.
            for share in &c.shares {
                let (tag, path) = (cstr(&share.tag)?, cpath(&share.path)?);
                check(
                    "krun_add_virtiofs4",
                    (self.add_virtiofs)(
                        ctx,
                        tag.as_ptr(),
                        path.as_ptr(),
                        0,
                        share.read_only,
                        KRUN_SEMANTICS_LINUX_SIMPLIFIED,
                    ),
                )?;
            }
            // No network: a vsock device without TSI, so guest sockets reach
            // nothing. The only ways out are the ports mapped below, each
            // served or bound by the host.
            check(
                "krun_disable_implicit_vsock",
                (self.disable_implicit_vsock)(ctx),
            )?;
            check("krun_add_vsock", (self.add_vsock)(ctx, 0))?;
            let ports = c
                .vsock_connect
                .iter()
                .map(|p| (p, false))
                .chain(c.vsock_listen.iter().map(|p| (p, true)));
            for (port, listen) in ports {
                let path = cpath(&port.socket)?;
                check(
                    "krun_add_vsock_port2",
                    (self.add_vsock_port2)(ctx, port.port, path.as_ptr(), listen),
                )?;
            }
            let workdir = cstr(&c.workdir)?;
            check(
                "krun_set_workdir",
                (self.set_workdir)(ctx, workdir.as_ptr()),
            )?;

            let (exec, args) = c.exec.split_first().ok_or("empty exec")?;
            let exec = cstr(exec)?;
            let args = args
                .iter()
                .map(|a| cstr(a))
                .collect::<Result<Vec<_>, _>>()?;
            let env = c
                .env
                .iter()
                .map(|e| cstr(e))
                .collect::<Result<Vec<_>, _>>()?;
            let (argv, envp) = (null_terminated(&args), null_terminated(&env));
            check(
                "krun_set_exec",
                (self.set_exec)(ctx, exec.as_ptr(), argv.as_ptr(), envp.as_ptr()),
            )?;
            Ok(ctx)
        }
    }
}

fn check(what: &str, ret: i32) -> Result<(), String> {
    if ret < 0 {
        Err(format!("{what} failed: {ret}"))
    } else {
        Ok(())
    }
}

fn cstr(s: &str) -> Result<CString, String> {
    CString::new(s).map_err(|_| format!("NUL byte in {s:?}"))
}

fn cpath(p: &Path) -> Result<CString, String> {
    CString::new(p.as_os_str().as_bytes()).map_err(|_| format!("NUL byte in {}", p.display()))
}

fn null_terminated(items: &[CString]) -> Vec<*const c_char> {
    items
        .iter()
        .map(|s| s.as_ptr())
        .chain([std::ptr::null()])
        .collect()
}

fn dlopen(path: &Path) -> Result<*mut c_void, String> {
    let c = cpath(path)?;
    // SAFETY: c is a valid C string; the handle is never closed.
    let handle = unsafe { libc::dlopen(c.as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL) };
    if handle.is_null() {
        return Err(format!("dlopen {}: {}", path.display(), dlerror()));
    }
    Ok(handle)
}

/// # Safety
/// `T` must be the function pointer type matching the symbol's C signature.
unsafe fn sym<T: Copy>(lib: *mut c_void, name: &CStr) -> Result<T, String> {
    let ptr = unsafe { libc::dlsym(lib, name.as_ptr()) };
    if ptr.is_null() {
        return Err(format!("dlsym {}: {}", name.to_string_lossy(), dlerror()));
    }
    // SAFETY: T is a function pointer, the same size as *mut c_void.
    Ok(unsafe { std::mem::transmute_copy(&ptr) })
}

fn dlerror() -> String {
    // SAFETY: dlerror returns NULL or a C string valid until the next dl* call.
    let msg = unsafe { libc::dlerror() };
    if msg.is_null() {
        "unknown error".into()
    } else {
        unsafe { CStr::from_ptr(msg) }
            .to_string_lossy()
            .into_owned()
    }
}
