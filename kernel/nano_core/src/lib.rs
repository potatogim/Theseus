// Copyright 2017 Kevin Boos. 
// Licensed under the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>,
// This file may not be copied, modified, or distributed
// except according to those terms.



#![feature(lang_items)]
#![feature(const_fn, unique)]
#![feature(alloc, collections)]
#![feature(asm)]
#![feature(naked_functions)]
#![feature(abi_x86_interrupt)]
#![feature(drop_types_in_const)] 
#![feature(compiler_fences)]
#![no_std]


// #![feature(compiler_builtins_lib)]  // this is needed for our odd approach of including the nano_core as a library for other kernel crates
// extern crate compiler_builtins; // this is needed for our odd approach of including the nano_core as a library for other kernel crates


// ------------------------------------
// ----- EXTERNAL CRATES BELOW --------
// ------------------------------------
extern crate rlibc;
extern crate volatile;
extern crate spin; // core spinlocks 
extern crate multiboot2;
#[macro_use] extern crate bitflags;
extern crate x86;
#[macro_use] extern crate x86_64;
#[macro_use] extern crate once; // for assert_has_not_been_called!()
extern crate bit_field;
#[macro_use] extern crate lazy_static; // for lazy static initialization
extern crate alloc;
#[macro_use] extern crate collections;
#[macro_use] extern crate log;
//extern crate atomic;


// ------------------------------------
// ------ OUR OWN CRATES BELOW --------
// ------------------------------------
extern crate kernel_config; // our configuration options, just a set of const definitions.
extern crate irq_safety; // for irq-safe locking and interrupt utilities
extern crate keycodes_ascii; // for keyboard 
extern crate port_io; // for port_io, replaces external crate "cpu_io"
extern crate heap_irq_safe; // our wrapper around the linked_list_allocator crate
extern crate serial_port;
#[macro_use] extern crate vga_buffer; 
extern crate dfqueue; // decoupled, fault-tolerant queue
extern crate test_lib;


#[macro_use] mod console;  // I think this mod declaration MUST COME FIRST because it includes the macro for println!
#[macro_use] mod drivers;  
#[macro_use] mod util;
mod arch;
mod logger;
#[macro_use] mod task;
mod memory;
mod interrupts;
mod syscall;


use spin::RwLockWriteGuard;
use irq_safety::{RwLockIrqSafe, RwLockIrqSafeReadGuard, RwLockIrqSafeWriteGuard};
use task::TaskList;
use collections::string::String;
use core::sync::atomic::{AtomicUsize, Ordering};
use interrupts::tsc;
use drivers::{ata_pio, pci};



fn test_loop_1(_: Option<u64>) -> Option<u64> {
    debug!("Entered test_loop_1!");
    loop {
        let mut i = 10000000; // usize::max_value();
        while i > 0 {
            i -= 1;
        }
        print!("1");
    }
}


fn test_loop_2(_: Option<u64>) -> Option<u64> {
    debug!("Entered test_loop_2!");
    loop {
        let mut i = 10000000; // usize::max_value();
        while i > 0 {
            i -= 1;
        }
        print!("2");
    }
}


fn test_loop_3(_: Option<u64>) -> Option<u64> {
    debug!("Entered test_loop_3!");
    loop {
        let mut i = 10000000; // usize::max_value();
        while i > 0 {
            i -= 1;
        }
        print!("3");
    }
}




fn first_thread_main(arg: Option<u64>) -> u64  {
    println!("Hello from first thread, arg: {:?}!!", arg);
    1
}

fn second_thread_main(arg: u64) -> u64  {
    println!("Hello from second thread, arg: {}!!", arg);
    2
}


fn third_thread_main(arg: String) -> String {
    println!("Hello from third thread, arg: {}!!", arg);
    String::from("3")
}


fn fourth_thread_main(arg: u64) -> Option<String> {
    println!("Hello from fourth thread, arg: {:?}!!", arg);
    // String::from("returned None")
    None
}



