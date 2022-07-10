#![feature(naked_functions)]

use std::arch::asm;

const STACK_BUFFER_SIZE: usize = 1024 * 1024 * 5; // 5MB
const MAX_STACKS: usize = 5;
const PROCESS_MAIN_STACK_ID: usize = 0;
const TRIGGER_OFFSET: isize = -32;
const DATA_SHARE_BUFFER_SIZE: usize = 1024 * 1024 * 5; // 5MB

static mut RUNTIME_PTR: *mut LeydiStacks = 0 as *mut LeydiStacks;

pub struct LeydiStacks {
    stack_pool: Vec<Stack>,
    curr_stack_id: usize,
    data_buffer: ShareBuffer,
}

#[allow(dead_code)]
struct Stack {
    stack_id: usize,
    state: State,
    stack_buffer: Vec<u8>,
    stack_context: StackContext,
}

#[derive(PartialEq, Eq, Debug)]
enum State {
    AVAIABLE,
    RUNNING,
    READY,
}

#[derive(Debug, Default)]
#[repr(C)]
struct StackContext {
    rsp: u64,
    r15: u64,
    r14: u64,
    r13: u64,
    r12: u64,
    rbx: u64,
    rbp: u64,
    edi: u64,
    esi: u64,
}

#[derive(Debug, Default, Clone, Copy)]
#[repr(C)]
pub struct Event {
    pair: (usize, usize),
    data: usize,
}

#[derive(Debug)]
#[repr(C)]
enum ScheduleType {
    RR,
    O1(usize),
}

#[derive(Debug)]
struct ShareBuffer {
    data_pool: Vec<Event>,
    avaiable_index: usize,
}

impl ShareBuffer {
    fn new() -> ShareBuffer {
        let data_pool = Vec::<Event>::with_capacity(DATA_SHARE_BUFFER_SIZE);
        ShareBuffer {
            data_pool,
            avaiable_index: 0,
        }
    }
}

impl Stack {
    fn new(stack_id: usize, state: State) -> Self {
        Stack {
            stack_id,
            stack_buffer: vec![0_u8; STACK_BUFFER_SIZE],
            stack_context: StackContext::default(),
            state,
        }
    }
}

impl LeydiStacks {
    #[cfg(target_arch = "x86_64")]
    pub fn new() -> Self {
        let process_main_stack = Stack::new(PROCESS_MAIN_STACK_ID, State::RUNNING);

        let mut stack_pool = Vec::with_capacity(MAX_STACKS);
        stack_pool.push(process_main_stack);

        let mut avaiable_stacks: Vec<Stack> = (1..=MAX_STACKS)
            .map(|i| Stack::new(i, State::AVAIABLE))
            .collect();
        stack_pool.append(&mut avaiable_stacks);

        LeydiStacks {
            stack_pool,
            curr_stack_id: PROCESS_MAIN_STACK_ID,
            data_buffer: ShareBuffer::new(),
        }
    }

    pub fn run(&mut self) -> () {
        unsafe {
            RUNTIME_PTR = self as *mut LeydiStacks;
        }
        while self.switch_stack(ScheduleType::RR) {}
    }

    #[inline(never)]
    fn switch_stack(&mut self, schedule_type: ScheduleType) -> bool {
        let mut ready_stack_id = 0 as usize;

        match schedule_type {
            ScheduleType::RR => {
                // Get a READY stack id
                while self.stack_pool[ready_stack_id].state != State::READY {
                    ready_stack_id += 1;
                    if ready_stack_id == MAX_STACKS {
                        ready_stack_id = 0;
                    }
                    if ready_stack_id == self.curr_stack_id {
                        return false;
                    }
                }
            }
            ScheduleType::O1(id) => {
                ready_stack_id = id;
            }
        }

        // set ready stack state READY to RUNNING
        self.stack_pool[ready_stack_id].state = State::RUNNING;

        // set current stack RUNNING to READY
        if self.stack_pool[self.curr_stack_id].state != State::AVAIABLE {
            self.stack_pool[self.curr_stack_id].state = State::READY;
        }

        let paused_stack_id = self.curr_stack_id;
        self.curr_stack_id = ready_stack_id;

        unsafe {
            let paused_stack_context: *mut StackContext =
                &mut self.stack_pool[paused_stack_id].stack_context;

            let ready_stack_context: *const StackContext =
                &self.stack_pool[ready_stack_id].stack_context;

            asm!("call switch_and_run", in("rdi") paused_stack_context, in("rsi") ready_stack_context, clobber_abi("C"));
        }
        true
    }

