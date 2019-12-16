use crate::error::{Error, Result};
use log::{debug, info};

use super::Windows;

use flow_core::address::{Address, Length};
use flow_core::arch::{Architecture, InstructionSet, SystemArchitecture};
use flow_core::mem::*;

use std::cell::RefCell;
use std::rc::Rc;

use super::module::ModuleIterator;

pub struct ProcessIterator<T: VirtualRead> {
    win: Rc<RefCell<Windows<T>>>,
    first_eprocess: Address,
    eprocess: Address,
}

impl<T: VirtualRead> ProcessIterator<T> {
    pub fn new(win: Rc<RefCell<Windows<T>>>) -> Self {
        let eprocess = win.borrow().eprocess_base;
        Self {
            win,
            first_eprocess: eprocess,
            eprocess,
        }
    }
}

impl<T: VirtualRead> Iterator for ProcessIterator<T> {
    type Item = Process<T>;

    fn next(&mut self) -> Option<Process<T>> {
        // is eprocess null (first iter, read err, sysproc)?
        if self.eprocess.is_null() {
            return None;
        }

        // copy memory for the lifetime of this function
        let win = self.win.borrow();
        let start_block = win.start_block;
        let kernel_pdb = &mut win.kernel_pdb.as_ref()?.borrow_mut();

        let memory = &mut win.mem.borrow_mut();

        // resolve offsets
        let _eprocess = kernel_pdb.find_struct("_EPROCESS")?;
        let _eprocess_links = _eprocess.find_field("ActiveProcessLinks")?.offset;

        let _list_entry = kernel_pdb.find_struct("_LIST_ENTRY")?;
        let _list_entry_blink = _list_entry.find_field("Blink")?.offset;

        // read next eprocess entry
        let mut next = VirtualReader::with(&mut **memory, start_block.arch, start_block.dtb)
            .virt_read_addr(self.eprocess + _eprocess_links + _list_entry_blink)
            .unwrap(); // TODO: convert to Option
        if !next.is_null() {
            next -= _eprocess_links;
        }

        // if next process is 'system' again just null it
        if next == self.first_eprocess {
            next = Address::null();
        }

        // return the previous process and set 'next' for next iter
        let cur = self.eprocess;
        self.eprocess = next;

        Some(Process::with(self.win.clone(), cur))
    }
}

pub trait ProcessTrait {
    fn pid(&mut self) -> Result<i32>;
    fn name(&mut self) -> Result<String>;
    fn dtb(&mut self) -> Result<Address>;

    fn first_peb_entry(&mut self) -> Result<Address>;
    //fn module_iter(&self) -> Result<ModuleIterator<Self>>; // TODO: implement me
}

pub struct Process<T: VirtualRead> {
    pub win: Rc<RefCell<Windows<T>>>,
    pub eprocess: Address,
}

impl<T: VirtualRead> Clone for Process<T>
where
    Rc<RefCell<T>>: Clone,
    Address: Clone,
{
    fn clone(&self) -> Self {
        Self {
            win: self.win.clone(),
            eprocess: self.eprocess,
        }
    }
}

// TODO: read/ret "ProcessInfo"
impl<T: VirtualRead> Process<T> {
    pub fn with(win: Rc<RefCell<Windows<T>>>, eprocess: Address) -> Self {
        Self { win, eprocess }
    }

    // TODO: macro? pub?
    pub fn find_offset(&mut self, strct: &str, field: &str) -> Result<Length> {
        let win = &mut self.win.borrow_mut();
        let mut _pdb = win
            .kernel_pdb
            .as_ref()
            .ok_or_else(|| "kernel pdb not found")?
            .borrow_mut();
        let _strct = _pdb
            .find_struct(strct)
            .ok_or_else(|| format!("{} not found", strct))?;
        let _field = _strct
            .find_field(field)
            .ok_or_else(|| format!("{} not found", field))?;
        debug!("offset {}::{}={:x}", strct, field, _field.offset);
        Ok(_field.offset)
    }

    // system arch = type arch
    pub fn wow64(&mut self) -> Result<Address> {
        let offs = self.find_offset("_EPROCESS", "WoW64Process")?;
        let win = self.win.borrow();
        let start_block = win.start_block;
        let mem = &mut win.mem.borrow_mut();
        Ok(
            VirtualReader::with(&mut **mem, start_block.arch, start_block.dtb)
                .virt_read_addr(self.eprocess + offs)?,
        )
    }

    pub fn has_wow64(&mut self) -> Result<bool> {
        Ok(!self.wow64()?.is_null())
    }

    // module_iter will explicitly clone self and feed it into an iterator
    pub fn module_iter(&self) -> Result<ModuleIterator<Process<T>>> {
        let rc = Rc::new(RefCell::new(self.clone()));
        ModuleIterator::new(rc)
    }
}