#[no_mangle]
pub extern "C" fn rust_main(multiboot_information_physical_address: usize) {
	
	// start the kernel with interrupts disabled
	unsafe { ::x86_64::instructions::interrupts::disable(); }
	
    // early initialization of things like vga console and logging that don't require memory system.
    logger::init_logger().expect("WTF: couldn't init logger.");
    println_unsafe!("Logger initialized.");
    
    drivers::early_init();
    
    println_unsafe!("multiboot_information_physical_address: {:#x}", multiboot_information_physical_address);
    let boot_info = unsafe { multiboot2::load(multiboot_information_physical_address) };
    enable_nxe_bit();
    enable_write_protect_bit();

    // init memory management: set up stack with guard page, heap, kernel text/data mappings, etc
    // this returns a MMI struct with the page table, stack allocator, and VMA list for the kernel's address space (task_zero)
    let mut kernel_mmi: memory::MemoryManagementInfo = memory::init(boot_info);

    
    // initialize our interrupts and IDT
    let double_fault_stack = kernel_mmi.alloc_stack(1).expect("could not allocate double fault stack");
    let privilege_stack = kernel_mmi.alloc_stack(4).expect("could not allocate privilege stack");
    let syscall_stack = kernel_mmi.alloc_stack(4).expect("could not allocate syscall stack");
    interrupts::init(double_fault_stack.top_unusable(), privilege_stack.top_unusable());

    syscall::init(syscall_stack.top_usable());

    // println_unsafe!("KernelCode: {:#x}", interrupts::get_segment_selector(interrupts::AvailableSegmentSelector::KernelCode).0); 
    // println_unsafe!("KernelData: {:#x}", interrupts::get_segment_selector(interrupts::AvailableSegmentSelector::KernelData).0); 
    // println_unsafe!("UserCode32: {:#x}", interrupts::get_segment_selector(interrupts::AvailableSegmentSelector::UserCode32).0); 
    // println_unsafe!("UserData32: {:#x}", interrupts::get_segment_selector(interrupts::AvailableSegmentSelector::UserData32).0); 
    // println_unsafe!("UserCode64: {:#x}", interrupts::get_segment_selector(interrupts::AvailableSegmentSelector::UserCode64).0); 
    // println_unsafe!("UserData64: {:#x}", interrupts::get_segment_selector(interrupts::AvailableSegmentSelector::UserData64).0); 
    // println_unsafe!("TSS:        {:#x}", interrupts::get_segment_selector(interrupts::AvailableSegmentSelector::Tss).0); 

    // create the initial `Task`, called task_zero
    // this is scoped in order to automatically release the tasklist RwLockIrqSafe
    // TODO: transform this into something more like "task::init(initial_mmi)"
    {
        let mut tasklist_mut: RwLockIrqSafeWriteGuard<TaskList> = task::get_tasklist().write();
        tasklist_mut.init_task_zero(kernel_mmi);
    }

    // initialize the kernel console
    let console_queue_producer = console::console_init(task::get_tasklist().write());

    // initialize the rest of our drivers
    drivers::init(console_queue_producer);



    println!("initialization done!");

	
	//interrupts::enable_interrupts(); //apparently this line is unecessary
	println!("enabled interrupts!");


    // create a second task to test context switching
    if true {
        let mut tasklist_mut: RwLockIrqSafeWriteGuard<TaskList> = task::get_tasklist().write();    
        { let _second_task = tasklist_mut.spawn_kthread(first_thread_main, Some(6),  "first_thread"); }
        { let _second_task = tasklist_mut.spawn_kthread(second_thread_main, 6, "second_thread"); }
        { let _second_task = tasklist_mut.spawn_kthread(third_thread_main, String::from("hello"), "third_thread"); } 
        { let _second_task = tasklist_mut.spawn_kthread(fourth_thread_main, 12345u64, "fourth_thread"); }

        // must be lexically scoped like this to avoid the "multiple mutable borrows" error
        { tasklist_mut.spawn_kthread(test_loop_1, None, "test_loop_1"); }
        { tasklist_mut.spawn_kthread(test_loop_2, None, "test_loop_2"); } 
        { tasklist_mut.spawn_kthread(test_loop_3, None, "test_loop_3"); } 
    }
    
    // try to schedule in the second task
    info!("attempting to schedule away from zeroth init task");
    schedule!();


    // the idle thread's (Task 0) busy loop
    trace!("Entering Task0's idle loop");
	

    // create and jump to the first userspace thread
    if true
    {
        debug!("trying to jump to userspace");
        let mut tasklist_mut: RwLockIrqSafeWriteGuard<TaskList> = task::get_tasklist().write();   
        let module = memory::get_module(0).expect("Error: no userspace modules found!");
        tasklist_mut.spawn_userspace(module, Some("userspace_module"));
    }

    if true
    {
        debug!("trying to jump to userspace 2nd time");
        let mut tasklist_mut: RwLockIrqSafeWriteGuard<TaskList> = task::get_tasklist().write();   
        let module = memory::get_module(0).expect("Error: no userspace modules found!");
        tasklist_mut.spawn_userspace(module, Some("userspace_module_2"));
    }

    // create and jump to a userspace thread that tests syscalls
    if true
    {
        debug!("trying out a system call module");
        let mut tasklist_mut: RwLockIrqSafeWriteGuard<TaskList> = task::get_tasklist().write();   
        let module = memory::get_module(1).expect("Error: no module 2 found!");
        tasklist_mut.spawn_userspace(module, Some("syscall_test"));
    }

    // a second duplicate syscall test user task
    if true
    {
        debug!("trying out a second system call module");
        let mut tasklist_mut: RwLockIrqSafeWriteGuard<TaskList> = task::get_tasklist().write();   
        let module = memory::get_module(1).expect("Error: no module 2 found!");
        tasklist_mut.spawn_userspace(module, Some("syscall_test_2"));
    }


    debug!("rust_main(): entering idle loop: interrupts enabled: {}", interrupts::interrupts_enabled());

    use test_lib;
    println!("test_lib::test_lib_func(10) = {}", test_lib::test_lib_func(10));


    loop { 
        // TODO: exit this loop cleanly upon a shutdown signal
    }


    // cleanup here
    logger::shutdown().expect("WTF: failed to shutdown logger... oh well.");
    
    

}