    pub fn new_stack(&mut self, base_function: fn(), trigger_function: fn(usize, usize)) {
        let new_stack = self
            .stack_pool
            .iter_mut()
            .find(|t| t.state == State::AVAIABLE)
            .expect("No avaiable stack found in pool.");

        // set stack as READY
        new_stack.state = State::READY;

        unsafe {
            let stack_buff_ptr = new_stack
                .stack_buffer
                .as_mut_ptr()
                .offset(new_stack.stack_buffer.len() as isize);

            let stack_buff_ptr = (stack_buff_ptr as usize & !15) as *mut u8;

            let mut _buffer_index: *mut u64 = 0 as *mut u64;

            //**************Event Flow***************/
            _buffer_index = stack_buff_ptr.offset(-16) as *mut u64;
            std::ptr::write(_buffer_index, finish_and_next_stack as u64);

            _buffer_index = stack_buff_ptr.offset(-24) as *mut u64;
            std::ptr::write(_buffer_index, func_return as u64);

            _buffer_index = stack_buff_ptr.offset(-32) as *mut u64;
            std::ptr::write(_buffer_index, trigger_function as u64);

            _buffer_index = stack_buff_ptr.offset(-40) as *mut u64;
            std::ptr::write(_buffer_index, func_return as u64);

            //**************Event Flow***************/
            _buffer_index = stack_buff_ptr.offset(-48) as *mut u64;
            std::ptr::write(_buffer_index, finish_and_next_stack as u64);

            _buffer_index = stack_buff_ptr.offset(-56) as *mut u64;
            std::ptr::write(_buffer_index, func_return as u64);

            _buffer_index = stack_buff_ptr.offset(-64) as *mut u64;
            std::ptr::write(_buffer_index, base_function as u64);

            new_stack.stack_context.rsp = stack_buff_ptr.offset(-64) as *mut u64 as u64;
        }
    }

    #[inline(never)]
    fn terminate_stacks(&mut self) -> bool {
        // make all stacks AVAIABLE
        for stack in &mut self.stack_pool {
            stack.state = State::AVAIABLE;
        }
        // set main stack as READY
        self.stack_pool[PROCESS_MAIN_STACK_ID].state = State::READY;
        return self.switch_stack(ScheduleType::RR);
    }

    #[inline(never)]
    fn switch_stack_to(&mut self, stack_id: usize) -> bool {
        if self.stack_pool[stack_id].state == State::RUNNING {
            eprintln!("Stack:{} already running..", stack_id);
            return false;
        }
        return self.switch_stack(ScheduleType::O1(stack_id));
    }

    #[inline(never)]
    fn trigger_stack_func(&mut self, target_stack_id: usize, event: Event) -> bool {
        unsafe {
            if target_stack_id <= PROCESS_MAIN_STACK_ID || target_stack_id > MAX_STACKS {
                eprintln!("Wrong stack ID!");
                return false;
            }
            let mut target_stack = &mut self.stack_pool[target_stack_id];

            let mut stack_buff_ptr = target_stack
                .stack_buffer
                .as_mut_ptr()
                .offset(target_stack.stack_buffer.len() as isize);

            stack_buff_ptr = (stack_buff_ptr as usize & !15) as *mut u8;

            target_stack.stack_context.rsp =
                stack_buff_ptr.offset(TRIGGER_OFFSET) as *mut u64 as u64;

            self.data_buffer.data_pool.push(event);

            target_stack.stack_context.edi = self.curr_stack_id as u64;
            target_stack.stack_context.esi = self.data_buffer.avaiable_index as u64;

            self.data_buffer.avaiable_index += 1;
        }
        return self.switch_stack_to(target_stack_id);
    }
}