impl<T: VirtualRead> ProcessTrait for Process<T> {
    // system arch = type arch
    fn pid(&mut self) -> Result<i32> {
        let offs = self.find_offset("_EPROCESS", "UniqueProcessId")?;
        let win = self.win.borrow();
        let start_block = win.start_block;
        let mem = &mut win.mem.borrow_mut();
        Ok(
            VirtualReader::with(&mut **mem, start_block.arch, start_block.dtb)
                .virt_read_i32(self.eprocess + offs)?,
        )
    }

    // system arch = type arch
    fn name(&mut self) -> Result<String> {
        let offs = self.find_offset("_EPROCESS", "ImageFileName")?;
        let win = self.win.borrow();
        let start_block = win.start_block;
        let mem = &mut win.mem.borrow_mut();
        Ok(
            VirtualReader::with(&mut **mem, start_block.arch, start_block.dtb)
                .virt_read_cstr(self.eprocess + offs, 16)?,
        )
    }

    // system arch = type arch
    fn dtb(&mut self) -> Result<Address> {
        // _KPROCESS is the first entry in _EPROCESS
        let offs = self.find_offset("_KPROCESS", "DirectoryTableBase")?;
        let win = self.win.borrow();
        let start_block = win.start_block;
        let mem = &mut win.mem.borrow_mut();
        Ok(
            VirtualReader::with(&mut **mem, start_block.arch, start_block.dtb)
                .virt_read_addr(self.eprocess + offs)?,
        )
    }

    fn first_peb_entry(&mut self) -> Result<Address> {
        let wow64 = self.wow64()?;
        info!("wow64={:x}", wow64);

        let peb = if wow64.is_null() {
            // x64
            info!("reading peb for x64 process");
            let offs = self.find_offset("_EPROCESS", "Peb")?;
            self.virt_read_addr(self.eprocess + offs)?
        } else {
            // wow64 (first entry in wow64 struct = peb)
            info!("reading peb for wow64 process");
            self.virt_read_addr(wow64)?
        };
        info!("peb={:x}", peb);

        // TODO: process.virt_read_addr based on wow64 or not
        // TODO: forward declare virtual read in process?
        // TODO: use process architecture agnostic wrapper from here!

        // from here on out we are in the process context
        // we will be using the process type architecture now

        // process architecture agnostic offsets
        let proc_arch = self.arch()?;

        let ldr_offs = match proc_arch.instruction_set {
            InstructionSet::X64 => Length::from(0x18), // self.get_offset("_PEB", "Ldr")?,
            InstructionSet::X86 => Length::from(0xC),
            _ => return Err(Error::new("invalid process architecture")),
        };

        let ldr_list_offs = match proc_arch.instruction_set {
            InstructionSet::X64 => Length::from(0x10), // self.get_offset("_PEB_LDR_DATA", "InLoadOrderModuleList")?,
            InstructionSet::X86 => Length::from(0xC),
            _ => return Err(Error::new("invalid process architecture")),
        };

        // read PPEB_LDR_DATA Ldr
        // addr_t peb_ldr = this->read_ptr(peb + this->mo_ldr);
        let peb_ldr = self.virt_read_addr(peb + ldr_offs)?;
        info!("peb_ldr={:x}", peb_ldr);

        // loop LIST_ENTRY InLoadOrderModuleList
        // addr_t first_module = this->read_ptr(peb_ldr + this->mo_ldr_list);
        let first_module = self.virt_read_addr(peb_ldr + ldr_list_offs)?;
        info!("first_module={:x}", first_module);
        Ok(first_module)
    }
}

impl<T: VirtualRead> SystemArchitecture for Process<T> {
    fn arch(&mut self) -> flow_core::Result<Architecture> {
        let win = self.win.borrow();
        Ok(win.start_block.arch)
    }
}

impl<T: VirtualRead> TypeArchitecture for Process<T> {
    fn type_arch(&mut self) -> flow_core::Result<Architecture> {
        let start_block = {
            let win = self.win.borrow();
            win.start_block
        };
        if start_block.arch.instruction_set == InstructionSet::X86 {
            Ok(Architecture::from(InstructionSet::X86))
        } else if !self.has_wow64().map_err(flow_core::Error::new)? {
            Ok(Architecture::from(InstructionSet::X64))
        } else {
            Ok(Architecture::from(InstructionSet::X86))
        }
    }
}

// TODO: this is not entirely correct as it will use different VAT than required, split vat arch + type arch up again
impl<T: VirtualRead> VirtualReadHelper for Process<T> {
    fn virt_read(&mut self, addr: Address, len: Length) -> flow_core::Result<Vec<u8>> {
        let proc_arch = self.arch().map_err(flow_core::Error::new)?;
        let dtb = self.dtb().map_err(flow_core::Error::new)?;
        let win = self.win.borrow();
        let mem = &mut win.mem.borrow_mut();
        mem.virt_read(proc_arch, dtb, addr, len)
    }
}