fn enable_nxe_bit() {
    use x86_64::registers::msr::{IA32_EFER, rdmsr, wrmsr};

    let nxe_bit = 1 << 11;
    unsafe {
        let efer = rdmsr(IA32_EFER);
        wrmsr(IA32_EFER, efer | nxe_bit);
    }
}

fn enable_write_protect_bit() {
    use x86_64::registers::control_regs::{cr0, cr0_write, Cr0};

    unsafe { cr0_write(cr0() | Cr0::WRITE_PROTECT) };
}

#[cfg(not(test))]
#[lang = "eh_personality"]
extern "C" fn eh_personality() {}

#[cfg(not(test))]
#[lang = "panic_fmt"]
#[no_mangle]
pub extern "C" fn panic_fmt(fmt: core::fmt::Arguments, file: &'static str, line: u32) -> ! {
    println_unsafe!("\n\nPANIC in {} at line {}:", file, line);
    println_unsafe!("    {}", fmt);

    // TODO: check out Redox's unwind implementation: https://github.com/redox-os/kernel/blob/b364d052f20f1aa8bf4c756a0a1ea9caa6a8f381/src/arch/x86_64/interrupt/trace.rs#L9

    loop {}
}

#[allow(non_snake_case)]
#[no_mangle]
pub extern "C" fn _Unwind_Resume() -> ! {
    println_unsafe!("\n\nin _Unwind_Resume, unimplemented!");
    loop {}
}