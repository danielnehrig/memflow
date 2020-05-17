use crate::error::{Error, Result};
use crate::offsets::Win32Offsets;
use crate::win32::Win32;

use flow_core::architecture::Architecture;
use flow_core::mem::{AccessVirtualMemory, VirtualMemoryContext};
use flow_core::types::{Address, Length};
use flow_core::OsProcess;

use log::trace;
use pelite::{self, pe64::exports::Export, PeView};

#[derive(Debug, Clone)]
pub struct Win32Process {
    address: Address,
    pid: i32,
    name: String,
    dtb: Address,
    wow64: Address,
    peb: Address,
    peb_module: Address,
    sys_arch: Architecture,
    proc_arch: Architecture,
}

impl Win32Process {
    pub fn try_from_kernel<T>(mem: &mut T, win: &Win32) -> Result<Self>
    where
        T: AccessVirtualMemory,
    {
        let mut reader = VirtualMemoryContext::with(mem, win.start_block.arch, win.start_block.dtb);

        // read pe header
        let mut pe_buf = vec![0; win.kernel_size.as_usize()];
        reader.virt_read_raw_into(win.kernel_base, &mut pe_buf)?;

        let pe = PeView::from_bytes(&pe_buf)?;

        // find PsActiveProcessHead
        let loaded_module_list = match pe.get_export_by_name("PsLoadedModuleList")? {
            Export::Symbol(s) => win.kernel_base + Length::from(*s),
            Export::Forward(_) => {
                return Err(Error::new(
                    "PsLoadedModuleList found but it was a forwarded export",
                ))
            }
        };

        let peb_module = reader.virt_read_addr(loaded_module_list)?;

        Ok(Self {
            address: win.kernel_base,
            pid: 0,
            name: "ntoskrnl.exe".to_string(),
            dtb: win.start_block.dtb,
            wow64: Address::null(),
            peb: Address::null(),
            peb_module,
            sys_arch: win.start_block.arch,
            proc_arch: win.start_block.arch,
        })
    }

    pub fn try_with_eprocess<T>(
        mem: &mut T,
        win: &Win32,
        offsets: &Win32Offsets,
        eprocess: Address,
    ) -> Result<Self>
    where
        T: AccessVirtualMemory,
    {
        let mut reader = VirtualMemoryContext::with(mem, win.start_block.arch, win.start_block.dtb);

        let mut pid = 0i32;
        reader.virt_read_into(eprocess + offsets.eproc_pid, &mut pid)?;
        trace!("pid={}", pid);
        let name = reader.virt_read_cstr(eprocess + offsets.eproc_name, Length::from(16))?;
        trace!("name={}", name);
        let dtb = reader.virt_read_addr(eprocess + offsets.kproc_dtb)?;
        trace!("dtb={:x}", dtb);
        let wow64 = if offsets.eproc_wow64.is_zero() {
            trace!("eproc_wow64=null; skipping wow64 detection");
            Address::null()
        } else {
            trace!(
                "eproc_wow64=${:x}; trying to read wow64 pointer",
                offsets.eproc_wow64
            );
            reader.virt_read_addr(eprocess + offsets.eproc_wow64)?
        };
        trace!("wow64={:x}", wow64);

        // read peb
        let peb = if wow64.is_null() {
            trace!("reading peb for native process");
            reader.virt_read_addr(eprocess + offsets.eproc_peb)?
        } else {
            trace!("reading peb for wow64 process");
            reader.virt_read_addr(wow64)?
        };
        trace!("peb={:x}", peb);

        let sys_arch = win.start_block.arch;
        trace!("sys_arch={:?}", sys_arch);
        let proc_arch = match sys_arch.bits() {
            64 => {
                if wow64.is_null() {
                    Architecture::X64
                } else {
                    Architecture::X86
                }
            }
            32 => Architecture::X86,
            _ => return Err(Error::new("invalid architecture")),
        };
        trace!("proc_arch={:?}", proc_arch);

        // from here on out we are in the process context
        // we will be using the process type architecture now
        let (peb_ldr_offs, ldr_list_offs) = match proc_arch.bits() {
            64 => (offsets.peb_ldr_x64, offsets.ldr_list_x64),
            32 => (offsets.peb_ldr_x86, offsets.ldr_list_x86),
            _ => return Err(Error::new("invalid architecture")),
        };
        trace!("peb_ldr_offs={:x}", peb_ldr_offs);
        trace!("ldr_list_offs={:x}", ldr_list_offs);

        // construct reader with process dtb
        let mut proc_reader =
            VirtualMemoryContext::with_proc_arch(mem, win.start_block.arch, proc_arch, dtb);
        let peb_ldr = proc_reader.virt_read_addr(peb + peb_ldr_offs)?;
        trace!("peb_ldr={:x}", peb_ldr);

        let peb_module = proc_reader.virt_read_addr(peb_ldr + ldr_list_offs)?;
        trace!("peb_module={:x}", peb_module);

        Ok(Self {
            address: eprocess,
            pid,
            name,
            dtb,
            wow64,
            peb,
            peb_module,
            sys_arch,
            proc_arch,
        })
    }

    pub fn try_with_name<T>(
        mem: &mut T,
        win: &Win32,
        offsets: &Win32Offsets,
        name: &str,
    ) -> Result<Self>
    where
        T: AccessVirtualMemory,
    {
        win.eprocess_list(mem, offsets)?
            .iter()
            .map(|eproc| Win32Process::try_with_eprocess(mem, win, offsets, *eproc))
            .filter_map(Result::ok)
            .inspect(|p| trace!("{} {}", p.pid(), p.name()))
            .find(|p| p.name() == name)
            .ok_or_else(|| Error::new(format!("unable to find process {}", name)))
    }

    pub fn wow64(&self) -> Address {
        self.wow64
    }

    pub fn peb(&self) -> Address {
        self.peb
    }

    pub fn peb_module(&self) -> Address {
        self.peb_module
    }

    pub fn peb_list<T: AccessVirtualMemory>(&self, mem: &mut T) -> Result<Vec<Address>> {
        let mut proc_reader =
            VirtualMemoryContext::with_proc_arch(mem, self.sys_arch, self.proc_arch, self.dtb);

        let mut pebs = Vec::new();

        let list_start = self.peb_module;
        let mut list_entry = list_start;
        loop {
            pebs.push(list_entry);
            list_entry = proc_reader.virt_read_addr(list_entry)?;
            if list_entry.is_null() || list_entry == self.peb_module {
                break;
            }
        }

        Ok(pebs)
    }
}

impl OsProcess for Win32Process {
    fn address(&self) -> Address {
        self.address
    }

    fn pid(&self) -> i32 {
        self.pid
    }

    fn name(&self) -> String {
        self.name.clone()
    }

    fn dtb(&self) -> Address {
        self.dtb
    }

    fn sys_arch(&self) -> Architecture {
        self.sys_arch
    }

    fn proc_arch(&self) -> Architecture {
        self.proc_arch
    }
}