#[naked] // no prolouge & epilouge
#[no_mangle]
unsafe extern "C" fn switch_and_run() {
    asm!(
        "mov [rdi + 0x00], rsp",
        "mov [rdi + 0x08], r15",
        "mov [rdi + 0x10], r14",
        "mov [rdi + 0x18], r13",
        "mov [rdi + 0x20], r12",
        "mov [rdi + 0x28], rbx",
        "mov [rdi + 0x30], rbp",
        "mov [rdi + 0x38], edi",
        "mov [rdi + 0x40], esi",
        "mov rsp, [rsi + 0x00]",
        "mov r15, [rsi + 0x08]",
        "mov r14, [rsi + 0x10]",
        "mov r13, [rsi + 0x18]",
        "mov r12, [rsi + 0x20]",
        "mov rbx, [rsi + 0x28]",
        "mov rbp, [rsi + 0x30]",
        "mov edi, [rsi + 0x38]",
        "mov esi, [rsi + 0x40]",
        "ret",
        options(noreturn)
    );
}

#[naked]
#[no_mangle]
unsafe extern "C" fn func_return() {
    asm!("ret", options(noreturn))
}

#[no_mangle]
fn finish_and_next_stack() {
    unsafe {
        if (*RUNTIME_PTR).curr_stack_id != PROCESS_MAIN_STACK_ID {
            (*RUNTIME_PTR).stack_pool[(*RUNTIME_PTR).curr_stack_id].state = State::AVAIABLE;
            (*RUNTIME_PTR).switch_stack(ScheduleType::RR);
        }
    };
}

pub fn next_stack() {
    unsafe {
        (*RUNTIME_PTR).switch_stack(ScheduleType::RR);
    };
}

pub fn goto_main() {
    unsafe {
        (*RUNTIME_PTR).terminate_stacks();
    };
}

pub fn stack_to(id: usize) {
    unsafe {
        (*RUNTIME_PTR).switch_stack_to(id);
    };
}

pub fn trigger_stack_to(id: usize, event: Event) {
    unsafe {
        (*RUNTIME_PTR).trigger_stack_func(id, event);
    };
}

pub fn get_current_stack_id() -> usize {
    unsafe { (*RUNTIME_PTR).curr_stack_id }
}

pub fn main() {
    let mut runtime = LeydiStacks::new();

    runtime.new_stack(func1, stack1_trigger);
    runtime.new_stack(func2, stack2_trigger);
    runtime.new_stack(func3, stack3_trigger);
    runtime.new_stack(func4, stack4_trigger);

    runtime.run();

    println!("end of main");
}

fn func1() {
    println!("func 1");
}

fn func2() {
    println!("func 2");
}

fn func3() {
    println!("func 3");
}

fn func4() {
    println!("func 4");
}

pub fn stack1_trigger(from_stack_id: usize, _data_buff_index: usize) {
    println!(
        "stack1_trigger called from {} to {}",
        from_stack_id,
        get_current_stack_id()
    );
}

pub fn stack2_trigger(from_stack_id: usize, _data_buff_index: usize) {
    println!(
        "stack2_trigger called from {} to {}",
        from_stack_id,
        get_current_stack_id()
    );
}

pub fn stack3_trigger(from_stack_id: usize, _data_buff_index: usize) {
    println!(
        "stack3_trigger called from {} to {}",
        from_stack_id,
        get_current_stack_id()
    );
}

pub fn stack4_trigger(from_stack_id: usize, _data_buff_index: usize) {
    println!(
        "stack4_trigger called from {} to {}",
        from_stack_id,
        get_current_stack_id()
    );
}
