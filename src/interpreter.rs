// ============================================================================
// INTERPRETER / EVALUATOR
// ============================================================================
//
// The interpreter runs the source directly while parsing it, without building
// a program in memory:
//
// * Blocks are found by indentation. A block that must not run (the false
//   branch of an `if`, a function body at definition time) is still parsed,
//   so syntax errors are reported, but nothing in it is executed.
// * A `while` loop keeps a copy of the parser (a small `Copy` value) taken at
//   its header and jumps back to it after every pass through the body.
// * A `def` stores a parser copy pointing at the function body; a call runs
//   the body from there with its own local variables.
//
// Memory use is fixed: objects created at runtime (strings built by the
// script) live in a fixed-size heap given by the application (`with_heap`),
// with a garbage collector; there is no system allocator. Native recursion
// is bounded by `MAX_NESTING` (nested blocks) and `MAX_CALL_DEPTH` (calls).

#[path = "builtins.rs"]
mod builtins;
#[path = "bytes.rs"]
mod bytes;
#[path = "classes.rs"]
mod classes;
#[path = "containers.rs"]
mod containers;
#[path = "exceptions.rs"]
mod exceptions;
#[path = "files.rs"]
mod files;
#[path = "format.rs"]
mod format;
#[path = "modules.rs"]
mod modules;
#[path = "strings.rs"]
mod strings;

use core::cell::RefCell;
use core::cmp::Ordering as CmpOrdering;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::ast_parser::{
    BinOp, CmpOp, Code, Expr, Item, Parser, Stmt, Targets, MAX_CALL_ARGS, MAX_PARAMS, MAX_TARGETS,
    STAR_ARGS, STAR_KWARGS,
};
use crate::heap::{Heap, Kind, ObjRef};
use crate::{Arg, Error, FileSystem, HardwareBridge, HwFn, Module, NoHardware, Output};

/// Variable slots, shared by globals and the locals of active function calls.
pub const MAX_VARS: usize = 128;
/// Number of `def` functions that can exist at the same time.
pub const MAX_FUNCS: usize = 64;
/// Maximum depth of nested user function calls (recursion included).
pub const MAX_CALL_DEPTH: usize = 8;
/// `global` names declared by all active function calls together.
pub const MAX_GLOBAL_DECLS: usize = 16;
/// Maximum nesting of blocks (`if`/`while`/function bodies) being run at once.
pub const MAX_NESTING: usize = 40;
/// Values in flight: the operands of every expression being evaluated,
/// including those of the calling functions. They are roots for the garbage
/// collector, which is why there is one shared stack instead of one per call.
pub const VALUE_STACK: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value<'a> {
    None,
    Bool(bool),
    Int(i64),
    /// A string in the script source (literals, and slices of them).
    Str(&'a str),
    /// A string created at runtime, on the heap.
    HStr(ObjRef),
    /// A `b'...'` literal in the script source.
    Bytes(&'a [u8]),
    /// Bytes created at runtime, on the heap.
    HBytes(ObjRef),
    /// A list, tuple, dict or range on the heap (see `containers.rs`).
    Obj(ObjRef),
    /// An imported native module (index into `HardwareBridge::modules()`).
    Module(u8),
    /// A function of a native module, e.g. after `from display import line`.
    NativeFn {
        module: u8,
        func: u8,
    },
    /// A built-in exception class, e.g. `ValueError` (see `exceptions.rs`).
    ExcClass(u8),
    /// A `def` function or a lambda (index into the function table).
    Func(u8),
    /// A built-in function or type used as a value (`key=len`, `int`).
    Builtin(u8),
    /// `globals()`: a live view of the global variables.
    Globals,
}

impl Value<'_> {
    /// Integer value of ints and bools (`True` is 1, as in Python).
    fn as_int(self) -> Option<i64> {
        match self {
            Value::Int(n) => Some(n),
            Value::Bool(b) => Some(b as i64),
            _ => None,
        }
    }
}

/// What a statement or block asks its enclosing code to do next.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Flow<'a> {
    Normal,
    Break,
    Continue,
    Return(Value<'a>),
}

/// Where a statement is, for checking `break`/`continue`/`return`/`def`.
#[derive(Debug, Clone, Copy, Default)]
struct Ctx {
    in_loop: bool,
    in_func: bool,
    /// The body of a `class` statement: `def` makes a method.
    in_class: bool,
}

/// A `def` function: its signature and a parser positioned at its body.
#[derive(Debug, Clone, Copy)]
struct FuncDef<'a> {
    /// The name, or for a lambda its source text (which identifies it).
    name: &'a str,
    params: [&'a str; MAX_PARAMS],
    count: usize,
    /// Indentation of the `def` line; the body is indented deeper.
    indent: u32,
    body: Parser<'a>,
    /// A tuple of the default values of the last parameters, or `None`.
    defaults: Value<'a>,
    /// A lambda: `body` is at the start of its text, and the body is an
    /// expression.
    lambda: bool,
    /// A method (or a function made inside a function): a value, not a
    /// global name.
    method: bool,
    /// `STAR_ARGS` / `STAR_KWARGS`: the last parameters collect extra arguments.
    star: u8,
    /// The dict of the `.py` module the function is defined in (its global
    /// names), or `None` for the main script.
    module: Value<'a>,
}

impl FuncDef<'_> {
    /// A `def` function at the top level, found by its name.
    fn named(&self) -> bool {
        !self.lambda && !self.method
    }
}

/// Keyword arguments of a call: `(name, value)` pairs.
/// Keyword arguments of a call: `(name, value)` pairs (the name is a string
/// value: from the source, or from the keys of a `**dict`).
type Kwargs<'k, 'a> = &'k [(Value<'a>, Value<'a>)];

/// The arguments of a call, after `*items` and `**dict` are unpacked.
struct CallArgs<'a> {
    args: [Value<'a>; MAX_CALL_ARGS],
    npos: usize,
    kwargs: [(Value<'a>, Value<'a>); MAX_CALL_ARGS],
    nkw: usize,
}

pub struct Interpreter<'a, O: Output, H: HardwareBridge = NoHardware> {
    /// Top of `stack`.
    sp: usize,
    nvars: usize,
    /// First local of the innermost call, counted from `globals_len`, so new
    /// globals can be inserted below running calls (`global x; x = 1`).
    frame_off: usize,
    /// Number of globals while a function call is active.
    globals_len: usize,
    call_depth: usize,
    nesting: usize,
    global_count: usize,
    frame_globals: usize,
    steps: u32,
    /// Maximum statements one `eval` call may execute (`None` = unlimited).
    step_limit: Option<u32>,
    /// Line of the last error returned by `eval`, if any.
    error_line: Option<u32>,
    /// REPL mode: top-level expression statements print their value.
    interactive: bool,
    out: &'a mut O,
    hw: Option<&'a mut H>,
    /// Set from outside (e.g. a UART Ctrl-C handler) to stop a running script.
    abort: Option<&'a AtomicBool>,
    /// Globals first, then one frame of locals per active function call.
    vars: [(&'a str, Value<'a>); MAX_VARS],
    /// Values in flight (see `VALUE_STACK`).
    stack: [Value<'a>; VALUE_STACK],
    /// Names declared `global` by the active calls; the innermost call's
    /// declarations start at `frame_globals`.
    global_decls: [&'a str; MAX_GLOBAL_DECLS],
    funcs: [Option<FuncDef<'a>>; MAX_FUNCS],
    /// Objects created at runtime (empty unless `with_heap` is used).
    heap: Heap<'a>,
    /// The exception object of the last `raise` (see `Error::Exception`).
    exc: Value<'a>,
    /// The exception the innermost running `except` block handles.
    handling: Value<'a>,
    /// Files for `open()` and the `os` module (see `with_files`).
    files: Option<&'a RefCell<dyn FileSystem + 'a>>,
    /// The class and `self` of the running method, for `super()`.
    method_class: Value<'a>,
    method_self: Value<'a>,
    /// The variables of the running closure (a dict), and of the one
    /// about to be called.
    closure_env: Value<'a>,
    next_env: Value<'a>,
    /// Reachable function slots during a collection (bit per slot).
    func_marks: u64,
    /// Lowest native stack address scripts may reach (`set_stack_floor`).
    stack_floor: usize,
    /// Memory for the source of imported `.py` modules (`with_module_memory`).
    module_memory: Option<&'a mut [u8]>,
    /// `.py` modules built into the application (`with_frozen_modules`).
    frozen: &'a [(&'a str, &'a str)],
    /// Imported `.py` modules by name (a dict), like `sys.modules`.
    py_modules: Value<'a>,
    /// The dict of the module whose code runs (`None`: the main script), and
    /// whether that's its top level (not inside a function or class body).
    module_env: Value<'a>,
    module_top: bool,
}

/// The address of a local variable: how deep the native stack is right now.
#[inline(always)]
fn stack_address() -> usize {
    let marker = 0u8;
    core::hint::black_box(&marker) as *const u8 as usize
}

impl<'a, O: Output> Interpreter<'a, O, NoHardware> {
    pub fn new(out: &'a mut O) -> Self {
        Self::from_parts(out, None)
    }
}

impl<'a, O: Output, H: HardwareBridge> Interpreter<'a, O, H> {
    pub fn with_hardware(out: &'a mut O, hw: &'a mut H) -> Self {
        Self::from_parts(out, Some(hw))
    }

    fn from_parts(out: &'a mut O, hw: Option<&'a mut H>) -> Self {
        Self {
            out,
            hw,
            vars: [("", Value::None); MAX_VARS],
            nvars: 0,
            frame_off: 0,
            global_decls: [""; MAX_GLOBAL_DECLS],
            global_count: 0,
            frame_globals: 0,
            globals_len: 0,
            call_depth: 0,
            nesting: 0,
            funcs: [None; MAX_FUNCS],
            error_line: None,
            step_limit: None,
            steps: 0,
            abort: None,
            interactive: false,
            heap: Heap::new(&mut []),
            stack: [Value::None; VALUE_STACK],
            sp: 0,
            exc: Value::None,
            handling: Value::None,
            files: None,
            method_class: Value::None,
            method_self: Value::None,
            closure_env: Value::None,
            next_env: Value::None,
            func_marks: 0,
            stack_floor: 0,
            module_memory: None,
            frozen: &[],
            py_modules: Value::None,
            module_env: Value::None,
            module_top: false,
        }
    }

    /// Gives the interpreter a heap for objects created at runtime, such as
    /// strings built with `+`, `str()` or escape sequences. Without one, those
    /// operations raise `Error::OutOfMemory`.
    pub fn with_heap(mut self, memory: &'a mut [u8]) -> Self {
        self.set_heap(memory);
        self
    }

    /// Like `with_heap`, on an interpreter that already exists. (The `with_`
    /// builders move the interpreter, which can leave copies of it in the
    /// caller's stack frame; the `set_` methods change it in place.)
    pub fn set_heap(&mut self, memory: &'a mut [u8]) {
        self.heap = Heap::new(memory);
    }

    /// Gives scripts a file system: `open()` and the `os` module. The
    /// application can use it too between `eval` calls (a `RefCell`, so it's
    /// borrowed only while the interpreter works with files).
    pub fn with_files(mut self, files: &'a RefCell<dyn FileSystem + 'a>) -> Self {
        self.files = Some(files);
        self
    }

    /// Memory for the source of `.py` modules that scripts import from the
    /// file system (`import sensors` reads `sensors.py`). Each module stays
    /// there until the interpreter is dropped; without it, only native
    /// modules can be imported.
    pub fn with_module_memory(mut self, memory: &'a mut [u8]) -> Self {
        self.module_memory = Some(memory);
        self
    }

    /// Python modules built into the application, as `(name, source)` pairs
    /// (e.g. `("machine", include_str!("machine.py"))`): `import name` runs the
    /// source like a `name.py` file, but without reading it into the module
    /// memory. Like MicroPython's frozen modules, a `name.py` on the file
    /// system comes first.
    pub fn with_frozen_modules(mut self, modules: &'a [(&'a str, &'a str)]) -> Self {
        self.frozen = modules;
        self
    }

    /// Like `with_frozen_modules`, on an interpreter that already exists.
    pub fn set_frozen_modules(&mut self, modules: &'a [(&'a str, &'a str)]) {
        self.frozen = modules;
    }

    /// Like `with_module_memory`, on an interpreter that already exists.
    pub fn set_module_memory(&mut self, memory: &'a mut [u8]) {
        self.module_memory = Some(memory);
    }

    /// Like `with_files`, on an interpreter that already exists.
    pub fn set_files(&mut self, files: &'a RefCell<dyn FileSystem + 'a>) {
        self.files = Some(files);
    }

    /// The lowest native stack address scripts may use (the stack grows
    /// down): deeper calls, blocks or expressions raise `RecursionError`
    /// (`Error::RecursionLimit`) instead of overflowing the stack, which on a
    /// microcontroller crashes the device. Give it the bottom of the stack
    /// plus a margin for the code the interpreter calls (hardware drivers,
    /// interrupts). `None`: no check.
    pub fn set_stack_floor(&mut self, address: Option<usize>) {
        self.stack_floor = address.unwrap_or(0);
    }

    /// `Err(RecursionLimit)` if the native stack went below the floor.
    #[inline(always)]
    fn check_stack(&self) -> Result<(), Error> {
        if stack_address() < self.stack_floor {
            return Err(Error::RecursionLimit);
        }
        Ok(())
    }

    /// `(used bytes, capacity, live objects, collections)` of the heap.
    pub fn heap_stats(&self) -> (usize, usize, usize, u32) {
        (
            self.heap.used(),
            self.heap.capacity(),
            self.heap.objects(),
            self.heap.collections,
        )
    }

    /// Python truth value: `None`, `False`, `0` and `''` are false.
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn truthy(&self, v: Value<'a>) -> bool {
        match v {
            Value::None => false,
            Value::Bool(b) => b,
            Value::Int(n) => n != 0,
            Value::Str(s) => !s.is_empty(),
            Value::HStr(r) => !self.heap.str(r).is_empty(),
            Value::Bytes(b) => !b.is_empty(),
            Value::HBytes(r) => !self.heap.bytes(r).is_empty(),
            Value::Obj(_) => self.container_len(v) != Some(0),
            Value::Module(_)
            | Value::NativeFn { .. }
            | Value::ExcClass(_)
            | Value::Func(_)
            | Value::Builtin(_)
            | Value::Globals => true,
        }
    }

    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn push(&mut self, v: Value<'a>) -> Result<(), Error> {
        if self.sp >= VALUE_STACK {
            return Err(Error::OutOfMemory);
        }
        self.stack[self.sp] = v;
        self.sp += 1;
        Ok(())
    }

    /// Limits how many statements (and loop iterations) a single `eval` call
    /// may execute. When the limit is hit, `eval` returns `Error::StepLimit`.
    /// Protects the device from scripts that never finish.
    pub fn set_step_limit(&mut self, limit: Option<u32>) {
        self.step_limit = limit;
    }

    /// Registers a flag that stops the running script when set to `true`
    /// (checked before every statement and loop iteration), making `eval`
    /// return `Error::Interrupted`. The flag is only read, so the owner
    /// clears it before the next run.
    pub fn set_abort_flag(&mut self, flag: &'a AtomicBool) {
        self.abort = Some(flag);
    }

    /// The `Output` the interpreter prints to, e.g. for a REPL to write its
    /// prompt and echo to the same place as `print()`.
    pub fn output(&mut self) -> &mut O {
        self.out
    }

    /// REPL mode, like Python's interactive prompt: an expression statement at
    /// the top level prints its value (`1 + 2` prints `3`, `'a'` prints `'a'`),
    /// unless it is `None`. Calls such as `print(x)` return `None`, so they
    /// don't print twice.
    pub fn set_interactive(&mut self, on: bool) {
        self.interactive = on;
    }

    /// 1-based line where the error returned by the last `eval` call happened,
    /// or `None` if it succeeded.
    pub const fn error_line(&self) -> Option<u32> {
        self.error_line
    }

    /// Runs a script. Variables and functions stay defined for later calls.
    pub fn eval(&mut self, source: &'a str) -> Result<(), Error> {
        self.error_line = None;
        self.steps = 0;
        self.sp = 0;
        self.handling = Value::None;
        let result = self.run(source);
        match result {
            Ok(()) => self.error_line = None,
            Err(_) => self.note_error(1), // only if no line was recorded
        }
        // Calls and blocks always unwind their own state, even on errors
        debug_assert!(self.call_depth == 0 && self.nesting == 0);
        result
    }

    fn run(&mut self, source: &'a str) -> Result<(), Error> {
        let mut parser = Parser::new(source)?;
        let mut code = Code::new();
        self.exec_block(&mut parser, 0, true, Ctx::default(), &mut code)
            .map(|_| ())
    }

    /// Records the line of an error. The innermost (first) record wins, so
    /// an error inside a block or function points at the failing statement.
    fn note_error(&mut self, line: u32) {
        if self.error_line.is_none() {
            self.error_line = Some(line);
        }
    }

    /// Counts one step and checks the abort flag and the step limit.
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn tick(&mut self) -> Result<(), Error> {
        if self.abort.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return Err(Error::Interrupted);
        }
        self.steps = self.steps.saturating_add(1);
        if self.step_limit.is_some_and(|limit| self.steps > limit) {
            return Err(Error::StepLimit);
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Statements and blocks
    // ------------------------------------------------------------------

    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn parse_stmt(
        &mut self,
        parser: &mut Parser<'a>,
        code: &mut Code<'a>,
    ) -> Result<Option<Stmt<'a>>, Error> {
        let result = parser.parse_next_stmt(code);
        if result.is_err() {
            self.note_error(parser.line());
        }
        result
    }

    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn next_indent(&mut self, parser: &mut Parser<'a>) -> Result<Option<u32>, Error> {
        let result = parser.next_indent();
        if result.is_err() {
            self.note_error(parser.line());
        }
        result
    }

    /// Runs (or, with `run == false`, only parses) the statements of a block
    /// whose statements are indented by exactly `indent` columns.
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn exec_block(
        &mut self,
        parser: &mut Parser<'a>,
        indent: u32,
        run: bool,
        ctx: Ctx,
        code: &mut Code<'a>,
    ) -> Result<Flow<'a>, Error> {
        let mut flow = Flow::Normal;
        loop {
            match self.next_indent(parser)? {
                None => return Ok(flow),
                Some(i) if i < indent => return Ok(flow),
                Some(i) if i > indent => {
                    self.note_error(parser.line());
                    return Err(Error::IndentationError);
                }
                Some(_) => {}
            }
            // After `break`/`continue`/`return`, the rest of the block is
            // only parsed, so the parser ends up after the block.
            let active = run && flow == Flow::Normal;
            let result = self.exec_stmt(parser, indent, active, ctx, code, false)?;
            if active {
                flow = result;
            }
        }
    }

    /// Runs the body after a `header:` line: an indented block on the next
    /// lines, or a single simple statement on the same line (`if x: y = 1`).
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn exec_body(
        &mut self,
        parser: &mut Parser<'a>,
        header_indent: u32,
        run: bool,
        ctx: Ctx,
        code: &mut Code<'a>,
    ) -> Result<Flow<'a>, Error> {
        if self.nesting >= MAX_NESTING {
            // Too deep (through calls, e.g. recursion): like Python, a RecursionError
            self.note_error(parser.line());
            return Err(Error::RecursionLimit);
        }
        if let Err(err) = self.check_stack() {
            self.note_error(parser.line());
            return Err(err);
        }
        self.nesting += 1;

        let result = if parser.at_line_end() {
            match self.next_indent(parser) {
                Ok(Some(i)) if i > header_indent => self.exec_block(parser, i, run, ctx, code),
                Ok(_) => {
                    // `expected an indented block`
                    self.note_error(parser.line());
                    Err(Error::IndentationError)
                }
                Err(e) => Err(e),
            }
        } else {
            self.exec_inline_body(parser, header_indent, run, ctx, code)
        };

        self.nesting -= 1;
        result
    }

    /// A body on the header's line: one or more simple statements joined by
    /// `;` (`if x: a = 1; b = 2`).
    fn exec_inline_body(
        &mut self,
        parser: &mut Parser<'a>,
        header_indent: u32,
        run: bool,
        ctx: Ctx,
        code: &mut Code<'a>,
    ) -> Result<Flow<'a>, Error> {
        let mut flow = Flow::Normal;
        loop {
            let active = run && flow == Flow::Normal;
            let result = self.exec_stmt(parser, header_indent, active, ctx, code, true)?;
            if active {
                flow = result;
            }
            if !parser.after_semicolon() || parser.at_line_end() {
                return Ok(flow);
            }
        }
    }

    /// Parses one statement and runs it if `run` is true. `inline` is set for
    /// a body on the same line as its header, which can't be compound.
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn exec_stmt(
        &mut self,
        parser: &mut Parser<'a>,
        indent: u32,
        run: bool,
        ctx: Ctx,
        code: &mut Code<'a>,
        inline: bool,
    ) -> Result<Flow<'a>, Error> {
        let start = *parser;
        let Some(stmt) = self.parse_stmt(parser, code)? else {
            return Ok(Flow::Normal);
        };
        let line = parser.stmt_line();

        let result = if inline && stmt.is_compound() {
            Err(Error::InvalidSyntax)
        } else if run {
            self.tick()
                .and_then(|()| self.exec_parsed(stmt, start, parser, indent, true, ctx, code))
        } else {
            self.exec_parsed(stmt, start, parser, indent, false, ctx, code)
        };
        if result.is_err() {
            self.note_error(line);
        }
        result
    }

    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    #[allow(clippy::too_many_arguments)]
    fn exec_parsed(
        &mut self,
        stmt: Stmt<'a>,
        start: Parser<'a>,
        parser: &mut Parser<'a>,
        indent: u32,
        run: bool,
        ctx: Ctx,
        code: &mut Code<'a>,
    ) -> Result<Flow<'a>, Error> {
        match stmt {
            Stmt::Assign { name, expr } => {
                if run {
                    let val = self.eval_expr(code, expr)?;
                    self.set_var(name, val)?;
                }
            }
            Stmt::AssignMulti { targets, expr } => {
                if run {
                    let val = self.eval_expr(code, expr)?;
                    if targets.unpack {
                        self.assign_targets(&targets, val)?;
                    } else {
                        for &name in targets.names() {
                            self.set_var(name, val)?;
                        }
                    }
                }
            }
            Stmt::SetItem { target, value } => {
                if run {
                    let base = self.sp;
                    let result = self.exec_set_item(code, target, None, value);
                    self.sp = base;
                    result?;
                }
            }
            Stmt::AugSetItem { target, op, value } => {
                if run {
                    let base = self.sp;
                    let result = self.exec_set_item(code, target, Some(op), value);
                    self.sp = base;
                    result?;
                }
            }
            Stmt::DelNames(targets) => {
                if run {
                    for &name in targets.names() {
                        self.del_var(name)?;
                    }
                }
            }
            Stmt::DelItem(target) => {
                if run {
                    let base = self.sp;
                    let result = self.run_items(code.items(target)).and_then(|()| {
                        if self.sp != base + 2 {
                            return Err(Error::InvalidSyntax);
                        }
                        self.del_item(self.stack[base], self.stack[base + 1])
                    });
                    self.sp = base;
                    result?;
                }
            }
            Stmt::For { targets, iter } => {
                return self.exec_for(targets, iter, parser, indent, run, ctx, code)
            }
            Stmt::AugAssign { name, op, expr } => {
                if run {
                    // Like Python, `x += 1` needs `x` in the current scope
                    // (a global can't be updated from inside a function).
                    let current = self.get_scope_var(name)?;
                    if op == BinOp::Add && self.obj_kind(current) == Some(Kind::List) {
                        // `list += iterable` extends the list in place
                        let base = self.sp;
                        let result = self.push(current).and_then(|()| {
                            let rhs = self.eval_expr(code, expr)?;
                            self.push(rhs)?;
                            self.call_method(current, "extend", &[rhs])
                        });
                        self.sp = base;
                        result?;
                        return Ok(Flow::Normal);
                    }
                    // Both operands stay on the value stack while `op` may allocate
                    let base = self.sp;
                    let result = self.push(current).and_then(|()| {
                        let rhs = self.eval_expr(code, expr)?;
                        self.push(rhs)?;
                        self.apply_bin(op)?;
                        Ok(self.stack[self.sp - 1])
                    });
                    self.sp = base;
                    self.set_var(name, result?)?;
                }
            }
            Stmt::Expr(expr) => {
                if run {
                    let val = self.eval_expr(code, expr)?;
                    if self.interactive && self.call_depth == 0 && val != Value::None {
                        self.write_value(val, true);
                        self.out.write_str("\n");
                    }
                }
            }
            Stmt::Pass => {}
            Stmt::Break | Stmt::Continue => {
                if !ctx.in_loop {
                    return Err(Error::InvalidSyntax);
                }
                if run {
                    return Ok(if stmt == Stmt::Break {
                        Flow::Break
                    } else {
                        Flow::Continue
                    });
                }
            }
            Stmt::Return(expr) => {
                if !ctx.in_func {
                    return Err(Error::InvalidSyntax);
                }
                if run {
                    let val = match expr {
                        Some(expr) => self.eval_expr(code, expr)?,
                        None => Value::None,
                    };
                    return Ok(Flow::Return(val));
                }
            }
            // `elif`/`else` without a matching `if`
            Stmt::Elif(_) | Stmt::Else | Stmt::Except { .. } | Stmt::Finally => {
                return Err(Error::InvalidSyntax)
            }
            Stmt::Try => return self.exec_try(parser, indent, run, ctx, code),
            Stmt::With { expr, name } => {
                return self.exec_with(expr, name, parser, indent, run, ctx, code)
            }
            Stmt::Raise(expr) => {
                if run {
                    return Err(match expr {
                        Some(expr) => {
                            let val = self.eval_expr(code, expr)?;
                            self.raise_value(val)
                        }
                        None => self.reraise(),
                    });
                }
            }
            Stmt::Assert { cond, message } => {
                if run {
                    let val = self.eval_expr(code, cond)?;
                    if !self.truthy(val) {
                        let message = match message {
                            Some(m) => Some(self.eval_expr(code, m)?),
                            None => None,
                        };
                        let base = self.sp;
                        if let Some(m) = message {
                            self.push(m)?;
                        }
                        let err = self.raise_assertion(message);
                        self.sp = base;
                        return Err(err);
                    }
                }
            }
            Stmt::If(cond) => return self.exec_if(cond, parser, indent, run, ctx, code),
            Stmt::While(cond) => {
                return self.exec_while(cond, start, parser, indent, run, ctx, code)
            }
            Stmt::Global { names, count } => {
                if run {
                    self.declare_globals(&names[..count])?;
                }
            }
            Stmt::Import { names, count } => {
                if run {
                    for &(module, bound) in &names[..count] {
                        // A native module, or `module.py` from the file system
                        let value = match self.find_module(module) {
                            Some(index) => Value::Module(index),
                            None => self.import_py(module)?,
                        };
                        self.set_var(bound, value)?;
                    }
                }
            }
            Stmt::FromImport {
                module,
                names,
                count,
            } => {
                if run {
                    match self.find_module(module) {
                        Some(index) => {
                            for &(name, bound) in &names[..count] {
                                let val = self
                                    .module_attr(index, name)
                                    .map_err(|_| Error::ImportError)?;
                                self.set_var(bound, val)?;
                            }
                        }
                        None => {
                            let py = self.import_py(module)?;
                            let dict = self.module_dict(py).ok_or(Error::ImportError)?;
                            for &(name, bound) in &names[..count] {
                                let val =
                                    self.dict_get_str(dict, name).ok_or(Error::ImportError)?;
                                self.set_var(bound, val)?;
                            }
                        }
                    }
                }
            }
            Stmt::Def {
                name,
                params,
                count,
                defaults,
                star,
            } => {
                // Default values are evaluated once, when `def` runs (like Python)
                let defaults = match (run, defaults) {
                    (true, Some(expr)) => self.eval_expr(code, expr)?,
                    _ => Value::None,
                };
                let func = FuncDef {
                    name,
                    params,
                    count,
                    indent,
                    body: *parser,
                    defaults,
                    lambda: false,
                    method: ctx.in_class || ctx.in_func || self.module_top,
                    star,
                    module: self.module_env,
                };
                // Parse (but don't run) the body to find where it ends
                let body_ctx = Ctx {
                    in_loop: false,
                    in_func: true,
                    in_class: false,
                };
                self.exec_body(parser, indent, false, body_ctx, code)?;
                if run && (ctx.in_class || self.module_top) {
                    // A method: a local of the class body (it goes into the class dict)
                    let slot = self.method_slot(func)?;
                    self.set_var(name, Value::Func(slot))?;
                } else if run && ctx.in_func {
                    // A function inside a function: a closure, as a local
                    let slot = self.method_slot(func)?;
                    let closure = self.make_closure(Value::Func(slot))?;
                    self.set_var(name, closure)?;
                } else if run {
                    self.define_func(func)?;
                }
            }
            Stmt::Class { name, base } => {
                return self.exec_class(name, base, parser, indent, run, ctx, code)
            }
            Stmt::Decorator(expr) => {
                return self.exec_decorated(expr, parser, indent, run, ctx, code)
            }
            Stmt::SetAttr {
                target,
                attr,
                value,
            } => {
                if run {
                    let base = self.sp;
                    let result = (|| {
                        let val = self.eval_expr(code, value)?;
                        self.push(val)?;
                        let obj = self.eval_expr(code, target)?;
                        self.push(obj)?;
                        self.set_attr(obj, attr, val)
                    })();
                    self.sp = base;
                    result?;
                }
            }
            Stmt::AugSetAttr {
                target,
                attr,
                op,
                value,
            } => {
                if run {
                    let base = self.sp;
                    let result = (|| {
                        let obj = self.eval_expr(code, target)?;
                        self.push(obj)?;
                        let current = self.get_attr(obj, attr)?.ok_or(Error::AttributeError)?;
                        self.push(current)?;
                        let rhs = self.eval_expr(code, value)?;
                        self.push(rhs)?;
                        self.apply_bin(op)?;
                        let new = self.stack[self.sp - 1];
                        self.set_attr(obj, attr, new)
                    })();
                    self.sp = base;
                    result?;
                }
            }
        }
        Ok(Flow::Normal)
    }

    /// `class Name(Base):` runs its body like a function call; the body's
    /// variables (attributes, methods) become the class dict.
    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn exec_class(
        &mut self,
        name: &'a str,
        base: Option<Expr>,
        parser: &mut Parser<'a>,
        indent: u32,
        run: bool,
        ctx: Ctx,
        code: &mut Code<'a>,
    ) -> Result<Flow<'a>, Error> {
        let class_ctx = Ctx {
            in_loop: false,
            in_func: false,
            in_class: true,
        };
        if ctx.in_func || ctx.in_class {
            return Err(Error::InvalidSyntax); // classes are defined at the top level
        }
        if !run {
            self.exec_body(parser, indent, false, class_ctx, code)?;
            return Ok(Flow::Normal);
        }
        let base_value = match base {
            Some(expr) => self.eval_expr(code, expr)?,
            None => Value::None,
        };
        let base_value = match base_value {
            // `class C(object)` is `class C`
            Value::Builtin(b) if builtins::builtin_name(b) == "object" => Value::None,
            v @ (Value::None | Value::ExcClass(_)) => v,
            Value::Obj(r) if self.heap.kind(r) == Kind::Class => base_value,
            _ => return Err(Error::TypeError), // other bases aren't supported
        };
        if self.call_depth >= MAX_CALL_DEPTH {
            return Err(Error::RecursionLimit);
        }
        let stack_base = self.sp;
        let dict = self.new_container(Kind::Dict, 0)?;
        self.push(Value::Obj(dict))?;
        for v in [Value::Str(name), base_value, Value::Obj(dict)] {
            self.push(v)?;
        }
        self.build(Kind::Tuple, 3)?;
        let class = self.stack[self.sp - 1];
        if let Value::Obj(r) = class {
            self.heap.set_kind(r, Kind::Class);
        }

        // The body runs in a frame of its own, like a call
        let saved_off = self.frame_off;
        let saved_globals = (self.frame_globals, self.global_count);
        if self.call_depth == 0 {
            self.globals_len = self.nvars;
        }
        let base_off = self.nvars - self.globals_len;
        self.frame_off = base_off;
        self.frame_globals = self.global_count;
        self.call_depth += 1;
        let was_top = core::mem::replace(&mut self.module_top, false);
        let body = self.exec_body(parser, indent, true, class_ctx, code);
        self.module_top = was_top;
        let first = self.globals_len + base_off;
        let mut moved = Ok(());
        for i in first..self.nvars {
            let (attr, v) = self.vars[i];
            moved = moved.and_then(|()| self.dict_set(dict, Value::Str(attr), v));
        }
        self.call_depth -= 1;
        self.frame_off = saved_off;
        (self.frame_globals, self.global_count) = saved_globals;
        self.nvars = self.globals_len + base_off;

        let result = body.and(moved).and_then(|_| self.set_var(name, class));
        self.sp = stack_base;
        result.map(|()| Flow::Normal)
    }

    /// `@decorator` before a `def` or `class` (or another decorator): runs
    /// the definition, then replaces it with `decorator(definition)`.
    #[inline(never)]
    fn exec_decorated(
        &mut self,
        expr: Expr,
        parser: &mut Parser<'a>,
        indent: u32,
        run: bool,
        ctx: Ctx,
        code: &mut Code<'a>,
    ) -> Result<Flow<'a>, Error> {
        // The name being defined: skip over further decorators to the header
        let mut peek = *parser;
        let mut scratch = Code::new();
        let name = loop {
            if peek.next_indent()? != Some(indent) {
                return Err(Error::InvalidSyntax);
            }
            match peek.parse_next_stmt(&mut scratch)? {
                Some(Stmt::Decorator(_)) => {}
                Some(Stmt::Def { name, .. } | Stmt::Class { name, .. }) => break name,
                _ => return Err(Error::InvalidSyntax), // a decorator must be followed by def/class
            }
        };
        let decorator = if run {
            self.eval_expr(code, expr)?
        } else {
            Value::None
        };
        let base = self.sp;
        self.push(decorator)?;
        if self.next_indent(parser)? != Some(indent) {
            self.sp = base;
            return Err(Error::InvalidSyntax);
        }
        let result = self
            .exec_stmt(parser, indent, run, ctx, code, false)
            .and_then(|_| {
                if !run {
                    return Ok(());
                }
                let defined = self.get_var(name)?;
                self.push(defined)?;
                let decorated = self.call_value(decorator, &[defined], &[])?;
                self.set_var(name, decorated)
            });
        self.sp = base;
        result.map(|()| Flow::Normal)
    }

    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    #[inline(never)]
    fn exec_if(
        &mut self,
        cond: Expr,
        parser: &mut Parser<'a>,
        indent: u32,
        run: bool,
        ctx: Ctx,
        code: &mut Code<'a>,
    ) -> Result<Flow<'a>, Error> {
        // `taken`: one of the branches has run, so the rest are only parsed
        let mut taken = run && {
            let val = self.eval_expr(code, cond)?;
            self.truthy(val)
        };
        let mut flow = self.exec_body(parser, indent, taken, ctx, code)?;

        while self.next_indent(parser)? == Some(indent) {
            if parser.at_elif() {
                let Some(Stmt::Elif(cond)) = self.parse_stmt(parser, code)? else {
                    unreachable!("at_elif() guarantees an elif header");
                };
                let line = parser.stmt_line();
                let this = run && !taken && {
                    let val = self.eval_expr(code, cond);
                    if val.is_err() {
                        self.note_error(line);
                    }
                    let val = val?;
                    self.truthy(val)
                };
                let result = self.exec_body(parser, indent, this, ctx, code)?;
                if this {
                    flow = result;
                    taken = true;
                }
            } else if parser.at_else() {
                self.parse_stmt(parser, code)?;
                let this = run && !taken;
                let result = self.exec_body(parser, indent, this, ctx, code)?;
                if this {
                    flow = result;
                }
                break;
            } else {
                break;
            }
        }
        Ok(flow)
    }

    /// `with open(...) as f:`: the file is closed when the body ends, also
    /// after an error or a `return`.
    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn exec_with(
        &mut self,
        expr: Expr,
        name: Option<&'a str>,
        parser: &mut Parser<'a>,
        indent: u32,
        run: bool,
        ctx: Ctx,
        code: &mut Code<'a>,
    ) -> Result<Flow<'a>, Error> {
        if !run {
            return self.exec_body(parser, indent, false, ctx, code);
        }
        let manager = self.eval_expr(code, expr)?;
        let base = self.sp;
        self.push(manager)?;
        // A file, or an object with `__enter__` / `__exit__`
        let val = match self.obj_kind(manager) {
            Some(Kind::File) => manager,
            Some(Kind::Instance) => match self.call_dunder(manager, "__enter__", &[]) {
                Some(entered) => entered?,
                None => return Err(Error::TypeError),
            },
            _ => return Err(Error::TypeError),
        };
        self.push(val)?;
        if let Some(name) = name {
            self.set_var(name, val)?;
        }
        let outcome = self.run_clause(parser, indent, true, ctx, code);
        let line = self.error_line.take();
        // Keep a pending exception (and a return value) while closing
        let (pending_exc, flow_val) = (
            self.exc,
            match outcome {
                Ok(Ok(Flow::Return(v))) => v,
                _ => Value::None,
            },
        );
        let close = self
            .push(pending_exc)
            .and_then(|()| self.push(flow_val))
            .and_then(|()| match self.obj_kind(manager) {
                Some(Kind::Instance) => {
                    let none = [Value::None; 3];
                    self.call_dunder(manager, "__exit__", &none)
                        .unwrap_or(Ok(Value::None))
                        .map(|_| ())
                }
                _ => self.close_if_file(val),
            });
        self.sp = base;
        self.exc = pending_exc;
        let outcome = outcome?;
        if outcome.is_err() {
            self.error_line = line;
        }
        close?;
        outcome
    }

    /// `try:` and its `except` / `else` / `finally` clauses.
    #[inline(never)]
    fn exec_try(
        &mut self,
        parser: &mut Parser<'a>,
        indent: u32,
        run: bool,
        ctx: Ctx,
        code: &mut Code<'a>,
    ) -> Result<Flow<'a>, Error> {
        let base = self.sp;
        let result = self.run_try(parser, indent, run, ctx, code);
        self.sp = base;
        let (outcome, line) = result?;
        if outcome.is_err() {
            // Report the line where the exception happened, not the `try`
            self.error_line = line;
        }
        outcome
    }

    /// The clauses of a `try`. The outer `Result` is for errors that end the
    /// script (syntax errors); the inner one is the outcome of the statement,
    /// with the line of its error.
    #[inline(never)]
    #[allow(clippy::type_complexity)]
    fn run_try(
        &mut self,
        parser: &mut Parser<'a>,
        indent: u32,
        run: bool,
        ctx: Ctx,
        code: &mut Code<'a>,
    ) -> Result<(Result<Flow<'a>, Error>, Option<u32>), Error> {
        let mut outcome = self.run_clause(parser, indent, run, ctx, code)?;
        let mut line = self.error_line.take();
        let body_ok = outcome.is_ok();
        let mut clauses = 0;
        let mut seen_else = false;
        // After one handler ran, an error it raised goes to the caller,
        // not to the next `except` clause
        let mut handled = false;

        while self.next_indent(parser)? == Some(indent) {
            if parser.at_except() && !seen_else {
                let Some(Stmt::Except { classes, name }) = self.parse_stmt(parser, code)? else {
                    unreachable!("at_except() guarantees an except header");
                };
                let header_line = parser.stmt_line();
                clauses += 1;
                // Does this clause handle the pending exception?
                let mut this = false;
                if let (true, false, Err(err)) = (run, handled, outcome) {
                    if let Some(class) = self.caught_class(err) {
                        this = match classes {
                            None => Self::bare_except_catches(class),
                            Some(spec) => match self.eval_expr(code, spec) {
                                Ok(spec) => match self.exc_matches(class, spec) {
                                    Ok(m) => m,
                                    Err(e) => {
                                        outcome = Err(e);
                                        line = Some(header_line);
                                        false
                                    }
                                },
                                Err(e) => {
                                    outcome = Err(e);
                                    line = Some(header_line);
                                    false
                                }
                            },
                        };
                        if this {
                            handled = true;
                            let exc = self.exception_object(err)?;
                            self.push(exc)?; // rooted while the handler runs
                            self.exc = Value::None;
                            if let Some(name) = name {
                                self.set_var(name, exc)?;
                            }
                            let outer = core::mem::replace(&mut self.handling, exc);
                            outcome = self.run_clause(parser, indent, true, ctx, code)?;
                            line = self.error_line.take();
                            self.handling = outer;
                            if let Some(name) = name {
                                // Like Python: the name is deleted after the handler
                                let _ = self.del_var(name);
                            }
                        }
                    }
                }
                if !this {
                    self.exec_body(parser, indent, false, ctx, code)?;
                }
            } else if parser.at_else() && clauses > 0 && !seen_else {
                self.parse_stmt(parser, code)?;
                seen_else = true;
                let this = run && body_ok && outcome == Ok(Flow::Normal);
                if this {
                    outcome = self.run_clause(parser, indent, true, ctx, code)?;
                    line = self.error_line.take();
                } else {
                    self.exec_body(parser, indent, false, ctx, code)?;
                }
            } else if parser.at_finally() {
                self.parse_stmt(parser, code)?;
                clauses += 1;
                if run {
                    // Keep what's pending rooted while `finally` runs
                    let (pending_exc, pending_line) = (self.exc, line);
                    self.push(pending_exc)?;
                    if let Ok(Flow::Return(val)) = outcome {
                        self.push(val)?;
                    }
                    let result = self.run_clause(parser, indent, true, ctx, code)?;
                    if result == Ok(Flow::Normal) {
                        // `finally` finished: the earlier outcome stands
                        self.exc = pending_exc;
                        line = pending_line;
                    } else {
                        outcome = result;
                        line = self.error_line.take();
                    }
                } else {
                    self.exec_body(parser, indent, false, ctx, code)?;
                }
                break;
            } else {
                break;
            }
        }
        if clauses == 0 {
            // `try:` needs at least one `except` or `finally`
            self.note_error(parser.line());
            return Err(Error::InvalidSyntax);
        }
        Ok((outcome, line))
    }

    /// Runs the body of a `try` clause. If it raises a catchable error, the
    /// rest of the body is skipped (parsed from its start without running),
    /// so the parser is after the body either way.
    fn run_clause(
        &mut self,
        parser: &mut Parser<'a>,
        indent: u32,
        run: bool,
        ctx: Ctx,
        code: &mut Code<'a>,
    ) -> Result<Result<Flow<'a>, Error>, Error> {
        let start = *parser;
        let sp = self.sp;
        match self.exec_body(parser, indent, run, ctx, code) {
            Ok(flow) => Ok(Ok(flow)),
            Err(err) if self.caught_class(err).is_some() => {
                self.sp = sp;
                let line = self.error_line.take();
                *parser = start;
                self.exec_body(parser, indent, false, ctx, code)?;
                self.error_line = line;
                Ok(Err(err))
            }
            Err(err) => Err(err),
        }
    }

    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn exec_while(
        &mut self,
        mut cond: Expr,
        start: Parser<'a>,
        parser: &mut Parser<'a>,
        indent: u32,
        run: bool,
        ctx: Ctx,
        code: &mut Code<'a>,
    ) -> Result<Flow<'a>, Error> {
        let loop_ctx = Ctx {
            in_loop: true,
            ..ctx
        };
        loop {
            let go = run && {
                let val = self.eval_expr(code, cond)?;
                self.truthy(val)
            };
            let flow = self.exec_body(parser, indent, go, loop_ctx, code)?;
            if !go {
                // The body was only parsed: the parser is now after the loop
                return Ok(Flow::Normal);
            }
            match flow {
                Flow::Break => return Ok(Flow::Normal),
                Flow::Return(val) => return Ok(Flow::Return(val)),
                Flow::Normal | Flow::Continue => {}
            }

            // Jump back to the `while` header and evaluate the condition again
            self.tick()?;
            *parser = start;
            match self.parse_stmt(parser, code)? {
                Some(Stmt::While(next)) => cond = next,
                _ => unreachable!("re-parsing a while header"),
            }
        }
    }

    /// `x[key] = value` or `x[key] op= value`.
    #[inline(never)]
    fn exec_set_item(
        &mut self,
        code: &Code<'a>,
        target: Expr,
        op: Option<BinOp>,
        value: Expr,
    ) -> Result<(), Error> {
        let base = self.sp;
        // Python evaluates the value first, then the container and the key
        let val = self.eval_expr(code, value)?;
        self.push(val)?;
        self.run_items(code.items(target))?;
        if self.sp != base + 3 {
            return Err(Error::InvalidSyntax);
        }
        let (container, key) = (self.stack[base + 1], self.stack[base + 2]);
        let val = match op {
            None => val,
            Some(op) => {
                let current = self.index_value(container, key)?;
                self.push(current)?;
                self.push(val)?;
                self.apply_bin(op)?;
                self.stack[self.sp - 1]
            }
        };
        self.set_item(container, key, val)
    }

    /// Assigns `val` (rooted while it is unpacked) to the names of `targets`.
    fn assign_targets(&mut self, targets: &Targets<'a>, val: Value<'a>) -> Result<(), Error> {
        if !targets.unpack {
            return self.set_var(targets.names[0], val);
        }
        let base = self.sp;
        let mut parts = [Value::None; MAX_TARGETS];
        let result = self.push(val).and_then(|()| {
            self.unpack(val, &mut parts[..targets.count])?;
            // The parts stay rooted until all names are set
            for &p in &parts[..targets.count] {
                self.push(p)?;
            }
            for (&name, &p) in targets.names().iter().zip(&parts) {
                self.set_var(name, p)?;
            }
            Ok(())
        });
        self.sp = base;
        result
    }

    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn exec_for(
        &mut self,
        targets: Targets<'a>,
        iter: Expr,
        parser: &mut Parser<'a>,
        indent: u32,
        run: bool,
        ctx: Ctx,
        code: &mut Code<'a>,
    ) -> Result<Flow<'a>, Error> {
        let loop_ctx = Ctx {
            in_loop: true,
            ..ctx
        };
        let body = *parser;
        if run {
            let it = self.eval_expr(code, iter)?;
            // The iterable stays on the value stack (rooted) during the loop
            let base = self.sp;
            self.push(it)?;
            let it = self.iterable(it)?;
            self.push(it)?;
            let result = self.run_for(&targets, it, body, parser, indent, loop_ctx, code);
            self.sp = base;
            if let Some(flow) = result? {
                return Ok(flow);
            }
            // The body never ran: it's parsed below
        }
        *parser = body;
        self.exec_body(parser, indent, false, loop_ctx, code)?;
        Ok(Flow::Normal)
    }

    /// The passes of a `for` loop; `None` if the body never ran (the parser
    /// is then still at the body).
    #[allow(clippy::too_many_arguments)]
    fn run_for(
        &mut self,
        targets: &Targets<'a>,
        it: Value<'a>,
        body: Parser<'a>,
        parser: &mut Parser<'a>,
        indent: u32,
        ctx: Ctx,
        code: &mut Code<'a>,
    ) -> Result<Option<Flow<'a>>, Error> {
        let mut state = 0;
        let mut ran = false;
        while let Some(item) = self.iter_next(it, &mut state)? {
            if ran {
                self.tick()?;
            }
            ran = true;
            self.assign_targets(targets, item)?;
            *parser = body;
            match self.exec_body(parser, indent, true, ctx, code)? {
                Flow::Break => return Ok(Some(Flow::Normal)),
                Flow::Return(val) => return Ok(Some(Flow::Return(val))),
                Flow::Normal | Flow::Continue => {}
            }
        }
        Ok(ran.then_some(Flow::Normal))
    }

    // ------------------------------------------------------------------
    // Variables and functions
    // ------------------------------------------------------------------

    /// Removes a variable of the current scope (or a declared global).
    fn del_var(&mut self, name: &str) -> Result<(), Error> {
        if let (true, Value::Obj(dict)) = (self.module_top, self.module_env) {
            return self.module_del(dict, name);
        }
        let (i, global) = if self.is_declared_global(name) {
            (self.find_in(0..self.globals_len, name), true)
        } else {
            (self.find_in(self.scope_start()..self.nvars, name), false)
        };
        let Some(i) = i else {
            // `del f` of a `def` function (functions are global names)
            let at_top = self.call_depth == 0 || global;
            let slot = self
                .funcs
                .iter()
                .position(|f| at_top && f.is_some_and(|f| f.named() && f.name == name))
                .ok_or(Error::UnknownIdentifier)?;
            self.funcs[slot] = None;
            return Ok(());
        };
        self.vars.copy_within(i + 1..self.nvars, i);
        self.nvars -= 1;
        if global || (self.call_depth > 0 && i < self.globals_len) {
            self.globals_len -= 1;
        }
        Ok(())
    }

    /// Slots of the current scope: all globals at the top level, or the
    /// locals of the innermost function call.
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn scope_start(&self) -> usize {
        if self.call_depth == 0 {
            0
        } else {
            self.globals_len + self.frame_off
        }
    }

    /// True if the innermost function call declared `name` with `global`.
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn is_declared_global(&self, name: &str) -> bool {
        self.call_depth > 0
            && self.global_decls[self.frame_globals..self.global_count].contains(&name)
    }

    /// `global a, b` inside a function (at the top level it does nothing).
    fn declare_globals(&mut self, names: &[&'a str]) -> Result<(), Error> {
        if self.call_depth == 0 {
            return Ok(());
        }
        for &name in names {
            if self.is_declared_global(name) {
                continue;
            }
            if self.global_count >= MAX_GLOBAL_DECLS {
                return Err(Error::OutOfMemory);
            }
            self.global_decls[self.global_count] = name;
            self.global_count += 1;
        }
        Ok(())
    }

    /// Assigns a global from inside a function, creating it if needed by
    /// moving the locals of the running calls up one slot.
    fn set_global(&mut self, name: &'a str, val: Value<'a>) -> Result<(), Error> {
        if let Value::Obj(dict) = self.module_env {
            return self.dict_set(dict, Value::Str(name), val);
        }
        if let Some(i) = self.find_in(0..self.globals_len, name) {
            self.vars[i].1 = val;
            return Ok(());
        }
        if self.nvars >= MAX_VARS {
            return Err(Error::OutOfMemory);
        }
        self.vars
            .copy_within(self.globals_len..self.nvars, self.globals_len + 1);
        self.vars[self.globals_len] = (name, val);
        self.globals_len += 1;
        self.nvars += 1;
        Ok(())
    }

    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn find_in(&self, range: core::ops::Range<usize>, name: &str) -> Option<usize> {
        range.into_iter().find(|&i| self.vars[i].0 == name)
    }

    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn set_var(&mut self, name: &'a str, val: Value<'a>) -> Result<(), Error> {
        if self.module_top {
            if let Value::Obj(dict) = self.module_env {
                return self.dict_set(dict, Value::Str(name), val);
            }
        }
        if self.is_declared_global(name) {
            return self.set_global(name, val);
        }
        match self.find_in(self.scope_start()..self.nvars, name) {
            Some(i) => self.vars[i].1 = val,
            None => self.push_var(name, val)?,
        }
        Ok(())
    }

    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn push_var(&mut self, name: &'a str, val: Value<'a>) -> Result<(), Error> {
        if self.nvars >= MAX_VARS {
            return Err(Error::OutOfMemory);
        }
        self.vars[self.nvars] = (name, val);
        self.nvars += 1;
        Ok(())
    }

    /// Looks a name up in the current scope only (the globals, for a name
    /// declared `global`).
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn get_scope_var(&self, name: &str) -> Result<Value<'a>, Error> {
        if let Value::Obj(dict) = self.module_env {
            if self.module_top || self.is_declared_global(name) {
                return self
                    .dict_get_str(dict, name)
                    .ok_or(Error::UnknownIdentifier);
            }
        }
        let range = if self.is_declared_global(name) {
            0..self.globals_len
        } else {
            self.scope_start()..self.nvars
        };
        self.find_in(range, name)
            .map(|i| self.vars[i].1)
            .ok_or(Error::UnknownIdentifier)
    }

    /// `print(*args, sep=' ', end='\n')` (`flush=` is accepted and ignored).
    #[inline(never)]
    fn print(&mut self, args: &[Value<'a>], kwargs: Kwargs<'_, 'a>) -> Result<Value<'a>, Error> {
        let (mut sep, mut end) = (Value::Str(" "), Value::Str("\n"));
        for &(key, v) in kwargs {
            let which = ["sep", "end", "flush"]
                .iter()
                .position(|n| self.str_of(key) == Some(n))
                .ok_or(Error::TypeError)?;
            let v = match v {
                Value::None if which < 2 => Value::Str(if which == 0 { " " } else { "\n" }),
                v => v,
            };
            match which {
                0 if Self::is_str(v) => sep = v,
                1 if Self::is_str(v) => end = v,
                2 => {}
                _ => return Err(Error::TypeError),
            }
        }
        for (i, val) in args.iter().enumerate() {
            if i > 0 {
                self.write_value(sep, false);
            }
            // An object's `__str__` runs first, so its errors propagate
            let base = self.sp;
            let text = match self.custom_str(*val, false) {
                Some(text) => text?,
                None if self.obj_kind(*val).is_some() => self.stringify(*val, false)?,
                None => *val,
            };
            self.push(text)?;
            self.write_value(text, false);
            self.sp = base;
        }
        self.write_value(end, false);
        Ok(Value::None)
    }

    /// Looks a name up in the current scope, then in the globals, then in
    /// the functions, the hardware constants (`OUT`, `IN`, ...) and the
    /// built-in exception classes.
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn get_var(&self, name: &str) -> Result<Value<'a>, Error> {
        self.get_scope_var(name)
            .or_else(|err| {
                // The variables a closure remembers
                match self.closure_env {
                    Value::Obj(env) if self.call_depth > 0 => {
                        self.dict_get_str(env, name).ok_or(err)
                    }
                    _ => Err(err),
                }
            })
            .or_else(|err| {
                // The globals: of the function's module, or of the main script
                if let Value::Obj(dict) = self.module_env {
                    return self.dict_get_str(dict, name).ok_or(err);
                }
                if self.call_depth == 0 {
                    return Err(err);
                }
                self.find_in(0..self.globals_len, name)
                    .map(|i| self.vars[i].1)
                    .ok_or(err)
            })
            .or_else(|err| {
                self.hw
                    .as_ref()
                    .and_then(|hw| hw.constants().iter().find(|(n, _)| *n == name))
                    .map(|&(_, val)| Value::Int(val))
                    .ok_or(err)
            })
            .or_else(|err| {
                // `def` functions of the main script (a module's are in its dict)
                if self.module_env != Value::None {
                    return Err(err);
                }
                self.funcs
                    .iter()
                    .position(|f| f.is_some_and(|f| f.named() && f.name == name))
                    .map(|i| Value::Func(i as u8))
                    .ok_or(err)
            })
            .or_else(|err| {
                // A hardware function as a value: `f = pin_write`, `reset = reset`
                // (after the script's own functions, which shadow it)
                self.hw
                    .as_ref()
                    .and_then(|hw| hw.functions().iter().position(|f| f.name == name))
                    .filter(|&i| i < HW_FUNCS as usize)
                    .map(|i| Value::NativeFn {
                        module: HW_FUNCS,
                        func: i as u8,
                    })
                    .ok_or(err)
            })
            .or_else(|err| {
                exceptions::class_by_name(name)
                    .map(Value::ExcClass)
                    .or_else(|| builtins::builtin_by_name(name).map(Value::Builtin))
                    .or((name == "__name__").then_some(Value::Str("__main__")))
                    .ok_or(err)
            })
    }

    fn define_func(&mut self, func: FuncDef<'a>) -> Result<(), Error> {
        let slot = match self
            .funcs
            .iter()
            .position(|f| f.is_some_and(|f| f.named() && f.name == func.name))
        {
            Some(i) => i,
            None => self.free_func_slot()?,
        };
        self.funcs[slot] = Some(func);
        Ok(())
    }

    /// A free slot in the function table; unreachable lambdas and methods
    /// are collected first if needed.
    fn free_func_slot(&mut self) -> Result<usize, Error> {
        if let Some(i) = self.funcs.iter().position(Option::is_none) {
            return Ok(i);
        }
        self.collect();
        self.funcs
            .iter()
            .position(Option::is_none)
            .ok_or(Error::OutOfMemory)
    }

    /// The slot of a method (a method defined by the same source text, e.g.
    /// when its class statement runs again, gets the same slot back).
    fn method_slot(&mut self, func: FuncDef<'a>) -> Result<u8, Error> {
        let same = |f: &FuncDef<'a>| f.method && core::ptr::eq(f.name, func.name);
        let slot = match self.funcs.iter().position(|f| f.as_ref().is_some_and(same)) {
            Some(i) => i,
            None => self.free_func_slot()?,
        };
        self.funcs[slot] = Some(func);
        Ok(slot as u8)
    }

    /// Calls a `def` function: binds the arguments as new locals and runs the body.
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn call_user_func(
        &mut self,
        func: FuncDef<'a>,
        args: &[Value<'a>],
        kwargs: Kwargs<'_, 'a>,
    ) -> Result<Value<'a>, Error> {
        let stack_base = self.sp;
        let bound = self.bind_args(&func, args, kwargs);
        let args = match bound {
            Ok(values) => values,
            Err(e) => {
                self.sp = stack_base;
                return Err(e);
            }
        };
        let args = &args[..func.count];
        if self.call_depth >= MAX_CALL_DEPTH || self.check_stack().is_err() {
            self.sp = stack_base;
            return Err(Error::RecursionLimit);
        }
        // A closure's variables apply to this call only
        let env = core::mem::replace(&mut self.next_env, Value::None);
        let outer_env = core::mem::replace(&mut self.closure_env, env);
        // The function's global names are those of its module
        let outer_module = (self.module_env, self.module_top);
        (self.module_env, self.module_top) = (func.module, false);

        let saved_off = self.frame_off;
        let saved_globals = (self.frame_globals, self.global_count);
        if self.call_depth == 0 {
            self.globals_len = self.nvars;
        }
        // Offsets from `globals_len` stay valid when a global is inserted
        let base_off = self.nvars - self.globals_len;
        for (&param, &val) in func.params[..func.count].iter().zip(args) {
            if let Err(err) = self.push_var(param, val) {
                self.nvars = self.globals_len + base_off;
                return Err(err);
            }
        }
        self.frame_off = base_off;
        self.frame_globals = self.global_count;
        self.call_depth += 1;
        // The arguments are locals now
        self.sp = stack_base;

        let mut parser = func.body;
        let mut code = Code::new();
        let body_ctx = Ctx {
            in_loop: false,
            in_func: true,
            in_class: false,
        };
        let result = if func.lambda {
            // The body of a lambda is one expression: its value is returned
            parser
                .parse_lambda_parts(&mut code)
                .and_then(|(_, _, _, body)| self.eval_expr(&code, body))
                .map(Flow::Return)
        } else {
            self.exec_body(&mut parser, func.indent, true, body_ctx, &mut code)
        };

        // Drop the locals and `global` declarations, back to the caller's scope
        self.closure_env = outer_env;
        (self.module_env, self.module_top) = outer_module;
        self.call_depth -= 1;
        self.frame_off = saved_off;
        (self.frame_globals, self.global_count) = saved_globals;
        self.nvars = self.globals_len + base_off;

        Ok(match result? {
            Flow::Return(val) => val,
            _ => Value::None,
        })
    }

    /// The values of the parameters of `func` for a call: positional
    /// arguments, then keyword arguments, then default values.
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    #[inline(never)]
    fn bind_args(
        &mut self,
        func: &FuncDef<'a>,
        args: &[Value<'a>],
        kwargs: Kwargs<'_, 'a>,
    ) -> Result<[Value<'a>; MAX_PARAMS], Error> {
        let (star_args, star_kw) = (func.star & STAR_ARGS != 0, func.star & STAR_KWARGS != 0);
        // The parameters that take one argument each
        let fixed = func.count - star_args as usize - star_kw as usize;
        if args.len() > fixed && !star_args {
            return Err(Error::TypeError); // too many arguments
        }
        let mut values = [Value::None; MAX_PARAMS];
        let mut set = [false; MAX_PARAMS];
        let n = args.len().min(fixed);
        values[..n].copy_from_slice(&args[..n]);
        set[..n].fill(true);
        // `*args`: the extra positional arguments, as a tuple (left on the
        // stack, rooted, until the caller has bound them)
        if star_args {
            let extra = &args[n..];
            for &v in extra {
                self.push(v)?;
            }
            self.build(Kind::Tuple, extra.len())?;
            values[fixed] = self.stack[self.sp - 1];
        }
        let extra_kw = if star_kw {
            let d = self.new_container(Kind::Dict, 0)?;
            self.push(Value::Obj(d))?;
            values[func.count - 1] = Value::Obj(d);
            Some(d)
        } else {
            None
        };
        for &(key, v) in kwargs {
            let found = func.params[..fixed]
                .iter()
                .position(|&p| self.str_of(key) == Some(p));
            match (found, extra_kw) {
                (Some(i), _) if set[i] => return Err(Error::TypeError), // multiple values
                (Some(i), _) => {
                    values[i] = v;
                    set[i] = true;
                }
                (None, Some(d)) => self.dict_set(d, key, v)?,
                (None, None) => return Err(Error::TypeError), // unexpected keyword argument
            }
        }
        let ndefaults = match func.defaults {
            Value::Obj(r) => self.count(r),
            _ => 0,
        };
        let first_default = fixed - ndefaults;
        for i in 0..fixed {
            if !set[i] {
                let Value::Obj(r) = func.defaults else {
                    return Err(Error::TypeError); // missing argument
                };
                if i < first_default {
                    return Err(Error::TypeError);
                }
                values[i] = self.slot(r, i - first_default);
            }
        }
        Ok(values)
    }

    // ------------------------------------------------------------------
    // Expressions
    // ------------------------------------------------------------------

    /// Evaluates one RPN expression on the shared value stack.
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn eval_expr(&mut self, code: &Code<'a>, expr: Expr) -> Result<Value<'a>, Error> {
        let base = self.sp;
        let result = self.eval_items(code.items(expr));
        self.sp = base;
        result
    }

    /// Evaluates a complete expression and leaves its value on the stack.
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn eval_items(&mut self, items: &[Item<'a>]) -> Result<Value<'a>, Error> {
        let base = self.sp;
        self.run_items(items)?;
        // The parser only emits well-formed expressions, which leave exactly one value.
        if self.sp != base + 1 {
            return Err(Error::InvalidSyntax);
        }
        Ok(self.stack[base])
    }

    /// Runs RPN items. Operands are replaced by their result only after the
    /// result exists, so anything that allocates keeps its inputs rooted.
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn run_items(&mut self, items: &[Item<'a>]) -> Result<(), Error> {
        self.check_stack()?;
        let mut i = 0;
        while i < items.len() {
            let item = items[i];
            i += 1;
            match item {
                Item::Int(n) => self.push(Value::Int(n))?,
                Item::Str(s) => {
                    let val = self.literal(s)?;
                    self.push(val)?;
                }
                Item::Bytes(s) => {
                    let val = self.bytes_literal(s)?;
                    self.push(val)?;
                }
                Item::Bool(b) => self.push(Value::Bool(b))?,
                Item::None => self.push(Value::None)?,
                Item::Ident(name) => {
                    let val = self.get_var(name)?;
                    self.push(val)?;
                }
                Item::Neg | Item::Invert => {
                    let n = self.stack[self.sp - 1].as_int().ok_or(Error::TypeError)?;
                    self.stack[self.sp - 1] = Value::Int(if item == Item::Neg {
                        n.checked_neg().ok_or(Error::Overflow)?
                    } else {
                        !n // `~n` == `-n - 1`, never overflows
                    });
                }
                Item::Not => {
                    let truthy = self.truthy(self.stack[self.sp - 1]);
                    self.stack[self.sp - 1] = Value::Bool(!truthy);
                }
                Item::Bin(op) => self.apply_bin(op)?,
                Item::Cmp(op) => {
                    let (lhs, rhs) = (self.stack[self.sp - 2], self.stack[self.sp - 1]);
                    let result = self.compare(op, lhs, rhs)?;
                    self.sp -= 1;
                    self.stack[self.sp - 1] = Value::Bool(result);
                }
                Item::CmpChain { op, skip } => {
                    let (lhs, rhs) = (self.stack[self.sp - 2], self.stack[self.sp - 1]);
                    let result = self.compare(op, lhs, rhs)?;
                    self.sp -= 1;
                    if result {
                        self.stack[self.sp - 1] = rhs; // left operand of the next link
                    } else {
                        self.stack[self.sp - 1] = Value::Bool(false);
                        i += skip as usize;
                    }
                }
                Item::AndJump { skip } | Item::OrJump { skip } => {
                    let truthy = self.truthy(self.stack[self.sp - 1]);
                    let is_and = matches!(item, Item::AndJump { .. });
                    if truthy != is_and {
                        // Result decided: keep this operand, skip the right side
                        i += skip as usize;
                    } else {
                        self.sp -= 1;
                    }
                }
                Item::Call { argc, kw, .. }
                | Item::CallValue { argc, kw }
                | Item::CallAttr { argc, kw, .. } => {
                    // Out of line, so the argument arrays don't make every
                    // (recursive) `run_items` frame big
                    self.call_item(item, argc, &items[i..i + kw as usize])?;
                    i += kw as usize;
                }
                Item::KwName(_) => {}
                Item::FormatValue { conv, spec } => {
                    // f-string field: the value (and its spec) are on the stack
                    let n = 1 + spec as usize;
                    let v = self.stack[self.sp - n];
                    let parsed = if spec {
                        let text = self.str_of(self.stack[self.sp - 1]).unwrap_or("");
                        format::Spec::parse(text)?
                    } else {
                        format::Spec::default()
                    };
                    let piece = self.format_one(v, conv as char, &parsed, 0)?;
                    self.sp -= n;
                    self.push(piece)?;
                }
                Item::BuildStr(n) => {
                    let base = self.sp - n as usize;
                    let s = self.join_pieces(base)?;
                    self.push(s)?;
                }
                Item::Lambda(src) => {
                    let f = self.make_lambda(src)?;
                    self.push(f)?;
                    // Inside a function, a lambda remembers its variables
                    let f = self.make_closure(f)?;
                    self.stack[self.sp - 1] = f;
                }
                Item::GetAttr(attr) => {
                    let target = self.stack[self.sp - 1];
                    let val = match self.get_attr(target, attr)? {
                        Some(v) => v,
                        None => self
                            .bound_method(target, attr)?
                            .ok_or(Error::AttributeError)?,
                    };
                    self.stack[self.sp - 1] = val;
                }

                Item::Index => {
                    let (obj, index) = (self.stack[self.sp - 2], self.stack[self.sp - 1]);
                    let val = self.index_value(obj, index)?;
                    self.sp -= 1;
                    self.stack[self.sp - 1] = val;
                }
                Item::Slice { has_start, has_end } => {
                    let bounds = has_start as usize + has_end as usize;
                    let obj = self.stack[self.sp - bounds - 1];
                    let mut next = self.sp - bounds;
                    let mut bound = |present: bool| {
                        if present {
                            next += 1;
                            self.stack[next - 1]
                        } else {
                            Value::None
                        }
                    };
                    let (start, end) = (bound(has_start), bound(has_end));
                    let val = self.slice_value(obj, start, end)?;
                    self.sp -= bounds;
                    self.stack[self.sp - 1] = val;
                }
                Item::BuildList(n) => self.build(Kind::List, n as usize)?,
                Item::BuildTuple(n) => self.build(Kind::Tuple, n as usize)?,
                Item::BuildDict(n) => self.build(Kind::Dict, n as usize)?,
                Item::IfElse { a, cond, b } => {
                    let (a, cond, b) = (a as usize, cond as usize, b as usize);
                    let parts = &items[i..i + a + cond + b];
                    i += a + cond + b;
                    let c = self.eval_items(&parts[a..a + cond])?;
                    self.sp -= 1;
                    let chosen = if self.truthy(c) {
                        &parts[..a]
                    } else {
                        &parts[a + cond..]
                    };
                    self.eval_items(chosen)?;
                }
                Item::Comp {
                    dict,
                    targets,
                    a,
                    b,
                    iter,
                    cond,
                } => {
                    let lens = [a, b, targets, iter, cond].map(|n| n as usize);
                    let total: usize = lens.iter().sum();
                    let parts = &items[i..i + total];
                    i += total;
                    self.comprehension(dict, parts, lens)?;
                }
            }
        }
        Ok(())
    }

    /// `Call`, `CallValue` and `CallAttr`: the arguments (and the callee or
    /// receiver below them) stay on the stack, rooted, during the call.
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    #[inline(never)]
    fn call_item(&mut self, item: Item<'a>, argc: u8, names: &[Item<'a>]) -> Result<(), Error> {
        let start = self.sp - argc as usize;
        let a = self.call_args(start, argc, names)?;
        let (args, kwargs) = (&a.args[..a.npos], &a.kwargs[..a.nkw]);
        let (val, below) = match item {
            Item::Call { name, .. } => (self.call_function(name, args, kwargs)?, start),
            Item::CallValue { .. } => {
                let callee = self.stack[start - 1];
                (self.call_value(callee, args, kwargs)?, start - 1)
            }
            Item::CallAttr { attr, .. } => {
                let receiver = self.stack[start - 1];
                let val = if kwargs.is_empty() {
                    self.call_method(receiver, attr, args)?
                } else {
                    self.call_method_kw(receiver, attr, args, kwargs)?
                };
                (val, start - 1)
            }
            _ => unreachable!("call_item for a call item"),
        };
        self.sp = below;
        self.push(val)
    }

    /// The positional and keyword arguments of a call, from the stack
    /// (`stack[start..]`, keyword values last) and the `KwName` items.
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    #[inline(never)]
    #[allow(clippy::type_complexity)]
    fn call_args(
        &mut self,
        start: usize,
        argc: u8,
        names: &[Item<'a>],
    ) -> Result<CallArgs<'a>, Error> {
        let mut a = CallArgs {
            args: [Value::None; MAX_CALL_ARGS],
            npos: 0,
            kwargs: [(Value::None, Value::None); MAX_CALL_ARGS],
            nkw: 0,
        };
        let fixed = argc as usize - names.len();
        a.args[..fixed].copy_from_slice(&self.stack[start..start + fixed]);
        a.npos = fixed;
        for (k, item) in names.iter().enumerate() {
            let Item::KwName(name) = *item else { continue };
            let v = self.stack[start + fixed + k];
            match name {
                // `*items`: the elements as positional arguments
                "*" => {
                    let mut state = 0;
                    while let Some(x) = self.iter_next(v, &mut state)? {
                        self.push(x)?; // rooted (strings made while iterating)
                        *a.args.get_mut(a.npos).ok_or(Error::TooManyArguments)? = x;
                        a.npos += 1;
                    }
                }
                // `**options`: the pairs of a dict as keyword arguments
                "**" => {
                    let d = match v {
                        Value::Obj(r) if self.heap.kind(r) == Kind::Dict => r,
                        _ => return Err(Error::TypeError),
                    };
                    for p in 0..self.count(d) {
                        let (key, val) = (self.slot(d, 2 * p), self.slot(d, 2 * p + 1));
                        if !Self::is_str(key) {
                            return Err(Error::TypeError); // keywords must be strings
                        }
                        *a.kwargs.get_mut(a.nkw).ok_or(Error::TooManyArguments)? = (key, val);
                        a.nkw += 1;
                    }
                }
                // A positional argument after `*items`
                "" => {
                    *a.args.get_mut(a.npos).ok_or(Error::TooManyArguments)? = v;
                    a.npos += 1;
                }
                name => {
                    *a.kwargs.get_mut(a.nkw).ok_or(Error::TooManyArguments)? =
                        (Value::Str(name), v);
                    a.nkw += 1;
                }
            }
        }
        Ok(a)
    }

    /// A function that remembers the values of the variables of the
    /// function it's made in (only inside a function; elsewhere `f` itself).
    #[inline(never)]
    fn make_closure(&mut self, f: Value<'a>) -> Result<Value<'a>, Error> {
        if self.call_depth == 0 {
            return Ok(f);
        }
        let base = self.sp;
        let result = (|| {
            let env = self.new_container(Kind::Dict, 0)?;
            self.push(Value::Obj(env))?;
            for i in self.scope_start()..self.nvars {
                let (name, v) = self.vars[i];
                self.dict_set(env, Value::Str(name), v)?;
            }
            // A closure made inside a closure sees the outer one's variables too
            if let Value::Obj(outer) = self.closure_env {
                for p in 0..self.count(outer) {
                    let (key, v) = (self.slot(outer, 2 * p), self.slot(outer, 2 * p + 1));
                    if self.dict_find(env, key)?.is_none() {
                        self.dict_set(env, key, v)?;
                    }
                }
            }
            self.push(f)?;
            self.push(Value::Obj(env))?;
            self.build(Kind::Tuple, 2)?;
            let c = self.stack[self.sp - 1];
            if let Value::Obj(r) = c {
                self.heap.set_kind(r, Kind::Closure);
            }
            Ok(c)
        })();
        self.sp = base;
        result
    }

    /// A method call with keyword arguments: only `list.sort(key=, reverse=)`.
    #[inline(never)]
    fn call_method_kw(
        &mut self,
        receiver: Value<'a>,
        attr: &str,
        args: &[Value<'a>],
        kwargs: Kwargs<'_, 'a>,
    ) -> Result<Value<'a>, Error> {
        match (receiver, attr, args) {
            (Value::Module(m), _, _) if self.is_builtins_module(m) => {
                self.call_function(attr, args, kwargs)
            }
            (Value::Obj(_), _, _) if self.module_dict(receiver).is_some() => {
                let f = self
                    .py_module_attr(receiver, attr)
                    .flatten()
                    .ok_or(Error::AttributeError)?;
                self.call_value(f, args, kwargs)
            }
            (Value::Str(_) | Value::HStr(_), "format", _) => {
                self.str_format(receiver, args, kwargs)
            }
            (Value::Obj(_), _, _)
                if matches!(
                    self.obj_kind(receiver),
                    Some(Kind::Class | Kind::Instance | Kind::Super)
                ) =>
            {
                self.object_method(receiver, attr, args, kwargs)
                    .unwrap_or(Err(Error::AttributeError))
            }
            (Value::Obj(r), "sort", []) if self.obj_kind(receiver) == Some(Kind::List) => {
                let (key, reverse) = self.sort_options(kwargs)?;
                self.sort_list(r, key, reverse).map(|()| Value::None)
            }
            _ => Err(Error::TypeError),
        }
    }

    /// The function value of a lambda (its text identifies it, so running
    /// the same `lambda` again reuses its slot in the function table).
    #[inline(never)]
    fn make_lambda(&mut self, src: &'a str) -> Result<Value<'a>, Error> {
        let same = |f: &FuncDef<'a>| {
            f.lambda && f.name.as_ptr() == src.as_ptr() && f.name.len() == src.len()
        };
        if let Some(i) = self.funcs.iter().position(|f| f.as_ref().is_some_and(same)) {
            return Ok(Value::Func(i as u8));
        }
        let mut parser = Parser::new(src)?;
        let (params, count, star, _) = parser.parse_lambda_parts(&mut Code::new())?;
        let slot = self.free_func_slot()?;
        self.funcs[slot] = Some(FuncDef {
            name: src,
            params,
            count,
            indent: 0,
            body: Parser::new(src)?,
            defaults: Value::None,
            lambda: true,
            method: false,
            star,
            module: self.module_env,
        });
        Ok(Value::Func(slot as u8))
    }

    /// Calls a function value: a `def` function or lambda, a native module
    /// function, or an exception class.
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    pub(super) fn call_value(
        &mut self,
        f: Value<'a>,
        args: &[Value<'a>],
        kwargs: Kwargs<'_, 'a>,
    ) -> Result<Value<'a>, Error> {
        match f {
            Value::Func(slot) => {
                let func = self.funcs[slot as usize].ok_or(Error::UnknownIdentifier)?;
                self.call_user_func(func, args, kwargs)
            }
            Value::NativeFn { module, func } if kwargs.is_empty() => {
                self.call_native(module, func, args)
            }
            Value::Builtin(b) if builtins::builtin_name(b) != "NoneType" => {
                self.call_function(builtins::builtin_name(b), args, kwargs)
            }
            Value::ExcClass(class) if kwargs.is_empty() => self.new_exception(class, args),
            Value::Obj(r) if self.heap.kind(r) == Kind::BoundMethod => {
                self.call_bound(r, args, kwargs)
            }
            Value::Obj(r) if self.heap.kind(r) == Kind::Class => self.instantiate(r, args, kwargs),
            // An object whose class defines `__call__`: `pin()`, `pin(1)`
            Value::Obj(r) if self.heap.kind(r) == Kind::Instance => {
                match self.call_dunder_kw(f, "__call__", args, kwargs) {
                    Some(result) => result,
                    None => Err(Error::TypeError),
                }
            }
            Value::Obj(r) if self.heap.kind(r) == Kind::Closure => {
                self.next_env = self.slot(r, 1);
                let result = self.call_value(self.slot(r, 0), args, kwargs);
                self.next_env = Value::None;
                result
            }
            _ => Err(Error::TypeError), // not callable, or no keyword arguments
        }
    }

    /// Runs a comprehension (see `Item::Comp`) and pushes the new list/dict.
    #[inline(never)]
    fn comprehension(
        &mut self,
        dict: bool,
        parts: &[Item<'a>],
        lens: [usize; 5],
    ) -> Result<(), Error> {
        let [a, b, ntargets, iter, cond] = lens;
        let (elem, rest) = parts.split_at(a);
        let (value, rest) = rest.split_at(b);
        let (target_items, rest) = rest.split_at(ntargets);
        let (iter_items, cond_items) = rest.split_at(iter);

        let mut targets = Targets {
            names: [""; MAX_TARGETS],
            count: ntargets,
            unpack: ntargets > 1,
        };
        for (slot, item) in targets.names.iter_mut().zip(target_items) {
            if let Item::Ident(name) = item {
                *slot = name;
            }
        }
        let base = self.sp;
        let it = self.eval_items(iter_items)?;
        let it = self.iterable(it)?;
        self.push(it)?;
        let result = self.new_container(if dict { Kind::Dict } else { Kind::List }, 0)?;
        self.push(Value::Obj(result))?;
        // Like Python 3, the loop variables don't leak: save what they were
        // (kept on the stack, so the old values stay rooted)
        let mut existed = [false; MAX_TARGETS];
        for (k, &name) in targets.names().iter().enumerate() {
            let old = self.get_scope_var(name);
            existed[k] = old.is_ok();
            self.push(old.unwrap_or(Value::None))?;
        }
        let saved = self.sp - ntargets;
        if !dict && targets.count == 1 && targets.unpack {
            targets.unpack = false;
        }

        let mut state = 0;
        let outcome = loop {
            // One pass: `Ok(false)` when the iterable is exhausted
            let pass = (|| {
                let Some(item) = self.iter_next(it, &mut state)? else {
                    return Ok(false);
                };
                self.assign_targets(&targets, item)?;
                if cond > 0 {
                    let c = self.eval_items(cond_items)?;
                    self.sp -= 1;
                    if !self.truthy(c) {
                        return Ok(true);
                    }
                }
                let top = self.sp;
                let k = self.eval_items(elem)?;
                if dict {
                    let v = self.eval_items(value)?;
                    self.dict_set(result, k, v)?;
                } else {
                    self.list_append(result, k)?;
                }
                self.sp = top;
                self.tick().map(|()| true)
            })();
            match pass {
                Ok(true) => {}
                Ok(false) => break Ok(()),
                Err(e) => break Err(e),
            }
        };

        // Restore (or remove) the loop variables
        for (k, &name) in targets.names().iter().enumerate() {
            if existed[k] {
                self.set_var(name, self.stack[saved + k])?;
            } else {
                let _ = self.del_var(name);
            }
        }
        outcome?;
        self.sp = base;
        self.push(Value::Obj(result))
    }

    /// Replaces the top two stack values with `lhs op rhs`.
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn apply_bin(&mut self, op: BinOp) -> Result<(), Error> {
        let (lhs, rhs) = (self.stack[self.sp - 2], self.stack[self.sp - 1]);
        let val = if op == BinOp::Mod && Self::is_str(lhs) {
            self.percent_format(lhs, rhs)?
        } else if matches!(lhs, Value::Obj(_)) || matches!(rhs, Value::Obj(_)) {
            self.container_binop(op, lhs, rhs)?
        } else if Self::is_str(lhs)
            || Self::is_str(rhs)
            || Self::is_bytes(lhs)
            || Self::is_bytes(rhs)
        {
            self.str_binop(op, lhs, rhs)?
        } else {
            binop(op, lhs, rhs)?
        };
        self.sp -= 1;
        self.stack[self.sp - 1] = val;
        Ok(())
    }

    /// `receiver.attr(args)`: a module function or a string method.
    fn call_method(
        &mut self,
        receiver: Value<'a>,
        attr: &str,
        args: &[Value<'a>],
    ) -> Result<Value<'a>, Error> {
        match receiver {
            Value::Module(module) if self.is_builtins_module(module) => {
                self.call_function(attr, args, &[])
            }
            Value::Module(module) => {
                let func = self
                    .module_func(module, attr)
                    .ok_or(Error::AttributeError)?;
                self.call_native(module, func, args)
            }
            Value::Globals => self.globals_view(attr, args),
            Value::Obj(_) if self.module_dict(receiver).is_some() => {
                let f = self
                    .py_module_attr(receiver, attr)
                    .flatten()
                    .ok_or(Error::AttributeError)?;
                self.call_value(f, args, &[])
            }
            Value::Str(_) | Value::HStr(_) => self.str_method(receiver, attr, args),
            Value::Bytes(_) | Value::HBytes(_) => self.bytes_method(receiver, attr, args),
            Value::Obj(_) => match self.file_method(receiver, attr, args) {
                Some(result) => result,
                None if matches!(
                    self.obj_kind(receiver),
                    Some(Kind::Class | Kind::Instance | Kind::Super)
                ) =>
                {
                    self.object_method(receiver, attr, args, &[])
                        .unwrap_or(Err(Error::AttributeError))
                }
                None => self
                    .container_method(receiver, attr, args)
                    .unwrap_or(Err(Error::AttributeError)),
            },
            _ => Err(Error::AttributeError),
        }
    }

    /// Comparison with Python semantics: numbers (ints/bools) and strings
    /// compare among themselves; `==`/`!=` between other types are simply
    /// false/true; ordering between other types is a `TypeError`.
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn compare(&self, op: CmpOp, lhs: Value<'a>, rhs: Value<'a>) -> Result<bool, Error> {
        match op {
            CmpOp::Is => return Ok(self.identical(lhs, rhs)),
            CmpOp::IsNot => return Ok(!self.identical(lhs, rhs)),
            _ => {}
        }
        if matches!(op, CmpOp::In | CmpOp::NotIn) {
            if let Some(found) = self.contains(rhs, lhs) {
                return Ok(found? == (op == CmpOp::In));
            }
        }
        if matches!(lhs, Value::Obj(_)) || matches!(rhs, Value::Obj(_)) {
            return match op {
                CmpOp::Eq => Ok(self.equal(lhs, rhs)),
                CmpOp::Ne => Ok(!self.equal(lhs, rhs)),
                _ => {
                    let ordering = self.seq_order(lhs, rhs)?;
                    Ok(match op {
                        CmpOp::Lt => ordering == CmpOrdering::Less,
                        CmpOp::Le => ordering != CmpOrdering::Greater,
                        CmpOp::Gt => ordering == CmpOrdering::Greater,
                        _ => ordering != CmpOrdering::Less,
                    })
                }
            };
        }
        if matches!(op, CmpOp::In | CmpOp::NotIn) {
            let found = if let Some(hay) = self.bytes_of(rhs) {
                // `5 in b` looks for a byte value, `b'ab' in b` for a sequence
                match (lhs.as_int(), self.bytes_of(lhs)) {
                    (Some(n), _) if (0..=255).contains(&n) => hay.contains(&(n as u8)),
                    (Some(_), _) => return Err(Error::ValueError),
                    (_, Some(needle)) => bytes::find_bytes(hay, needle).is_some(),
                    _ => return Err(Error::TypeError),
                }
            } else {
                let hay = self.str_of(rhs).ok_or(Error::TypeError)?;
                let needle = self.str_of(lhs).ok_or(Error::TypeError)?;
                hay.contains(needle)
            };
            return Ok(found == (op == CmpOp::In));
        }
        let ordering = match (
            self.str_of(lhs)
                .map(str::as_bytes)
                .or_else(|| self.bytes_of(lhs).filter(|_| Self::is_bytes(rhs))),
            self.str_of(rhs)
                .map(str::as_bytes)
                .or_else(|| self.bytes_of(rhs).filter(|_| Self::is_bytes(lhs))),
        ) {
            // str with str and bytes with bytes; a str never equals bytes
            (Some(a), Some(b)) if Self::is_bytes(lhs) == Self::is_bytes(rhs) => a.cmp(b),
            _ => match (lhs.as_int(), rhs.as_int()) {
                (Some(a), Some(b)) => a.cmp(&b),
                _ => {
                    // None == None, and a module, class or function equals itself
                    let equal = self.equal(lhs, rhs);
                    return match op {
                        CmpOp::Eq => Ok(equal),
                        CmpOp::Ne => Ok(!equal),
                        _ => Err(Error::TypeError),
                    };
                }
            },
        };
        Ok(match op {
            CmpOp::Eq => ordering == CmpOrdering::Equal,
            CmpOp::Ne => ordering != CmpOrdering::Equal,
            CmpOp::Lt => ordering == CmpOrdering::Less,
            CmpOp::Le => ordering != CmpOrdering::Greater,
            CmpOp::Gt => ordering == CmpOrdering::Greater,
            CmpOp::Ge => ordering != CmpOrdering::Less,
            CmpOp::In | CmpOp::NotIn | CmpOp::Is | CmpOp::IsNot => unreachable!("handled above"),
        })
    }

    /// Calls a `def` function, the built-in `print`, or a hardware function.
    /// Arguments are already evaluated, so a failing argument means nothing is
    /// printed or sent to hardware (like Python).
    #[cfg_attr(feature = "iram", link_section = ".rwtext")]
    fn call_function(
        &mut self,
        name: &str,
        args: &[Value<'a>],
        kwargs: Kwargs<'_, 'a>,
    ) -> Result<Value<'a>, Error> {
        // A variable holding a function: a module function (`from display
        // import line`), a `def` function or lambda, or an exception class
        // (`ValueError('bad')` creates an exception object)
        match self.get_var(name) {
            Ok(f @ (Value::NativeFn { .. } | Value::Func(_) | Value::ExcClass(_))) => {
                return self.call_value(f, args, kwargs);
            }
            // `w = f.write; w(data)`, `p = Point(1, 2)`, `led(1)` (`__call__`)
            Ok(f @ Value::Obj(r))
                if matches!(
                    self.heap.kind(r),
                    Kind::BoundMethod | Kind::Class | Kind::Closure | Kind::Instance
                ) =>
            {
                return self.call_value(f, args, kwargs);
            }
            // `op = abs; op(-3)` (but `abs(-3)` itself is handled below)
            Ok(Value::Builtin(b)) if builtins::builtin_name(b) != name => {
                return self.call_function(builtins::builtin_name(b), args, kwargs);
            }
            _ => {}
        }
        if name == "print" {
            return self.print(args, kwargs);
        }
        if name == "sorted" && !kwargs.is_empty() {
            let [it] = args else {
                return Err(Error::TypeError);
            };
            return self.sorted(*it, kwargs);
        }
        if !kwargs.is_empty() {
            return Err(Error::TypeError); // the other built-ins have no keyword arguments
        }
        match (name, args) {
            ("open", _) => return self.open(args),
            ("format", [v]) => return self.stringify(*v, false),
            ("hex", [v]) => return self.int_text(*v, 'x'),
            ("bin", [v]) => return self.int_text(*v, 'b'),
            ("oct", [v]) => return self.int_text(*v, 'o'),
            ("format", [v, spec]) => return self.format_builtin(*v, *spec),
            _ => {}
        }

        if let Some(result) = self.value_builtin(name, args) {
            return result;
        }
        if let Some(result) = self.str_builtin(name, args) {
            return result;
        }
        if let Some(result) = self.container_builtin(name, args) {
            return result;
        }

        match (name, args) {
            ("abs", [val]) => {
                let n = val.as_int().ok_or(Error::TypeError)?;
                return n.checked_abs().map(Value::Int).ok_or(Error::Overflow);
            }
            ("abs", _) => return Err(Error::TypeError),
            ("bool", []) => return Ok(Value::Bool(false)),
            ("bool", [val]) => return Ok(Value::Bool(self.truthy(*val))),
            ("min" | "max", [first, rest @ ..]) if !rest.is_empty() => {
                let want = if name == "min" { CmpOp::Lt } else { CmpOp::Gt };
                let mut best = *first;
                for &val in rest {
                    if self.compare(want, val, best)? {
                        best = val;
                    }
                }
                return Ok(best);
            }
            // min()/max() of one value would need an iterable
            ("min" | "max", _) => return Err(Error::TypeError),
            _ => {}
        }

        if name == "help" {
            match args {
                [] => self.print_help(),
                [Value::Module(module)] => self.print_module_help(*module),
                [v] if self.str_of(*v) == Some("modules") => self.print_modules(),
                _ => return Err(Error::TypeError),
            }
            return Ok(Value::None);
        }

        let hw = self.hw.as_mut().ok_or(Error::UnknownIdentifier)?;

        // Check the name and argument count against the bridge's function table
        let table = hw.functions();
        let mut returns_bytes = false;
        if !table.is_empty() {
            let spec = table
                .iter()
                .find(|f| f.name == name)
                .ok_or(Error::UnknownIdentifier)?;
            if args.len() < spec.min_args as usize || args.len() > spec.max_args as usize {
                return Err(Error::TypeError);
            }
            returns_bytes = spec.returns_bytes;
        }
        self.call_bridge(None, name, args, returns_bytes)
    }

    // ------------------------------------------------------------------
    // Native modules
    // ------------------------------------------------------------------

    fn hw_modules(&self) -> &[Module] {
        self.hw.as_ref().map_or(&[], |hw| hw.modules())
    }

    /// Module number `i`: core modules (`binascii`) first, then the bridge's.
    fn module(&self, i: u8) -> Option<Module> {
        lookup_module(self.hw_modules(), i)
    }

    fn module_count(&self) -> usize {
        CORE_MODULES.len() + self.hw_modules().len()
    }

    fn find_module(&self, name: &str) -> Option<u8> {
        (0..self.module_count() as u8).find(|&i| self.module(i).is_some_and(|m| m.name == name))
    }

    fn module_func(&self, module: u8, name: &str) -> Option<u8> {
        self.module(module)?
            .functions
            .iter()
            .position(|f| f.name == name)
            .map(|i| i as u8)
    }

    /// `module.name`: a constant, or a function value.
    fn is_builtins_module(&self, module: u8) -> bool {
        self.module(module).is_some_and(|m| m.name == "builtins")
    }

    fn module_attr(&self, module: u8, name: &str) -> Result<Value<'a>, Error> {
        if self.is_builtins_module(module) {
            return Self::builtins_attr(name).ok_or(Error::AttributeError);
        }
        if let Some(func) = self.module_func(module, name) {
            return Ok(Value::NativeFn { module, func });
        }
        self.module(module)
            .and_then(|m| m.constants.iter().find(|(n, _)| *n == name).copied())
            .map(|(_, val)| Value::Int(val))
            .ok_or(Error::AttributeError)
    }

    /// Calls a native module function after checking its argument count.
    fn call_native(
        &mut self,
        module: u8,
        func: u8,
        args: &[Value<'a>],
    ) -> Result<Value<'a>, Error> {
        if module == HW_FUNCS {
            let spec = *hw_tables(&self.hw)
                .1
                .get(func as usize)
                .ok_or(Error::UnknownIdentifier)?;
            if args.len() < spec.min_args as usize || args.len() > spec.max_args as usize {
                return Err(Error::TypeError);
            }
            return self.call_bridge(None, spec.name, args, spec.returns_bytes);
        }
        let m = self.module(module).ok_or(Error::UnknownIdentifier)?;
        let (module_name, spec) = (m.name, m.functions[func as usize]);
        if args.len() < spec.min_args as usize || args.len() > spec.max_args as usize {
            return Err(Error::TypeError);
        }
        if (module as usize) < CORE_MODULES.len() {
            return self.core_module_call(module_name, spec.name, args);
        }
        self.call_bridge(Some(module_name), spec.name, args, spec.returns_bytes)
    }

    /// Calls a function of the `HardwareBridge` (of a native module, or a
    /// plain one). Ints, bools, strings and bytes are passed as `Arg`s; a
    /// function that returns bytes fills a new bytes object.
    #[inline(never)]
    fn call_bridge(
        &mut self,
        module: Option<&str>,
        name: &str,
        args: &[Value<'a>],
        returns_bytes: bool,
    ) -> Result<Value<'a>, Error> {
        if !returns_bytes {
            let view = self.heap.view();
            let hw_args = bridge_args(&view, args)?;
            let hw = self.hw.as_mut().ok_or(Error::UnknownIdentifier)?;
            let result = match module {
                Some(m) => hw.call_module(m, name, &hw_args[..args.len()])?,
                None => hw.call_args(name, &hw_args[..args.len()])?,
            };
            return Ok(result.map_or(Value::None, Value::Int));
        }
        // The result buffer is the newest object, so the arguments (older
        // objects) can be read while the bridge writes it
        let size = crate::MAX_BRIDGE_BYTES.min(self.heap.capacity());
        let out = self.alloc_obj(Kind::Bytes, size)?;
        let (view, buf) = self.heap.view_and_dst(out);
        let hw_args = bridge_args(&view, args)?;
        let hw = self.hw.as_mut().ok_or(Error::UnknownIdentifier)?;
        let n = match module {
            Some(m) => hw.call_module_bytes(m, name, &hw_args[..args.len()], buf)?,
            None => hw.call_bytes(name, &hw_args[..args.len()], buf)?,
        };
        self.heap.realloc(out, n.min(size));
        Ok(Value::HBytes(out))
    }

    /// `help(module)`: its functions and constants.
    fn print_module_help(&mut self, module: u8) {
        let Some(m) = self.module(module) else {
            return;
        };
        for part in ["Module ", m.name] {
            self.out.write_str(part);
        }
        if !m.doc.is_empty() {
            self.out.write_str(" - ");
            self.out.write_str(m.doc);
        }
        self.out.write_str("\n");
        for f in m.functions {
            for part in ["  ", m.name, ".", f.name, "(", f.params, ")"] {
                self.out.write_str(part);
            }
            if !f.doc.is_empty() {
                self.out.write_str(" - ");
                self.out.write_str(f.doc);
            }
            self.out.write_str("\n");
        }
        if !m.constants.is_empty() {
            self.out.write_str("Constants:\n ");
            for (name, _) in m.constants {
                self.out.write_str(" ");
                self.out.write_str(name);
            }
            self.out.write_str("\n");
        }
    }

    /// `help()`: lists built-ins, the script's functions, and the hardware
    /// functions and constants of the bridge.
    fn print_help(&mut self) {
        self.out.write_str(
            "Built-in:\n  print(...)\n  help()\n  abs(x), min(a, b, ...), max(a, b, ...)\n  len(s), str(x), repr(x), int(x, [base]), ord(c), chr(n), bytes(x), bool(x)\n  list(x), tuple(x), dict(x), range(...), sorted(x), reversed(x), sum(x),\n  any(x), all(x), enumerate(x), zip(a, b), format(x, spec)\n  isinstance(x, t), type(x), hasattr(x, name), getattr(x, name, [default]),\n  callable(x), id(x), dir([x]), globals(), help('modules')\n",
        );

        let mut funcs = self.funcs.iter().flatten().peekable();
        if funcs.peek().is_some() {
            self.out.write_str("Script functions:\n");
        }
        for func in self.funcs.iter().flatten() {
            self.out.write_str("  ");
            self.out.write_str(func.name);
            self.out.write_str("(");
            for (i, param) in func.params[..func.count].iter().enumerate() {
                if i > 0 {
                    self.out.write_str(", ");
                }
                self.out.write_str(param);
            }
            self.out.write_str(")\n");
        }

        self.out
            .write_str("Modules (import them, then help(module)):\n ");
        for i in 0..self.module_count() as u8 {
            if let Some(m) = self.module(i) {
                self.out.write_str(" ");
                self.out.write_str(m.name);
            }
        }
        self.out.write_str("\n");

        let Some(hw) = self.hw.as_ref() else {
            return;
        };
        let (functions, constants) = (hw.functions(), hw.constants());
        if !functions.is_empty() {
            self.out.write_str("Hardware:\n");
        }
        for f in functions {
            for part in ["  ", f.name, "(", f.params, ")"] {
                self.out.write_str(part);
            }
            if !f.doc.is_empty() {
                self.out.write_str(" - ");
                self.out.write_str(f.doc);
            }
            self.out.write_str("\n");
        }
        if !constants.is_empty() {
            self.out.write_str("Constants:\n ");
            for (name, _) in constants {
                self.out.write_str(" ");
                self.out.write_str(name);
            }
            self.out.write_str("\n");
        }
    }
}

/// The `Arg`s for a bridge call (strings and bytes are read from `view`).
fn bridge_args<'v>(
    view: &crate::heap::HeapView<'v>,
    args: &[Value<'v>],
) -> Result<[Arg<'v>; MAX_CALL_ARGS], Error> {
    let mut hw_args = [Arg::Int(0); MAX_CALL_ARGS];
    for (slot, val) in hw_args.iter_mut().zip(args) {
        *slot = match *val {
            Value::Str(s) => Arg::Str(s),
            Value::HStr(r) => Arg::Str(view.str(r)),
            Value::Bytes(b) => Arg::Bytes(b),
            Value::HBytes(r) => Arg::Bytes(view.bytes(r)),
            other => Arg::Int(other.as_int().ok_or(Error::TypeError)?),
        };
    }
    Ok(hw_args)
}

/// Modules built into the interpreter, numbered before the bridge's modules.
const CORE_MODULES: &[Module] = &[
    bytes::BINASCII,
    builtins::BUILTINS_MODULE,
    files::OS,
    builtins::SYS_MODULE,
];

/// Module number `i` among the core modules and then `hw` (the bridge's).
/// Module number of the bridge's plain functions (`pin_write` as a value):
/// `Value::NativeFn { module: HW_FUNCS, func }` indexes `functions()`.
const HW_FUNCS: u8 = u8::MAX;

/// The bridge's modules and plain functions.
fn hw_tables<'h, H: HardwareBridge>(hw: &'h Option<&mut H>) -> (&'h [Module], &'h [HwFn]) {
    match hw {
        Some(hw) => (hw.modules(), hw.functions()),
        None => (&[], &[]),
    }
}

/// Names for rendering (`strings::ModuleLookup`): a module's name and functions.
fn lookup_names<'m>(
    modules: &'m [Module],
    funcs: &'m [HwFn],
    i: u8,
) -> Option<(&'static str, &'m [HwFn])> {
    if i == HW_FUNCS {
        return Some(("", funcs));
    }
    lookup_module(modules, i).map(|m| (m.name, m.functions))
}

fn lookup_module(hw: &[Module], i: u8) -> Option<Module> {
    let i = i as usize;
    match CORE_MODULES.get(i) {
        Some(m) => Some(*m),
        None => hw.get(i - CORE_MODULES.len()).copied(),
    }
}

/// The module in `val`, for `val.attr`; other values have no attributes.
/// Binary operator on values. Ints and bools mix like in Python (`True + 1 == 2`);
/// `&`, `|`, `^` on two bools give a bool.
#[cfg_attr(feature = "iram", link_section = ".rwtext")]
fn binop<'a>(op: BinOp, lhs: Value<'a>, rhs: Value<'a>) -> Result<Value<'a>, Error> {
    let (Some(a), Some(b)) = (lhs.as_int(), rhs.as_int()) else {
        return Err(Error::TypeError);
    };
    let both_bool = matches!((lhs, rhs), (Value::Bool(_), Value::Bool(_)));
    let result = int_binop(op, a, b)?;
    Ok(match op {
        BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor if both_bool => Value::Bool(result != 0),
        _ => Value::Int(result),
    })
}

/// Integer arithmetic with Python semantics and checked overflow.
fn int_binop(op: BinOp, a: i64, b: i64) -> Result<i64, Error> {
    let result = match op {
        BinOp::Add => a.checked_add(b),
        BinOp::Sub => a.checked_sub(b),
        BinOp::Mul => a.checked_mul(b),
        // No float support yet: `/` behaves like `//`
        BinOp::Div | BinOp::FloorDiv => {
            if b == 0 {
                return Err(Error::ZeroDivision);
            }
            // Rust `/` truncates toward zero, Python floors toward -infinity
            a.checked_div(b).map(|q| {
                if a % b != 0 && ((a < 0) != (b < 0)) {
                    q - 1
                } else {
                    q
                }
            })
        }
        BinOp::Mod => {
            if b == 0 {
                return Err(Error::ZeroDivision);
            }
            // Python: result takes the sign of the divisor.
            // `wrapping_rem` only wraps for i64::MIN % -1, whose true result is 0.
            let r = a.wrapping_rem(b);
            Some(if r != 0 && ((r < 0) != (b < 0)) {
                r + b
            } else {
                r
            })
        }
        BinOp::Pow => {
            // Negative exponents would produce a float
            if b < 0 {
                return Err(Error::TypeError);
            }
            u32::try_from(b).ok().and_then(|exp| a.checked_pow(exp))
        }
        BinOp::BitAnd => Some(a & b),
        BinOp::BitOr => Some(a | b),
        BinOp::BitXor => Some(a ^ b),
        BinOp::Shl => {
            if b < 0 {
                return Err(Error::ValueError); // negative shift count
            }
            if a == 0 {
                Some(0)
            } else if b >= 64 {
                None
            } else {
                // Overflow if shifting back doesn't give the original value
                let r = a << b;
                (r >> b == a).then_some(r)
            }
        }
        BinOp::Shr => {
            if b < 0 {
                return Err(Error::ValueError);
            }
            // Arithmetic shift: large counts give 0 or -1, as in Python
            Some(a >> b.min(63))
        }
    };
    result.ok_or(Error::Overflow)
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    struct MockOut {
        buf: [u8; 4096],
        len: usize,
    }

    impl MockOut {
        const fn new() -> Self {
            Self {
                buf: [0; 4096],
                len: 0,
            }
        }
        fn as_str(&self) -> &str {
            core::str::from_utf8(&self.buf[..self.len]).unwrap()
        }
    }

    impl Output for MockOut {
        fn write_str(&mut self, s: &str) {
            let bytes = s.as_bytes();
            self.buf[self.len..self.len + bytes.len()].copy_from_slice(bytes);
            self.len += bytes.len();
        }
    }

    struct MockHardware {
        led_state: bool,
        total_delay_ms: i64,
    }

    impl HardwareBridge for MockHardware {
        fn call(&mut self, name: &str, args: &[i64]) -> Result<Option<i64>, Error> {
            match (name, args) {
                ("led_on", []) => {
                    self.led_state = true;
                    Ok(None)
                }
                ("led_off", []) => {
                    self.led_state = false;
                    Ok(None)
                }
                ("delay_ms", [ms]) => {
                    self.total_delay_ms += *ms;
                    Ok(None)
                }
                // Fake sensor: returns pin number * 10
                ("adc_read", [pin]) => Ok(Some(*pin * 10)),
                ("millis", []) => Ok(Some(self.total_delay_ms)),
                _ => Err(Error::UnknownIdentifier),
            }
        }
    }

    #[test]
    fn test_variables_and_print() {
        let mut out = MockOut::new();
        let mut vm = Interpreter::new(&mut out);

        let code = r#"
msg = 'Hello, Rust!'
x = 10
x = 42
y = x
print(msg)
print('Answer:', y)
"#;

        vm.eval(code).unwrap();
        assert_eq!(out.as_str(), "Hello, Rust!\nAnswer: 42\n");
    }

    #[test]
    fn test_hardware_bridge_calls() {
        let mut out = MockOut::new();
        let mut hw = MockHardware {
            led_state: false,
            total_delay_ms: 0,
        };
        let mut vm = Interpreter::with_hardware(&mut out, &mut hw);

        let code = r#"
wait = 250
led_on()
delay_ms(wait)
led_off()
delay_ms(500)
led_on()
print('LED ok')
"#;

        vm.eval(code).unwrap();

        // Test error paths while `vm` still holds the mutable borrows
        assert_eq!(vm.eval("unknown_hw()"), Err(Error::UnknownIdentifier));
        assert_eq!(vm.eval("led_on(1)"), Err(Error::UnknownIdentifier));
        assert_eq!(vm.eval("delay_ms('bad')"), Err(Error::TypeError));

        // Now `vm` is no longer used, so we can inspect `hw` and `out`
        assert!(hw.led_state);
        assert_eq!(hw.total_delay_ms, 750);
        assert_eq!(out.as_str(), "LED ok\n");
    }

    /// Evaluates `print(<expr>)` and returns what was printed (without newline).
    fn eval_print<'b>(expr: &str, out: &'b mut MockOut) -> Result<&'b str, Error> {
        let mut buf = [0u8; 96];
        let mut len = 0;
        for part in [b"print(".as_slice(), expr.as_bytes(), b")"] {
            buf[len..len + part.len()].copy_from_slice(part);
            len += part.len();
        }
        let src = core::str::from_utf8(&buf[..len]).unwrap();

        let mut heap = [0u8; 2048];
        let mut vm = Interpreter::new(out).with_heap(&mut heap);
        vm.eval(src)?;
        Ok(out.as_str().trim_end_matches('\n'))
    }

    fn assert_eval(expr: &str, expected: &str) {
        let mut out = MockOut::new();
        assert_eq!(eval_print(expr, &mut out), Ok(expected), "expr: {}", expr);
    }

    fn assert_eval_err(expr: &str, expected: Error) {
        let mut out = MockOut::new();
        assert_eq!(eval_print(expr, &mut out), Err(expected), "expr: {}", expr);
    }

    #[test]
    fn test_arithmetic_basic_and_precedence() {
        assert_eval("1 + 2", "3");
        assert_eval("10 - 4 - 3", "3");
        assert_eval("6 * 7", "42");
        assert_eval("1 + 2 * 3", "7");
        assert_eval("(1 + 2) * 3", "9");
        assert_eval("2 * (3 + 4) - 5", "9");
        assert_eval("((2))", "2");
        assert_eval("-5", "-5");
        assert_eval("--5", "5");
        assert_eval("+5", "5");
        assert_eval("-(2 + 3) * 2", "-10");
    }

    #[test]
    fn test_arithmetic_python_division_semantics() {
        // Floor division rounds toward -infinity (not toward zero like Rust)
        assert_eval("7 // 2", "3");
        assert_eval("-7 // 2", "-4");
        assert_eval("7 // -2", "-4");
        assert_eval("-7 // -2", "3");
        assert_eval("6 // -3", "-2");
        // `/` is integer division until floats are supported
        assert_eval("7 / 2", "3");
        assert_eval("-7 / 2", "-4");
        // Modulo takes the sign of the divisor
        assert_eval("7 % 3", "1");
        assert_eval("-7 % 3", "2");
        assert_eval("7 % -3", "-2");
        assert_eval("-7 % -3", "-1");
        assert_eval("6 % 3", "0");
    }

    #[test]
    fn test_arithmetic_power() {
        assert_eval("2 ** 10", "1024");
        assert_eval("2 ** 3 ** 2", "512");
        assert_eval("-2 ** 2", "-4");
        assert_eval("(-2) ** 3", "-8");
        assert_eval("5 ** 0", "1");
        assert_eval("2 ** --2", "4");
    }

    #[test]
    fn test_arithmetic_errors() {
        assert_eval_err("1 / 0", Error::ZeroDivision);
        assert_eval_err("1 // 0", Error::ZeroDivision);
        assert_eval_err("1 % 0", Error::ZeroDivision);
        assert_eval_err("9223372036854775807 + 1", Error::Overflow);
        assert_eval_err("-9223372036854775807 - 2", Error::Overflow);
        assert_eval_err("3037000500 * 3037000500", Error::Overflow);
        assert_eval_err("2 ** 63", Error::Overflow);
        assert_eval_err("2 ** 9999999999", Error::Overflow);
        assert_eval_err("(-9223372036854775807 - 1) // -1", Error::Overflow);
        assert_eval_err("-(-9223372036854775807 - 1)", Error::Overflow);
        assert_eval_err("2 ** -1", Error::TypeError);
        assert_eval_err("'a' + 1", Error::TypeError);
        assert_eval_err("'a' - 'b'", Error::TypeError);
        assert_eval_err("-'a'", Error::TypeError);
        // i64::MIN is reachable and prints correctly
        assert_eval("-9223372036854775807 - 1", "-9223372036854775808");
        // `(-9223372036854775807 - 1) % -1` is 0 in Python
        assert_eval("(-9223372036854775807 - 1) % -1", "0");
    }

    #[test]
    fn test_arithmetic_with_variables_and_hardware() {
        let mut out = MockOut::new();
        let mut hw = MockHardware {
            led_state: false,
            total_delay_ms: 0,
        };
        let mut vm = Interpreter::with_hardware(&mut out, &mut hw);

        let code = r#"
base = 100
count = 3
total = base * count + 50
half = total // 2
count = count - 1
print('total =', total, 'half =', half, 'count =', count)
delay_ms(base * 2)
delay_ms(half - base)
"#;
        vm.eval(code).unwrap();

        // A failed print outputs nothing and keeps variables intact
        assert_eq!(vm.eval("print('x', total / 0)"), Err(Error::ZeroDivision));
        assert_eq!(vm.eval("print(total)"), Ok(()));

        assert_eq!(hw.total_delay_ms, 200 + 75);
        assert_eq!(out.as_str(), "total = 350 half = 175 count = 2\n350\n");
    }

    fn mock_hw() -> MockHardware {
        MockHardware {
            led_state: false,
            total_delay_ms: 0,
        }
    }

    #[test]
    fn test_hardware_return_values_in_expressions() {
        let mut out = MockOut::new();
        let mut hw = mock_hw();
        let mut vm = Interpreter::with_hardware(&mut out, &mut hw);

        let code = r#"
level = adc_read(34) * 3 + 1
delay_ms(adc_read(5) - 20)
print('level =', level, 'now =', millis())
print(adc_read(adc_read(1)))
nothing = led_on()
print(nothing)
"#;
        vm.eval(code).unwrap();

        // A function returning None can't be used in arithmetic
        assert_eq!(vm.eval("x = led_on() + 1"), Err(Error::TypeError));
        // print() itself returns None
        assert_eq!(vm.eval("x = print('side effect')"), Ok(()));
        assert_eq!(vm.eval("print(x)"), Ok(()));

        assert_eq!(hw.total_delay_ms, 30);
        assert_eq!(
            out.as_str(),
            "level = 1021 now = 30\n100\nNone\nside effect\nNone\n"
        );
    }

    #[test]
    fn test_failed_argument_skips_call() {
        let mut out = MockOut::new();
        let mut hw = mock_hw();
        let mut vm = Interpreter::with_hardware(&mut out, &mut hw);

        // The second argument fails, so neither print nor delay_ms run
        assert_eq!(vm.eval("print('a', 1 // 0)"), Err(Error::ZeroDivision));
        assert_eq!(vm.eval("delay_ms(missing)"), Err(Error::UnknownIdentifier));
        assert_eq!(hw.total_delay_ms, 0);
        assert_eq!(out.as_str(), "");
    }

    #[test]
    fn test_error_line_numbers() {
        let mut out = MockOut::new();
        let mut vm = Interpreter::new(&mut out);

        let code = "x = 1\n\n# comment\ny = x * 2\nz = y // 0\nprint(z)\n";
        assert_eq!(vm.eval(code), Err(Error::ZeroDivision));
        assert_eq!(vm.error_line(), Some(5));

        // Parse and lexer errors report the line of the offending token
        assert_eq!(vm.eval("a = 1\nb = (a +\n"), Err(Error::UnexpectedToken));
        assert_eq!(vm.error_line(), Some(3));
        assert_eq!(
            vm.eval("a = 1\nb = 2\nc = 'open"),
            Err(Error::UnterminatedString)
        );
        assert_eq!(vm.error_line(), Some(3));
        assert_eq!(
            vm.eval("print(1)\nprint(`)"),
            Err(Error::UnexpectedChar('`'))
        );
        assert_eq!(vm.error_line(), Some(2));
        assert_eq!(vm.eval("'first token"), Err(Error::UnterminatedString));
        assert_eq!(vm.error_line(), Some(1));

        // A successful run clears it
        assert_eq!(vm.eval("print(1)"), Ok(()));
        assert_eq!(vm.error_line(), None);
    }

    #[test]
    fn test_step_limit() {
        let mut out = MockOut::new();
        let mut vm = Interpreter::new(&mut out);
        vm.set_step_limit(Some(3));

        // Exactly at the limit is fine, and the budget resets for every eval()
        assert_eq!(vm.eval("a = 1\nb = 2\nc = 3"), Ok(()));
        assert_eq!(
            vm.eval("print(1)\nprint(2)\nprint(3)\nprint(4)"),
            Err(Error::StepLimit)
        );
        assert_eq!(vm.error_line(), Some(4));

        vm.set_step_limit(None);
        assert_eq!(vm.eval("a = 1\nb = 2\nc = 3\nd = 4"), Ok(()));

        // Statements before the limit ran
        assert_eq!(out.as_str(), "1\n2\n3\n");
    }

    #[test]
    fn test_abort_flag() {
        let abort = AtomicBool::new(false);
        let mut out = MockOut::new();
        let mut vm = Interpreter::new(&mut out);
        vm.set_abort_flag(&abort);

        assert_eq!(vm.eval("print(1)"), Ok(()));
        abort.store(true, Ordering::Relaxed);
        assert_eq!(vm.eval("print(2)"), Err(Error::Interrupted));
        assert_eq!(vm.error_line(), Some(1));
        abort.store(false, Ordering::Relaxed);
        assert_eq!(vm.eval("print(3)"), Ok(()));

        assert_eq!(out.as_str(), "1\n3\n");
    }

    /// Runs a script and returns its output, panicking on errors.
    fn run_ok<'b>(src: &str, out: &'b mut MockOut) -> &'b str {
        let mut heap = [0u8; 4096];
        let mut vm = Interpreter::new(out).with_heap(&mut heap);
        if let Err(err) = vm.eval(src) {
            panic!("error {:?} at line {:?} in:\n{}", err, vm.error_line(), src);
        }
        out.as_str()
    }

    fn assert_output(src: &str, expected: &str) {
        let mut out = MockOut::new();
        assert_eq!(run_ok(src, &mut out), expected, "script:\n{}", src);
    }

    /// Runs a script that must fail, returning the error and its line.
    fn run_err(src: &str) -> (Error, Option<u32>) {
        let mut out = MockOut::new();
        let mut heap = [0u8; 4096];
        let mut vm = Interpreter::new(&mut out).with_heap(&mut heap);
        let err = vm.eval(src).expect_err("script should fail");
        (err, vm.error_line())
    }

    // ---- 2.2 comparisons and booleans --------------------------------

    #[test]
    fn test_comparisons_and_booleans() {
        assert_eval("1 < 2", "True");
        assert_eval("2 <= 1", "False");
        assert_eval("3 == 3", "True");
        assert_eval("3 != 3", "False");
        assert_eval("5 > 2 + 2", "True");
        assert_eval("4 >= 2 * 2", "True");
        assert_eval("'abc' < 'abd'", "True");
        assert_eval("'a' == 'a'", "True");
        assert_eval("1 == 'a'", "False");
        assert_eval("None == None", "True");
        assert_eval("None != 0", "True");
        assert_eval("True == 1", "True");
        assert_eval("True + True", "2");
        assert_eval("True", "True");
        assert_eval("None", "None");
        assert_eval_err("1 < 'a'", Error::TypeError);
        assert_eval_err("None < 1", Error::TypeError);
    }

    #[test]
    fn test_comparison_chaining() {
        assert_eval("1 < 2 < 3", "True");
        assert_eval("1 < 3 < 2", "False");
        assert_eval("3 > 2 > 1 >= 1", "True");
        assert_eval("1 == 1 != 2", "True");
        // A false link skips the rest, so the TypeError on the right is never hit
        assert_eval("2 < 1 < 'x'", "False");
    }

    #[test]
    fn test_and_or_not_short_circuit() {
        // `and`/`or` return one of their operands, like Python
        assert_eval("0 or 5", "5");
        assert_eval("3 or 5", "3");
        assert_eval("0 and 5", "0");
        assert_eval("3 and 5", "5");
        assert_eval("'' or 'default'", "default");
        assert_eval("None or 0 or 7", "7");
        assert_eval("1 and 2 and 3", "3");
        assert_eval("not 0", "True");
        assert_eval("not 'x'", "False");
        assert_eval("not 1 == 2", "True");
        assert_eval("not not 3", "True");
        // Precedence: `and` binds tighter than `or`, `not` tighter than `and`
        assert_eval("1 or 0 and 0", "1");
        assert_eval("not 0 and 0", "0"); // (not 0) and 0
                                         // The right side is not evaluated when the result is already known
        assert_eval("0 and 1 // 0", "0");
        assert_eval("1 or missing_var", "1");
        assert_eval("0 < 1 and 2 < 3 or 1 // 0", "True");
    }

    #[test]
    fn test_short_circuit_skips_calls() {
        let mut out = MockOut::new();
        let mut hw = mock_hw();
        let mut vm = Interpreter::with_hardware(&mut out, &mut hw);
        vm.eval("x = 0 and delay_ms(100)\ny = 1 or delay_ms(100)\nz = 1 and delay_ms(5)")
            .unwrap();
        assert_eq!(hw.total_delay_ms, 5);
    }

    // ---- 2.3 hex/binary literals and bitwise operators --------------

    #[test]
    fn test_bitwise_operators() {
        assert_eval("0xF0 | 0x0F", "255");
        assert_eval("0xFF & 0b1010", "10");
        assert_eval("0b1100 ^ 0b1010", "6");
        assert_eval("~0", "-1");
        assert_eval("~5", "-6");
        assert_eval("1 << 10", "1024");
        assert_eval("-16 >> 2", "-4");
        assert_eval("1 >> 100", "0");
        assert_eval("-1 >> 100", "-1");
        assert_eval("0 << 100", "0");
        assert_eval("0o17", "15");
        assert_eval("1_000 + 0x_10", "1016");
        // Precedence: shifts < arithmetic, & < shifts, ^ < &, | < ^, comparisons < |
        assert_eval("1 << 2 + 1", "8");
        assert_eval("0xFF & 1 << 4", "16");
        assert_eval("1 | 2 ^ 3 & 1", "3");
        assert_eval("4 | 1 == 5", "True");
        // Register-style manipulation
        assert_eval("(0b1011 >> 1) & 1", "1");
        assert_eval("0xA5 & ~0x0F", "160");
        // bool & bool stays bool
        assert_eval("True & False", "False");
        assert_eval("True | 0", "1");
        assert_eval_err("1 << 63", Error::Overflow);
        assert_eval_err("1 << 64", Error::Overflow);
        assert_eval_err("1 << -1", Error::ValueError);
        assert_eval_err("1 >> -1", Error::ValueError);
        assert_eval_err("'a' | 1", Error::TypeError);
        assert_eval_err("~'a'", Error::TypeError);
        assert_eval("-1 << 63", "-9223372036854775808");
    }

    // ---- 2.4 augmented assignment ------------------------------------

    #[test]
    fn test_augmented_assignment() {
        assert_output(
            "x = 10\nx += 5\nx -= 3\nx *= 2\nx //= 5\nx %= 3\nx **= 3\nprint(x)\n\
             flags = 0\nflags |= 0b101\nflags &= ~1\nflags ^= 0b11\nflags <<= 4\nflags >>= 1\nprint(flags)",
            "1\n56\n",
        );
        let (err, line) = run_err("a = 1\nmissing += 1");
        assert_eq!((err, line), (Error::UnknownIdentifier, Some(2)));
        assert_output("s = 'a'\ns += 'b'\ns *= 2\nprint(s)", "abab\n");
        assert_eq!(run_err("s = 'a'\ns -= 'b'").0, Error::TypeError);
        assert_eq!(run_err("1 += 2").0, Error::InvalidSyntax);
    }

    // ---- 2.1 control flow --------------------------------------------

    #[test]
    fn test_if_elif_else() {
        let src = r#"
def classify(n):
    if n < 0:
        return 'negative'
    elif n == 0:
        return 'zero'
    elif n < 10:
        return 'small'
    else:
        return 'big'

print(classify(-5), classify(0), classify(3), classify(42))
"#;
        assert_output(src, "negative zero small big\n");

        // Only the first true branch runs, and nested ifs work
        assert_output(
            "x = 5\nif x > 1:\n    print('a')\n    if x > 3:\n        print('b')\n    else:\n        print('c')\nelif x > 0:\n    print('d')\nprint('end')",
            "a\nb\nend\n",
        );
        // if without else, falling through
        assert_output("if 0:\n    print('no')\nprint('yes')", "yes\n");
        // Inline bodies
        assert_output("x = 1\nif x: print('one')\nelse: print('other')", "one\n");
    }

    #[test]
    fn test_skipped_branches_are_not_executed() {
        let mut out = MockOut::new();
        let mut hw = mock_hw();
        let mut vm = Interpreter::with_hardware(&mut out, &mut hw);
        let src = "if 0:\n    delay_ms(100)\n    x = 1 // 0\nelif 1:\n    delay_ms(7)\nelif 1 // 0:\n    delay_ms(100)\nelse:\n    delay_ms(100)";
        vm.eval(src).unwrap();
        assert_eq!(hw.total_delay_ms, 7);
    }

    #[test]
    fn test_while_loops() {
        assert_output(
            "i = 0\ntotal = 0\nwhile i < 5:\n    total += i\n    i += 1\nprint(i, total)",
            "5 10\n",
        );
        // break / continue, nested loops
        let src = r#"
i = 0
while True:
    i += 1
    if i % 2 == 0:
        continue
    if i > 7:
        break
    j = 0
    while j < i:
        j += 1
    print(i, j)
print('done', i)
"#;
        assert_output(src, "1 1\n3 3\n5 5\n7 7\ndone 9\n");
        // Inline body and a loop that never runs
        assert_output(
            "n = 3\nwhile n: n -= 1\nwhile 0:\n    print('never')\nprint(n)",
            "0\n",
        );
    }

    #[test]
    fn test_blink_loop_with_hardware() {
        let mut out = MockOut::new();
        let mut hw = mock_hw();
        let mut vm = Interpreter::with_hardware(&mut out, &mut hw);
        let src = r#"
count = 0
while count < 3:
    led_on()
    delay_ms(100)
    led_off()
    delay_ms(50)
    count += 1
led_on()
"#;
        vm.eval(src).unwrap();
        assert!(hw.led_state);
        assert_eq!(hw.total_delay_ms, 450);
    }

    #[test]
    fn test_step_limit_stops_infinite_loops() {
        let mut out = MockOut::new();
        let mut vm = Interpreter::new(&mut out);
        vm.set_step_limit(Some(1000));
        assert_eq!(
            vm.eval("x = 0\nwhile True:\n    x += 1"),
            Err(Error::StepLimit)
        );
        assert_eq!(vm.error_line(), Some(3));
        // `while 1: pass` also counts steps
        assert_eq!(vm.eval("while 1: pass"), Err(Error::StepLimit));
        // State is usable afterwards
        assert_eq!(vm.eval("print(x > 100)"), Ok(()));
    }

    // ---- 2.5 functions -----------------------------------------------

    #[test]
    fn test_functions() {
        let src = r#"
def add(a, b):
    return a + b

def blink_ms(n, on_ms):
    total = 0
    i = 0
    while i < n:
        total += on_ms * 2
        i += 1
    return total

def nothing():
    pass

def early(x):
    if x:
        return 'early'
    print('not reached when x')

print(add(2, 3), add(add(1, 1), 10))
print(blink_ms(3, 50))
print(nothing())
print(early(1))
print(early(0))
"#;
        assert_output(src, "5 12\n300\nNone\nearly\nnot reached when x\nNone\n");
    }

    #[test]
    fn test_function_scopes() {
        let src = r#"
limit = 10
x = 'global'

def f(x):
    # parameter shadows the global, globals are readable
    y = x * 2
    return y + limit

print(f(5), x)

def g():
    local_only = 1
    return local_only

g()
"#;
        assert_output(src, "20 global\n");
        // Locals disappear after the call
        let (err, line) = run_err("def g():\n    local_only = 1\ng()\nprint(local_only)");
        assert_eq!((err, line), (Error::UnknownIdentifier, Some(4)));
        // `x += 1` on a global from inside a function needs a local `x` (like Python)
        let (err, line) = run_err("n = 0\ndef inc():\n    n += 1\ninc()");
        assert_eq!((err, line), (Error::UnknownIdentifier, Some(3)));
    }

    #[test]
    fn test_recursion() {
        let src = r#"
def fact(n):
    if n <= 1:
        return 1
    return n * fact(n - 1)

def fib(n):
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)

print(fact(7), fib(7))
"#;
        assert_output(src, "5040 13\n");

        let (err, line) = run_err("def forever(n):\n    return forever(n + 1)\nforever(0)");
        assert_eq!((err, line), (Error::RecursionLimit, Some(2)));
    }

    #[test]
    fn test_function_errors() {
        // Wrong number of arguments
        assert_eq!(
            run_err("def f(a):\n    return a\nf(1, 2)").0,
            Error::TypeError
        );
        assert_eq!(run_err("def f(a):\n    return a\nf()").0, Error::TypeError);
        // Error inside a function reports the line inside the function
        let (err, line) = run_err("def f(a):\n    b = a\n    return b // 0\nprint(f(1))");
        assert_eq!((err, line), (Error::ZeroDivision, Some(3)));
        // Function is redefinable, and a body is checked for syntax at definition
        assert_output(
            "def f():\n    return 1\ndef f():\n    return 2\nprint(f())",
            "2\n",
        );
        assert_eq!(
            run_err("def f():\n    x = (1 +\n").0,
            Error::UnexpectedToken
        );
        // A class inside a function and duplicate params are rejected
        assert_eq!(
            run_err("def f():\n    class C:\n        pass").0,
            Error::InvalidSyntax
        );
        assert_eq!(run_err("def f(a, a):\n    pass").0, Error::InvalidSyntax);
    }

    #[test]
    fn test_bare_return_and_elif_errors() {
        assert_output("def r():\n    return\nprint(r())", "None\n");
        let (err, line) = run_err("x = 0\nif x:\n    pass\nelif 1 // x:\n    pass");
        assert_eq!((err, line), (Error::ZeroDivision, Some(4)));
    }

    #[test]
    fn test_memory_footprint() {
        // Documented in README: keep an eye on these when changing limits
        let interp = core::mem::size_of::<Interpreter<'_, MockOut, MockHardware>>();
        let code = core::mem::size_of::<Code<'_>>();
        let parser = core::mem::size_of::<Parser<'_>>();
        std::println!("interp={} code={} parser={}", interp, code, parser);
        assert!(interp <= 34816, "Interpreter is {} bytes", interp);
        assert!(code <= 1600, "Code is {} bytes", code);
        assert!(parser <= 128, "Parser is {} bytes", parser);
    }

    #[test]
    fn test_function_variables_after_failed_call() {
        let mut out = MockOut::new();
        let mut vm = Interpreter::new(&mut out);
        vm.eval("g = 1\ndef bad(a):\n    tmp = a\n    return a // 0")
            .unwrap();
        assert_eq!(vm.eval("bad(5)"), Err(Error::ZeroDivision));
        // The failed call's locals are gone, globals are intact
        assert_eq!(vm.eval("print(tmp)"), Err(Error::UnknownIdentifier));
        assert_eq!(vm.eval("print(g)"), Ok(()));
    }

    // ---- syntax and indentation errors --------------------------------

    #[test]
    fn test_control_flow_syntax_errors() {
        use Error::*;
        let cases: [(&str, Error, u32); 14] = [
            ("break", InvalidSyntax, 1),
            ("x = 1\ncontinue", InvalidSyntax, 2),
            ("return 1", InvalidSyntax, 1),
            ("if 1:\n    break", InvalidSyntax, 2),
            ("else:\n    pass", InvalidSyntax, 1),
            ("x = 1\nelif x:\n    pass", InvalidSyntax, 2),
            ("if 1\n    pass", UnexpectedToken, 1),
            ("if 1:\npass", IndentationError, 2),
            ("if 1:", IndentationError, 1),
            ("    x = 1", IndentationError, 1),
            ("x = 1\n  y = 2", IndentationError, 2),
            ("if 1:\n    a = 1\n  b = 2", IndentationError, 3),
            ("if 1: while 1: pass", InvalidSyntax, 1),
            ("while 0:\n    break\n    x = (", UnexpectedToken, 3),
        ];
        for (src, err, line) in cases {
            assert_eq!(run_err(src), (err, Some(line)), "script:\n{}", src);
        }
        // A break inside a loop inside a function is fine; return inside a loop too
        assert_output(
            "def f():\n    while 1:\n        if 1:\n            return 'r'\nprint(f())",
            "r\n",
        );
    }

    #[test]
    fn test_nesting_limit() {
        // 50 nested ifs exceed MAX_NESTING without overflowing the native stack
        let mut buf = [0u8; 4096];
        let mut len = 0;
        for depth in 0..50 {
            for _ in 0..depth {
                buf[len] = b' ';
                len += 1;
            }
            let line = b"if 1:\n";
            buf[len..len + line.len()].copy_from_slice(line);
            len += line.len();
        }
        for _ in 0..50 {
            buf[len] = b' ';
            len += 1;
        }
        buf[len..len + 4].copy_from_slice(b"pass");
        len += 4;
        let src = core::str::from_utf8(&buf[..len]).unwrap();
        assert_eq!(run_err(src).0, Error::RecursionLimit);
    }

    /// A bridge with a function table, constants and a string argument.
    struct TableHardware {
        last_text: [u8; 16],
        text_len: usize,
        pins: [i64; 4],
    }

    const TABLE_FNS: &[crate::HwFn] = &[
        crate::HwFn::new("pin_write", "pin, value", "Set an output pin"),
        crate::HwFn::new("pin_read", "pin", "Read a pin"),
        crate::HwFn::with_args("text", 1, 3, "s, [x, y]", "Show text"),
        crate::HwFn::new("ticks_ms", "", ""),
    ];

    impl HardwareBridge for TableHardware {
        #[cfg_attr(feature = "iram", link_section = ".rwtext")]
        fn call_args(&mut self, name: &str, args: &[Arg<'_>]) -> Result<Option<i64>, Error> {
            match name {
                "pin_write" => {
                    self.pins[args[0].int()? as usize] = args[1].int()?;
                    Ok(None)
                }
                "pin_read" => Ok(Some(self.pins[args[0].int()? as usize])),
                "text" => {
                    let s = args[0].str()?;
                    self.last_text[..s.len()].copy_from_slice(s.as_bytes());
                    self.text_len = s.len();
                    Ok(Some(args.len() as i64))
                }
                "ticks_ms" => Ok(Some(1234)),
                _ => Err(Error::UnknownIdentifier),
            }
        }

        fn functions(&self) -> &[crate::HwFn] {
            TABLE_FNS
        }

        fn constants(&self) -> &[(&'static str, i64)] {
            &[("HIGH", 1), ("LOW", 0), ("LED", 2)]
        }
    }

    #[test]
    fn test_hw_functions_as_values() {
        let mut out = MockOut::new();
        let mut hw = TableHardware {
            last_text: [0; 16],
            text_len: 0,
            pins: [0; 4],
        };
        let mut heap = [0u8; 4096];
        let mut vm = Interpreter::with_hardware(&mut out, &mut hw).with_heap(&mut heap);
        vm.eval(
            "f = pin_write\nf(1, 7)\nprint(f, pin_read(1), callable(f), f == pin_write)\n\
             def apply(g, *a):\n    return g(*a)\nprint(apply(pin_read, 1), [h(1) for h in [pin_read]])\n\
             d = {'w': pin_write}\nd['w'](2, 9)\nprint(pin_read(2), type(f).__name__)",
        )
        .unwrap();
        // Argument counts are still checked; keyword arguments aren't taken
        assert_eq!(vm.eval("f(1)"), Err(Error::TypeError));
        assert_eq!(vm.eval("f(1, value=2)"), Err(Error::TypeError));
        // The script's own functions shadow hardware ones, also inside methods
        vm.eval(
            "def pin_read(p):\n    return 42\nclass C:\n    def m(self):\n        return pin_read(1)\n\
             print(pin_read(1), C().m(), f)",
        )
        .unwrap();
        let _ = vm;
        assert_eq!(
            out.as_str(),
            "<function pin_write> 7 True True\n7 [7]\n9 builtin_function_or_method\n42 42 <function pin_write>\n"
        );
    }

    #[test]
    fn test_call_dunder() {
        let mut out = MockOut::new();
        let text = run_ok(
            "class Pin:\n    def __init__(self, n):\n        self.n = n\n        self.v = 0\n\
             \x20   def __call__(self, v=None, scale=1):\n        if v is None:\n            return self.v\n\
             \x20       self.v = v * scale\np = Pin(2)\np(5)\nprint(p(), p(3, scale=10), p())\n\
             class Plain:\n    pass\ntry:\n    Plain()()\nexcept TypeError:\n    print('not callable')\n",
            &mut out,
        );
        assert_eq!(text, "5 None 30\nnot callable\n");
    }

    #[test]
    fn test_frozen_modules() {
        const FROZEN: &[(&str, &str)] = &[
            ("blinker", "COUNT = 2\ndef twice(x):\n    return x * COUNT\nclass K:\n    def go(self):\n        return twice(5)\n"),
            ("shadowed", "WHERE = 'frozen'\n"),
        ];
        let mut flash = std::vec![0u8; 4 * crate::flatfs::SLOT_SIZE as usize];
        let files = RefCell::new(
            crate::flatfs::FlatFs::mount(crate::flatfs::RamFlash::new(&mut flash)).unwrap(),
        );
        crate::FileSystem::create(&mut *files.borrow_mut(), "shadowed.py", false).unwrap();
        crate::FileSystem::write_file(&mut *files.borrow_mut(), 1, b"WHERE = 'file'\n").unwrap();
        crate::FileSystem::close_file(&mut *files.borrow_mut(), 1).unwrap();

        let mut out = MockOut::new();
        let mut heap = [0u8; 4096];
        let mut memory = [0u8; 64];
        let mut vm = Interpreter::new(&mut out)
            .with_heap(&mut heap)
            .with_frozen_modules(FROZEN);
        // Frozen modules need neither files nor module memory
        vm.eval(
            "import blinker\nfrom blinker import K\nprint(blinker.twice(4), K().go(), blinker)",
        )
        .unwrap();
        vm.eval("import shadowed\nprint(shadowed.WHERE)").unwrap();
        // A file of the same name comes first (on a new interpreter)
        let mut out2 = MockOut::new();
        let mut heap2 = [0u8; 4096];
        let mut vm2 = Interpreter::new(&mut out2)
            .with_heap(&mut heap2)
            .with_files(&files)
            .with_module_memory(&mut memory)
            .with_frozen_modules(FROZEN);
        vm2.eval("import shadowed, blinker\nprint(shadowed.WHERE, blinker.COUNT)")
            .unwrap();
        assert_eq!(vm2.eval("import nothing"), Err(Error::ImportError));
        let _ = (vm, vm2);
        assert_eq!(out.as_str(), "8 10 <module 'blinker'>\nfrozen\n");
        assert_eq!(out2.as_str(), "file 2\n");
    }

    #[test]
    fn test_hw_function_table_and_constants() {
        let mut out = MockOut::new();
        let mut hw = TableHardware {
            last_text: [0; 16],
            text_len: 0,
            pins: [0; 4],
        };
        let mut vm = Interpreter::with_hardware(&mut out, &mut hw);

        vm.eval("pin_write(LED, HIGH)\nprint(pin_read(LED), text('hi', 1, 2), ticks_ms())")
            .unwrap();
        // A variable shadows a constant
        vm.eval("LED = 3\npin_write(LED, True)\nprint(pin_read(3))")
            .unwrap();

        // Table checks happen before the call
        assert_eq!(vm.eval("pin_write(1)"), Err(Error::TypeError));
        assert_eq!(vm.eval("text()"), Err(Error::TypeError));
        assert_eq!(vm.eval("text('a', 1, 2, 3)"), Err(Error::TypeError));
        assert_eq!(vm.eval("not_in_table()"), Err(Error::UnknownIdentifier));
        // Type checks inside the bridge
        assert_eq!(vm.eval("pin_read('x')"), Err(Error::TypeError));
        assert_eq!(vm.eval("text(5)"), Err(Error::TypeError));
        assert_eq!(vm.eval("pin_read(None)"), Err(Error::TypeError));
        // Constants are read-only values, unknown names still fail
        assert_eq!(vm.eval("print(MISSING)"), Err(Error::UnknownIdentifier));

        assert_eq!(&hw.last_text[..hw.text_len], b"hi");
        assert_eq!(out.as_str(), "1 3 1234\n1\n");
    }

    #[test]
    fn test_default_bridge_rejects_strings() {
        // MockHardware only implements `call`, so string arguments are a TypeError
        let mut out = MockOut::new();
        let mut hw = mock_hw();
        let mut vm = Interpreter::with_hardware(&mut out, &mut hw);
        assert_eq!(vm.eval("delay_ms('100')"), Err(Error::TypeError));
        assert_eq!(vm.eval("delay_ms(True)"), Ok(()));
        assert_eq!(hw.total_delay_ms, 1);
    }

    #[test]
    fn test_help() {
        let mut out = MockOut::new();
        let mut hw = TableHardware {
            last_text: [0; 16],
            text_len: 0,
            pins: [0; 4],
        };
        let mut vm = Interpreter::with_hardware(&mut out, &mut hw);
        vm.eval("def blink(times, ms):\n    pass\nhelp()").unwrap();
        assert_eq!(vm.eval("help(1)"), Err(Error::TypeError));
        assert_eq!(
            out.as_str(),
            "Built-in:\n  print(...)\n  help()\n  abs(x), min(a, b, ...), max(a, b, ...)\n  len(s), str(x), repr(x), int(x, [base]), ord(c), chr(n), bytes(x), bool(x)\n  list(x), tuple(x), dict(x), range(...), sorted(x), reversed(x), sum(x),\n  any(x), all(x), enumerate(x), zip(a, b), format(x, spec)\n  isinstance(x, t), type(x), hasattr(x, name), getattr(x, name, [default]),\n  callable(x), id(x), dir([x]), globals(), help('modules')\n\
             Script functions:\n  blink(times, ms)\n\
             Modules (import them, then help(module)):\n  binascii builtins os sys\n\
             Hardware:\n  pin_write(pin, value) - Set an output pin\n  pin_read(pin) - Read a pin\n  \
             text(s, [x, y]) - Show text\n  ticks_ms()\n\
             Constants:\n  HIGH LOW LED\n"
        );

        // Without hardware: built-ins and the core modules
        let mut out = MockOut::new();
        Interpreter::new(&mut out).eval("help()").unwrap();
        assert_eq!(
            out.as_str(),
            "Built-in:\n  print(...)\n  help()\n  abs(x), min(a, b, ...), max(a, b, ...)\n  len(s), str(x), repr(x), int(x, [base]), ord(c), chr(n), bytes(x), bool(x)\n  list(x), tuple(x), dict(x), range(...), sorted(x), reversed(x), sum(x),\n  any(x), all(x), enumerate(x), zip(a, b), format(x, spec)\n  isinstance(x, t), type(x), hasattr(x, name), getattr(x, name, [default]),\n  callable(x), id(x), dir([x]), globals(), help('modules')\n\
             Modules (import them, then help(module)):\n  binascii builtins os sys\n"
        );
    }

    #[test]
    fn test_hwfn_param_counting() {
        let f = crate::HwFn::new("f", "a, b, c", "");
        assert_eq!((f.min_args, f.max_args), (3, 3));
        let f = crate::HwFn::new("f", "", "");
        assert_eq!((f.min_args, f.max_args), (0, 0));
        assert_eq!(
            crate::NoHardware.call("x", &[]),
            Err(Error::UnknownIdentifier)
        );
        assert_eq!(Arg::Int(1).str(), Err(Error::TypeError));
    }

    #[test]
    fn test_interactive_echo() {
        let mut out = MockOut::new();
        let mut vm = Interpreter::new(&mut out);
        vm.set_interactive(true);
        for line in [
            "1 + 2",
            "x = 5",
            "x * 2",
            "'hi'",
            "\"it's\"",
            "x > 3",
            "None",
            "print('once')",
            "def f():\n    7\n    return 8",
            "f()",
        ] {
            vm.eval(line).unwrap();
        }
        // Expressions inside functions and blocks don't echo, top-level ones do
        vm.eval("if 1:\n    9").unwrap();
        vm.set_interactive(false);
        vm.eval("10").unwrap();
        vm.output().write_str(">>> ");
        assert_eq!(
            out.as_str(),
            "3\n10\n'hi'\n\"it's\"\nTrue\nonce\n8\n9\n>>> "
        );
    }

    #[test]
    fn test_semicolons() {
        assert_output("x = 1; y = 2; print(x + y)", "3\n");
        assert_output("x = 5;\nprint(x)", "5\n");
        // Inline bodies: every `;`-joined statement belongs to the body
        assert_output("if 0: print('a'); print('b')\nprint('c')", "c\n");
        assert_output(
            "if 1: print('a'); print('b')\nelse: print('x'); print('y')",
            "a\nb\n",
        );
        assert_output("i = 0\nwhile i < 3: i += 1; print(i)", "1\n2\n3\n");
        assert_output("def f(a): b = a * 2; return b + 1\nprint(f(4))", "9\n");
        assert_output(
            "i = 0\nwhile 1: i += 1; print(i); break\nprint('done')",
            "1\ndone\n",
        );
        // In a block, `;` just separates statements of that block
        assert_output("if 1:\n    a = 1; b = 2\n    print(a, b)", "1 2\n");
        assert_eq!(run_err("x = 1;; y = 2").0, Error::UnexpectedToken);
        assert_eq!(run_err("; x = 1").0, Error::UnexpectedToken);
        assert_eq!(
            run_err("if 1: x = 1; while 1: pass").0,
            Error::InvalidSyntax
        );
    }

    /// A bridge with two native modules: `gfx` (drawing log) and `clock`.
    struct ModuleHardware {
        pixels: i64,
        last_line: [i64; 5],
        text: [u8; 16],
        text_len: usize,
    }

    const GFX: crate::Module = crate::Module {
        name: "gfx",
        doc: "Test drawing",
        functions: &[
            crate::HwFn::new("pixel", "x, y", "Set a pixel"),
            crate::HwFn::with_args("line", 4, 5, "x0, y0, x1, y1, [color]", "Draw a line"),
            crate::HwFn::new("text", "s", ""),
            crate::HwFn::new("count", "", "Pixels drawn"),
        ],
        constants: &[("WIDTH", 240), ("RED", 0xF800)],
    };
    const CLOCK: crate::Module = crate::Module {
        name: "clock",
        doc: "",
        functions: &[crate::HwFn::new("now", "", "")],
        constants: &[],
    };

    impl ModuleHardware {
        fn new() -> Self {
            Self {
                pixels: 0,
                last_line: [0; 5],
                text: [0; 16],
                text_len: 0,
            }
        }
    }

    impl HardwareBridge for ModuleHardware {
        fn modules(&self) -> &[crate::Module] {
            &[GFX, CLOCK]
        }

        fn call_module(
            &mut self,
            module: &str,
            name: &str,
            args: &[Arg<'_>],
        ) -> Result<Option<i64>, Error> {
            match (module, name) {
                ("gfx", "pixel") => {
                    self.pixels += 1;
                    Ok(None)
                }
                ("gfx", "line") => {
                    for (slot, arg) in self.last_line.iter_mut().zip(args) {
                        *slot = arg.int()?;
                    }
                    if args.len() == 4 {
                        self.last_line[4] = 0xFFFF; // default color
                    }
                    Ok(None)
                }
                ("gfx", "text") => {
                    let s = args[0].str()?;
                    self.text[..s.len()].copy_from_slice(s.as_bytes());
                    self.text_len = s.len();
                    Ok(None)
                }
                ("gfx", "count") => Ok(Some(self.pixels)),
                ("clock", "now") => Ok(Some(1234)),
                _ => Err(Error::UnknownIdentifier),
            }
        }
    }

    #[test]
    fn test_import_and_module_calls() {
        let mut out = MockOut::new();
        let mut hw = ModuleHardware::new();
        let mut vm = Interpreter::with_hardware(&mut out, &mut hw);
        let src = r#"
import gfx
gfx.pixel(1, 2)
gfx.pixel(3, 4)
gfx.line(0, 0, gfx.WIDTH - 1, 10, gfx.RED)
print(gfx.count(), gfx.WIDTH)

import gfx as g, clock
g.text('hi')
print(clock.now())

from gfx import pixel, line as ln, RED, WIDTH as W
pixel(5, 6)
ln(1, 2, 3, 4)
print(RED, W, gfx.count())

def draw(n):
    i = 0
    while i < n:
        pixel(i, i)
        i += 1
    return gfx.count()

print(draw(3))
f = gfx.pixel
f(0, 0)
print(gfx.count(), gfx, f, ln)
print(gfx == g, gfx == clock, f == gfx.pixel)
"#;
        vm.eval(src).unwrap();
        assert_eq!(
            out.as_str(),
            "2 240\n1234\n63488 240 3\n6\n7 <module 'gfx'> <function gfx.pixel> <function gfx.line>\nTrue False True\n"
        );
        assert_eq!(hw.last_line, [1, 2, 3, 4, 0xFFFF]);
        assert_eq!(&hw.text[..hw.text_len], b"hi");
    }

    #[test]
    fn test_import_errors() {
        let mut out = MockOut::new();
        let mut hw = ModuleHardware::new();
        let mut vm = Interpreter::with_hardware(&mut out, &mut hw);
        // Modules must be imported before use
        assert_eq!(vm.eval("gfx.pixel(1, 2)"), Err(Error::UnknownIdentifier));
        assert_eq!(vm.eval("import nope"), Err(Error::ImportError));
        assert_eq!(vm.eval("from nope import x"), Err(Error::ImportError));
        assert_eq!(vm.eval("from gfx import nope"), Err(Error::ImportError));
        vm.eval("import gfx\nx = 5").unwrap();
        assert_eq!(vm.eval("gfx.nope(1)"), Err(Error::AttributeError));
        assert_eq!(vm.eval("print(gfx.NOPE)"), Err(Error::AttributeError));
        assert_eq!(vm.eval("x.pixel(1, 2)"), Err(Error::AttributeError));
        // Argument counts come from the module table
        assert_eq!(vm.eval("gfx.pixel(1)"), Err(Error::TypeError));
        assert_eq!(vm.eval("gfx.line(1, 2, 3)"), Err(Error::TypeError));
        assert_eq!(vm.eval("gfx.line(1, 2, 3, 4, 5, 6)"), Err(Error::TypeError));
        assert_eq!(vm.eval("gfx.text(1)"), Err(Error::TypeError));
        assert_eq!(vm.eval("gfx + 1"), Err(Error::TypeError));
        assert_eq!(vm.error_line(), Some(1));

        // Without a bridge there are no modules
        let mut out = MockOut::new();
        let mut vm = Interpreter::new(&mut out);
        assert_eq!(vm.eval("import gfx"), Err(Error::ImportError));
    }

    #[test]
    fn test_module_help_and_repl_echo() {
        let mut out = MockOut::new();
        let mut hw = ModuleHardware::new();
        let mut vm = Interpreter::with_hardware(&mut out, &mut hw);
        vm.eval("import gfx\nhelp(gfx)").unwrap();
        vm.set_interactive(true);
        vm.eval("gfx").unwrap();
        vm.eval("gfx.WIDTH").unwrap();
        assert_eq!(vm.eval("help(5)"), Err(Error::TypeError));
        assert_eq!(
            out.as_str(),
            "Module gfx - Test drawing\n  gfx.pixel(x, y) - Set a pixel\n  \
             gfx.line(x0, y0, x1, y1, [color]) - Draw a line\n  gfx.text(s)\n  \
             gfx.count() - Pixels drawn\nConstants:\n  WIDTH RED\n<module 'gfx'>\n240\n"
        );

        let mut out = MockOut::new();
        let mut hw = ModuleHardware::new();
        Interpreter::with_hardware(&mut out, &mut hw)
            .eval("help()")
            .unwrap();
        assert!(out.as_str().ends_with(
            "Modules (import them, then help(module)):\n  binascii builtins os sys gfx clock\n"
        ));
    }

    #[test]
    fn test_global_statement() {
        let src = r#"
count = 0
total = 0

def press(n):
    global count, total
    count += 1
    total = total + n
    local = n * 2
    return local

def reset():
    global count
    count = 0

def create():
    global made
    made = 'new global'

press(5)
press(7)
print(count, total)
reset()
print(count, total)
create()
print(made)

def outer():
    x = 1
    inner()
    return x

def inner():
    global count
    count = 99
    y = 2

print(outer(), count)
"#;
        assert_output(src, "2 12\n0 12\nnew global\n1 99\n");
        // At the top level `global` does nothing
        assert_output("global q\nq = 3\nprint(q)", "3\n");

        // A `global` declaration only applies to the function that made it
        let (err, line) =
            run_err("n = 0\ndef a():\n    global n\n    n = 1\ndef b():\n    n += 1\na()\nb()");
        assert_eq!((err, line), (Error::UnknownIdentifier, Some(6)));
        // Creating a global while other calls are running keeps their locals intact
        assert_output(
            "def mk(v):\n    global g\n    g = v\ndef run(a):\n    b = a + 1\n    mk(a * 10)\n    return a + b\nprint(run(2), g)",
            "5 20\n",
        );
    }

    #[test]
    fn test_abs_min_max() {
        assert_eval("abs(-5)", "5");
        assert_eval("abs(7)", "7");
        assert_eval("abs(True)", "1");
        assert_eval("min(3, 1, 2)", "1");
        assert_eval("max(3, 1, 2)", "3");
        assert_eval("max(-1, -5)", "-1");
        assert_eval("min('b', 'a')", "a");
        assert_eval("max(2, 2)", "2");
        assert_eval_err("abs('x')", Error::TypeError);
        assert_eval_err("abs(1, 2)", Error::TypeError);
        assert_eval_err("min(1)", Error::TypeError);
        assert_eval_err("max()", Error::TypeError);
        assert_eval_err("max(1, 'a')", Error::TypeError);
        assert_eval_err("abs(-9223372036854775807 - 1)", Error::Overflow);
    }

    // ---- 5.1 strings ---------------------------------------------------

    #[test]
    fn test_string_operators_indexing_slicing() {
        assert_eval("'a' + 'b'", "ab");
        assert_eval("'ab' * 3", "ababab");
        assert_eval("3 * 'ab'", "ababab");
        assert_eval("'ab' * 0 + '|'", "|");
        assert_eval("'x' * -1 + '|'", "|");
        assert_eval("len('héllo')", "5");
        assert_eval("len('')", "0");
        assert_eval("'hello'[1]", "e");
        assert_eval("'hello'[-1]", "o");
        assert_eval("'hello'[1:3]", "el");
        assert_eval("'hello'[:2]", "he");
        assert_eval("'hello'[3:]", "lo");
        assert_eval("'hello'[-3:-1]", "ll");
        assert_eval("'hello'[4:1] + '|'", "|");
        assert_eval("'hello'[:]", "hello");
        assert_eval("'hello'[-100:100]", "hello");
        assert_eval("'héllo'[1]", "é");
        assert_eval("('ab' + 'cd')[1:3]", "bc");
        assert_eval("'ell' in 'hello'", "True");
        assert_eval("'z' not in 'hello'", "True");
        assert_eval("'' in 'abc'", "True");
        assert_eval("'a' < 'b' < 'c'", "True");
        assert_eval("'ab' == 'a' + 'b'", "True");
        assert_eval("'abc'[1] == 'b'", "True");
        assert_eval("not ''", "True");
        assert_eval("not ('a' + '')", "False");
    }

    #[test]
    fn test_string_escapes_str_repr() {
        assert_eval("'\\x41\\u00e9\\101'", "AéA");
        assert_eval("len('\\x00\\n')", "2");
        assert_eval("len('a\\\\b')", "3");
        assert_eval("'it\\'s'", "it's");
        assert_eval("\"say \\\"hi\\\"\"", "say \"hi\"");
        assert_eval("'\\q'", "\\q");
        assert_eval("repr('it\\'s')", "\"it's\"");
        assert_eval("repr('a\\nb')", "'a\\nb'");
        assert_eval("repr('tab\\there')", "'tab\\there'");
        assert_eval("repr('\\x01\\x7f')", "'\\x01\\x7f'");
        assert_eval("repr('q\"')", "'q\"'");
        assert_eval("repr(\"both ' and \\\"\")", "'both \\' and \"'");
        assert_eval("repr('back\\\\slash')", "'back\\\\slash'");
        assert_eval("repr('')", "''");
        assert_eval("str(42) + '!'", "42!");
        assert_eval("str(True)", "True");
        assert_eval("str(None) + str(-7)", "None-7");
        assert_eval("str('same')", "same");
        assert_eval("str()", "");
        assert_eval("repr(5)", "5");
        assert_eval("repr(None)", "None");
        assert_eval("repr('x') == \"'x'\"", "True");
        assert_eval("repr('a' + 'b')", "'ab'");
    }

    #[test]
    fn test_string_builtins_and_methods() {
        assert_eval("int('42') + int(' -7 ')", "35");
        assert_eval("int('0x1F', 16)", "31");
        assert_eval("int('ff', 16)", "255");
        assert_eval("int('0b101', 0)", "5");
        assert_eval("int('1_000')", "1000");
        assert_eval("int(True) + int(5)", "6");
        assert_eval("ord('A')", "65");
        assert_eval("chr(97) + chr(9786)", "a☺");
        assert_eval("'Hello'.upper()", "HELLO");
        assert_eval("'MiXeD'.lower()", "mixed");
        assert_eval("'straße'.upper()", "STRASSE");
        assert_eval("'  pad  '.strip() + '|'", "pad|");
        assert_eval("'xxhixx'.strip('x')", "hi");
        assert_eval("'  a '.lstrip() + '|'", "a |");
        assert_eval("' a  '.rstrip() + '|'", " a|");
        assert_eval("'   '.strip() + '|'", "|");
        assert_eval("'hello'.startswith('he')", "True");
        assert_eval("'hello'.endswith('lo')", "True");
        assert_eval("'hello'.find('l')", "2");
        assert_eval("'hello'.find('z')", "-1");
        assert_eval("'héllo'.find('l')", "2");
        assert_eval("'banana'.count('an')", "2");
        assert_eval("'abc'.count('')", "4");
        assert_eval("'a-b-c'.replace('-', '+')", "a+b+c");
        assert_eval("'aaa'.replace('a', 'bb')", "bbbbbb");
        assert_eval("'ab'.replace('', '-')", "-a-b-");
        assert_eval("'abc'.replace('x', 'y')", "abc");
        assert_eval("('a' + 'b').upper().lower().strip()", "ab");
        assert_eval("'123'.isdigit()", "True");
        assert_eval("''.isdigit()", "False");
        assert_eval("'abc'.isalpha()", "True");
        assert_eval("'A1'.isupper()", "True");
        assert_eval("'a1'.islower()", "True");
        assert_eval("' \\t'.isspace()", "True");
        assert_eval("'ab1'.isalnum()", "True");
    }

    #[test]
    fn test_string_errors() {
        assert_eval_err("'abc'[3]", Error::IndexError);
        assert_eval_err("'abc'[-4]", Error::IndexError);
        assert_eval_err("'abc'['x']", Error::TypeError);
        assert_eval_err("5[0]", Error::TypeError);
        assert_eval_err("len(5)", Error::TypeError);
        assert_eval_err("int('12x')", Error::ValueError);
        assert_eval_err("int('')", Error::ValueError);
        assert_eval_err("int('9223372036854775808')", Error::Overflow);
        assert_eval_err("chr(-1)", Error::ValueError);
        assert_eval_err("ord('ab')", Error::TypeError);
        assert_eval_err("'abc'.nope()", Error::AttributeError);
        assert_eval_err("'abc'.nope", Error::AttributeError);
        assert_eval("'abc'.upper", "<bound method upper>");
        assert_eval_err("'a' + 1", Error::TypeError);
        assert_eval_err("1 in 'abc'", Error::TypeError);
        assert_eval_err("'\\xZZ'", Error::InvalidSyntax);
        assert_eval_err("'a' < 1", Error::TypeError);
    }

    /// Runs a script with a heap and an 8-file file system in RAM.
    fn run_with_files(src: &str, heap_size: usize) -> (std::string::String, Result<(), Error>) {
        let mut flash = std::vec![0u8; 8 * crate::flatfs::SLOT_SIZE as usize];
        let files = RefCell::new(
            crate::flatfs::FlatFs::mount(crate::flatfs::RamFlash::new(&mut flash)).unwrap(),
        );
        let mut out = MockOut::new();
        let mut mem = std::vec![0u8; heap_size];
        let mut module_memory = std::vec![0u8; 16 * 1024];
        let mut vm = Interpreter::new(&mut out)
            .with_heap(&mut mem)
            .with_files(&files)
            .with_module_memory(&mut module_memory);
        let result = vm.eval(src);
        (out.as_str().into(), result)
    }

    /// Runs `src` with a heap of `size` bytes; returns output and heap stats.
    fn run_with_heap(src: &str, size: usize) -> (std::string::String, Result<(), Error>, u32) {
        let mut out = MockOut::new();
        let mut mem = std::vec![0u8; size];
        let mut vm = Interpreter::new(&mut out).with_heap(&mut mem);
        let result = vm.eval(src);
        let collections = vm.heap_stats().3;
        (out.as_str().into(), result, collections)
    }

    // ---- 5.2 bytes and binascii (expected output checked with CPython) ----

    #[test]
    fn test_bytes_literals_operators_methods() {
        assert_output(
            "b = b'hello'\nprint(b, len(b), b[0], b[-1], b[1:3], b[:0], b[-3:])\n\
             print(repr(b), str(b), b + b' world', b * 2, 2 * b'ab', b * 0)\n\
             print(b'\\x00\\xff\\n\\t\\\\', b\"it's\", b'say \"hi\"', b'both \\' and \"')\n\
             print(b'\\101\\x41', len(b'\\x00\\x01\\x02'), b'\\q', len(b'\\u263a'))\n\
             print(b == b'hello', b == 'hello', b < b'help', 104 in b, b'ell' in b, b'' in b)\n\
             print(b.hex(), b.decode(), 'za\u{17c}'.encode(), 'abc'.encode() == b'abc')\n\
             print(bytes(), bytes(3), bytes('\u{105}b', 'utf-8'), b.startswith(b'he'), b.find(b'l'))\n\
             x = b''\nx += bytes(1) + b'\\x41'\nprint(x, str(b'a' + b'\\x00'), repr(b'\\x7f\\x80'))\n\
             if b'':\n    print('no')\nif b:\n    print('yes')",
            "b'hello' 5 104 111 b'el' b'' b'llo'\n\
             b'hello' b'hello' b'hello world' b'hellohello' b'abab' b''\n\
             b'\\x00\\xff\\n\\t\\\\' b\"it's\" b'say \"hi\"' b'both \\' and \"'\n\
             b'AA' 3 b'\\\\q' 6\n\
             True False True True True True\n\
             68656c6c6f hello b'za\\xc5\\xbc' True\n\
             b'' b'\\x00\\x00\\x00' b'\\xc4\\x85b' True 2\n\
             b'\\x00A' b'a\\x00' b'\\x7f\\x80'\n\
             yes\n",
        );
    }

    #[test]
    fn test_binascii_module() {
        assert_output(
            "import binascii\nprint(binascii.hexlify(b'\\x00\\x10\\xfe'), binascii.unhexlify(b'48656c6c6f'))\n\
             print(binascii.unhexlify('00ff'), binascii.unhexlify(binascii.hexlify(b'round trip')))\n\
             print(binascii)",
            "b'0010fe' b'Hello'\nb'\\x00\\xff' b'round trip'\n<module 'binascii'>\n",
        );
        for (src, err) in [
            (
                "import binascii\nbinascii.unhexlify('abc')",
                Error::ValueError,
            ),
            (
                "import binascii\nbinascii.unhexlify('zz')",
                Error::ValueError,
            ),
            ("import binascii\nbinascii.hexlify('ab')", Error::TypeError),
            ("import binascii\nbinascii.hexlify()", Error::TypeError),
            ("b'a' + 'a'", Error::TypeError),
            ("b'a' < 'a'", Error::TypeError),
            ("b'a'[5]", Error::IndexError),
            ("300 in b'a'", Error::ValueError),
            ("'a' in b'a'", Error::TypeError),
            ("b'\\xff'.decode()", Error::ValueError),
            ("bytes(-1)", Error::ValueError),
            ("bytes('a')", Error::TypeError),
            ("b'\\x4'", Error::InvalidSyntax),
            ("b'\u{e9}'", Error::InvalidSyntax),
        ] {
            assert_eq!(run_err(src).0, err, "{}", src);
        }
    }

    /// Scripts in `tests/cpython/` must print exactly what CPython printed
    /// into the `.out` file next to them (`make cpython-expected` updates them).
    #[test]
    fn test_cpython_conformance() {
        macro_rules! case {
            ($name:literal) => {
                (
                    $name,
                    include_str!(concat!("../tests/cpython/", $name, ".py")),
                    include_str!(concat!("../tests/cpython/", $name, ".out")),
                )
            };
        }
        for (name, src, expected) in [
            case!("builtins_values"),
            case!("bytes"),
            case!("classes"),
            case!("closures"),
            case!("containers"),
            case!("exceptions"),
            case!("files"),
            case!("formatting"),
            case!("functions"),
            case!("modules"),
            case!("readme_examples"),
        ] {
            let (out, result) = run_with_files(src, 64 * 1024);
            assert_eq!(result, Ok(()), "{}.py, output so far:\n{}", name, out);
            assert_eq!(out, expected, "{}.py", name);
        }
    }

    // ---- 5.3 containers (expected output checked with CPython) ----------

    #[test]
    fn test_containers_basics() {
        assert_output(
            "a = [1, 2, 3]\nb = a\nb.append(4)\nprint(a, len(a), a[-1], a[1:3])\n\
             t = (1, 'two', (3,), [])\nprint(t, (), (5,), t[1])\n\
             d = {'a': 1, 'b': [1, 2], 3: 'x'}\nd['c'] = 5\nd['a'] = 10\n\
             print(d, d['b'], 'a' in d, d.get('z', 0))\n\
             for k, v in d.items():\n    print(k, v)\n\
             print(list(range(2, 10, 3)), range(3), [x * x for x in range(4)])\n\
             print({k: k * 2 for k in range(3) if k}, sum(x for x in range(10)))\n\
             x, y = 1, 2\nx, y = y, x\n(m, n) = [5, 6]\ni = j = 7\nprint(x, y, m, n, i, j)\n\
             print('a,b,,c'.split(','), ' a b '.split(), '-'.join(['a', 'b']))\n\
             print(sorted([(2, 'b'), (1, 'z')]), [1, 2] == [1, 2], (1, 2) < (1, 3))",
            "[1, 2, 3, 4] 4 4 [2, 3]\n(1, 'two', (3,), []) () (5,) two\n\
             {'a': 10, 'b': [1, 2], 3: 'x', 'c': 5} [1, 2] True 0\n\
             a 10\nb [1, 2]\n3 x\nc 5\n\
             [2, 5, 8] range(0, 3) [0, 1, 4, 9]\n{1: 2, 2: 4} 45\n\
             2 1 5 6 7 7\n['a', 'b', '', 'c'] ['a', 'b'] a-b\n\
             [(1, 'z'), (2, 'b')] True True\n",
        );
        // Loop variables of a comprehension don't leak (an existing one is kept)
        assert_output(
            "n = 'old'\nr = [n for n in range(3)]\nprint(n, r)",
            "old [0, 1, 2]\n",
        );
        assert_eq!(
            run_err("q = [z for z in 'ab']\nprint(z)").0,
            Error::UnknownIdentifier
        );
    }

    #[test]
    fn test_containers_errors() {
        for (src, err) in [
            ("[1][5]", Error::IndexError),
            ("{}['k']", Error::KeyError),
            ("{[1]: 2}", Error::TypeError),
            ("[1] + (2,)", Error::TypeError),
            ("[].pop()", Error::IndexError),
            ("a, b = [1, 2, 3]", Error::ValueError),
            ("a, b = 1", Error::TypeError),
            ("x = (1, 2)\nx[0] = 5", Error::TypeError),
            ("sorted([1, 'a'])", Error::TypeError),
            ("min([])", Error::ValueError),
            ("[x for x in 5]", Error::TypeError),
            ("for a, b in [1]:\n    pass", Error::TypeError),
            ("[x for x in y for y in z]", Error::InvalidSyntax),
            ("{1, 2}", Error::UnexpectedToken),
        ] {
            assert_eq!(run_err(src).0, err, "{}", src);
        }
    }

    #[test]
    fn test_gc_traces_containers() {
        // Lists, dicts and their strings survive collections in a 1 KB heap
        // while 300 temporary lists are created and dropped
        let src = "keep = {'name': 'x' * 10, 'items': [str(i) for i in range(5)]}\n\
                   i = 0\nwhile i < 300:\n    tmp = [i, str(i), (i, 'y' * 8)]\n    i += 1\n\
                   keep['items'].append('last')\nprint(keep, tmp)";
        let (out, result, collections) = run_with_heap(src, 1024);
        assert_eq!(result, Ok(()));
        assert_eq!(
            out,
            "{'name': 'xxxxxxxxxx', 'items': ['0', '1', '2', '3', '4', 'last']} \
             [299, '299', (299, 'yyyyyyyy')]\n"
        );
        assert!(collections > 5, "only {} collections", collections);

        // A list that contains itself is printed with a depth limit, and a
        // cycle is collected once unreachable
        let src = "a = [1]\na.append(a)\nprint(len(a), a[1][1][0])\na = None\n\
                   i = 0\nwhile i < 100:\n    t = [0] * 20\n    i += 1\nprint('ok')";
        let (out, result, _) = run_with_heap(src, 512);
        assert_eq!(result, Ok(()));
        assert_eq!(out, "2 1\nok\n");

        // A list growing past the heap fails cleanly
        let (_, result, _) = run_with_heap("a = []\nwhile True:\n    a.append(1)", 512);
        assert_eq!(result, Err(Error::OutOfMemory));
    }

    // ---- 6.1 bytes through the hardware bridge ------------------------------

    struct BusHardware {
        written: std::vec::Vec<u8>,
    }

    const BUS_FNS: &[crate::HwFn] = &[
        crate::HwFn::new("bus_write", "data", "Send bytes"),
        crate::HwFn::new("bus_read", "n", "Receive n bytes").returning_bytes(),
    ];
    const BUS_MODULE: &[crate::Module] = &[crate::Module {
        name: "bus",
        doc: "Test bus",
        functions: &[crate::HwFn::new("swap", "data", "Reversed").returning_bytes()],
        constants: &[],
    }];

    impl HardwareBridge for BusHardware {
        fn call_args(&mut self, name: &str, args: &[Arg<'_>]) -> Result<Option<i64>, Error> {
            match name {
                "bus_write" => {
                    let data = args[0].bytes()?;
                    self.written.extend_from_slice(data);
                    Ok(Some(data.len() as i64))
                }
                _ => Err(Error::UnknownIdentifier),
            }
        }

        fn call_bytes(
            &mut self,
            name: &str,
            args: &[Arg<'_>],
            out: &mut [u8],
        ) -> Result<usize, Error> {
            assert_eq!(name, "bus_read");
            let n = (args[0].int()? as usize).min(out.len());
            for (i, b) in out[..n].iter_mut().enumerate() {
                *b = (i as u8).wrapping_mul(3);
            }
            Ok(n)
        }

        fn call_module_bytes(
            &mut self,
            module: &str,
            name: &str,
            args: &[Arg<'_>],
            out: &mut [u8],
        ) -> Result<usize, Error> {
            assert_eq!((module, name), ("bus", "swap"));
            let data = args[0].bytes()?;
            for (o, b) in out.iter_mut().zip(data.iter().rev()) {
                *o = *b;
            }
            Ok(data.len())
        }

        fn functions(&self) -> &[crate::HwFn] {
            BUS_FNS
        }

        fn modules(&self) -> &[crate::Module] {
            BUS_MODULE
        }
    }

    #[test]
    fn test_bytes_through_the_bridge() {
        let mut out = MockOut::new();
        let mut hw = BusHardware {
            written: std::vec::Vec::new(),
        };
        let mut heap = [0u8; 4096];
        let mut vm = Interpreter::with_hardware(&mut out, &mut hw).with_heap(&mut heap);
        vm.eval(
            "print(bus_write(b'\\x01\\x02'), bus_write('AT\\r\\n'), bus_write(bytes([7]) if False else b'\\x07'))\n\
             data = bus_read(5)\nprint(data, len(data), data[4], bus_read(0), len(bus_read(9999)))\n\
             import bus\nprint(bus.swap(b'abc'), bus.swap(data))",
        )
        .unwrap();
        assert_eq!(vm.eval("bus_write(5)"), Err(Error::TypeError));
        let _ = vm;
        assert_eq!(
            out.as_str(),
            "2 4 1\nb'\\x00\\x03\\x06\\t\\x0c' 5 12 b'' 512\nb'cba' b'\\x0c\\t\\x06\\x03\\x00'\n"
        );
        assert_eq!(hw.written, b"\x01\x02AT\r\n\x07");
    }

    // ---- 6.3 importing .py modules ------------------------------------------

    #[test]
    fn test_module_memory_limits() {
        let mut flash = std::vec![0u8; 4 * crate::flatfs::SLOT_SIZE as usize];
        let files = RefCell::new(
            crate::flatfs::FlatFs::mount(crate::flatfs::RamFlash::new(&mut flash)).unwrap(),
        );
        crate::FileSystem::create(&mut *files.borrow_mut(), "m.py", false).unwrap();
        crate::FileSystem::write_file(&mut *files.borrow_mut(), 1, b"VALUE = 42\n").unwrap();
        crate::FileSystem::close_file(&mut *files.borrow_mut(), 1).unwrap();

        // Without module memory: only native modules
        let mut out = MockOut::new();
        let mut heap = [0u8; 4096];
        let mut vm = Interpreter::new(&mut out)
            .with_heap(&mut heap)
            .with_files(&files);
        assert_eq!(vm.eval("import m"), Err(Error::OutOfMemory));
        // Too small for the module, then a second one that fits
        let mut small = [0u8; 4];
        vm.set_module_memory(&mut small);
        assert_eq!(vm.eval("import m"), Err(Error::OutOfMemory));
        let mut enough = [0u8; 64];
        vm.set_module_memory(&mut enough);
        assert_eq!(vm.eval("import m\nprint(m.VALUE, m)"), Ok(()));
        assert_eq!(vm.eval("import nothing_here"), Err(Error::ImportError));
        let _ = vm;
        assert_eq!(out.as_str(), "42 <module 'm'>\n");
    }

    // ---- 5.8 classes and closures -----------------------------------------

    #[test]
    fn test_function_slots_are_collected() {
        // Every run of a class statement makes its methods again, and every
        // call of `make` a new closure: unreachable ones must free their slots
        let mut out = MockOut::new();
        let mut heap = [0u8; 8192];
        let mut vm = Interpreter::new(&mut out).with_heap(&mut heap);
        for _ in 0..50 {
            vm.eval("class P:\n    def __init__(self, x):\n        self.x = x\n    def get(self):\n        return self.x\np = P(7)")
                .unwrap();
        }
        vm.eval("def make(n):\n    return lambda: n\nfs = []\nfor i in range(100):\n    f = make(i)\n    if i % 30 == 0:\n        fs.append(f)\nprint(p.get(), [f() for f in fs])")
            .unwrap();
        assert!(vm.funcs.iter().flatten().count() < MAX_FUNCS);
        let _ = vm;
        assert_eq!(out.as_str(), "7 [0, 30, 60, 90]\n");
    }

    #[test]
    fn test_stack_floor_raises_recursion_error() {
        let mut out = MockOut::new();
        let mut heap = [0u8; 4096];
        let mut vm = Interpreter::new(&mut out).with_heap(&mut heap);
        vm.eval("def f(n):\n    if n:\n        if n:\n            return f(n - 1)\n    return [[[[1]]]]")
            .unwrap();
        // The lowest floor (in 4 KB steps) at which one call still works...
        let here = super::stack_address();
        let mut room = 4096;
        loop {
            vm.set_stack_floor(Some(here - room));
            if vm.eval("x = f(0)").is_ok() {
                break;
            }
            room += 4096;
            assert!(room < 4 * 1024 * 1024, "no floor works");
        }
        // ...stops deep recursion with a RecursionError that can be caught
        vm.eval("try:\n    f(7)\nexcept RecursionError:\n    print('caught')\nprint(f(0))")
            .unwrap();
        vm.set_stack_floor(None);
        vm.eval("print(f(7))").unwrap();
        let _ = vm;
        assert_eq!(out.as_str(), "caught\n[[[[1]]]]\n[[[[1]]]]\n");
    }

    #[test]
    fn test_class_errors() {
        for (src, err) in [
            ("def f():\n    class C:\n        pass", Error::InvalidSyntax),
            ("class C(5):\n    pass", Error::TypeError),
            ("class C:\n    pass\nC(1)", Error::TypeError),
            (
                "class C:\n    def __init__(self):\n        return 5\nC()",
                Error::TypeError,
            ),
            ("class C:\n    pass\nC().nope", Error::AttributeError),
            ("x = 5\nx.attr = 1", Error::AttributeError),
            ("@staticmethod\nx = 1", Error::InvalidSyntax),
            ("def f(*a, b):\n    pass", Error::InvalidSyntax),
            ("def f(**k, *a):\n    pass", Error::InvalidSyntax),
        ] {
            assert_eq!(run_err(src).0, err, "{}", src);
        }
        assert_eq!(
            run_err("def f(**k):\n    pass\nf(**{1: 2})").0,
            Error::TypeError
        );
        let (err, _) = run_err("super()");
        assert_eq!(err, Error::Exception); // RuntimeError
    }

    // ---- 5.6 files ---------------------------------------------------------

    #[test]
    fn test_os_module_specifics() {
        let (out, result) = run_with_files(
            "import os\nf = open('a.py', 'w')\nf.write('x = 1')\nf.close()\n\
             print(os.size('a.py'), os.listdir('/'), os.listdir('.'), os.stat('/')[0] == 0o040000)\n\
             print(os.statvfs('/')[2:4], os.uname()[0], open('/a.py').read())\n\
             try:\n    os.listdir('sub')\nexcept OSError as e:\n    print(e)\n\
             try:\n    open('bad name!', 'w')\nexcept OSError as e:\n    print(e.errno)\n\
             # opening a second file for writing saves the first one\n\
             f1 = open('one.txt', 'w')\nf1.write('1')\nf2 = open('two.txt', 'w')\n\
             print(os.listdir())\n\
             try:\n    f1.write('more')\nexcept OSError as e:\n    print(e)\n\
             f2.close()\nprint(sorted(os.listdir()), open('one.txt').read())",
            8192,
        );
        assert_eq!(result, Ok(()), "{}", out);
        assert_eq!(
            out,
            "5 ['a.py'] ['a.py'] True\n(8, 7) micropython-rs x = 1\n\
             [Errno 2] ENOENT: no such file\n22\n['a.py', 'one.txt']\n\
             [Errno 9] EBADF: file not open for writing\n['a.py', 'one.txt', 'two.txt'] 1\n"
        );

        // Without a file system: OSError (ENODEV)
        let (err, _) = run_err("import os\ntry:\n    os.listdir()\nexcept OSError as e:\n    raise ValueError(e.errno)");
        assert_eq!(err, Error::Exception);
        let mut out = MockOut::new();
        let mut heap = [0u8; 2048];
        let mut vm = Interpreter::new(&mut out).with_heap(&mut heap);
        assert_eq!(
            vm.eval("try:\n    open('x')\nexcept OSError as e:\n    print(e.errno)"),
            Ok(())
        );
        assert_eq!(out.as_str(), "19\n");
    }

    #[test]
    fn test_file_written_across_evals_and_collections() {
        // Thonny 3.2.7 writes a file with one `eval` per block; the file
        // object lives in a variable while the heap is collected in between
        let mut flash = std::vec![0u8; 4 * crate::flatfs::SLOT_SIZE as usize];
        let files = RefCell::new(
            crate::flatfs::FlatFs::mount(crate::flatfs::RamFlash::new(&mut flash)).unwrap(),
        );
        let mut out = MockOut::new();
        let mut heap = [0u8; 1024];
        let mut vm = Interpreter::new(&mut out)
            .with_heap(&mut heap)
            .with_files(&files);
        vm.eval("__thonny_written = 0\n__thonny_fp = open('main.py', 'wb')")
            .unwrap();
        vm.eval("from binascii import unhexlify as __thonny_unhex\ndef __W(x):\n    global __thonny_written\n    __thonny_written += __thonny_fp.write(__thonny_unhex(x))\n    __thonny_fp.flush()")
            .unwrap();
        for _ in 0..20 {
            vm.eval("__W(b'7072696e7428313233290a')").unwrap(); // print(123)\n
            vm.eval("junk = [str(i) * 5 for i in range(10)]").unwrap();
        }
        vm.eval("print(__thonny_written)\ntry:\n    del __W\n    __thonny_fp.close()\n    del __thonny_fp\nexcept:\n    pass")
            .unwrap();
        vm.eval("with open('main.py') as f:\n    lines = f.readlines()\nprint(len(lines), lines[0].strip())")
            .unwrap();
        assert!(vm.heap_stats().3 > 0);
        assert_eq!(out.as_str(), "220\n20 print(123)\n");
    }

    // ---- 5.4 exceptions ---------------------------------------------------

    #[test]
    fn test_exceptions_uncatchable_and_errors() {
        // Syntax errors aren't caught, even inside `try`
        let (err, line) = run_err("try:\n    x = 1 +\nexcept:\n    pass");
        assert_eq!((err, line), (Error::UnexpectedToken, Some(2)));
        let (err, _) = run_err("try:\n    pass\nprint(1)");
        assert_eq!(err, Error::InvalidSyntax); // no except/finally
        assert_eq!(run_err("except:\n    pass").0, Error::InvalidSyntax);
        assert_eq!(run_err("finally:\n    pass").0, Error::InvalidSyntax);
        assert_eq!(run_err("raise 5").0, Error::TypeError);

        // Ctrl-C (KeyboardInterrupt) passes a bare `except:` but can be named
        assert_eq!(
            run_err("try:\n    raise KeyboardInterrupt\nexcept:\n    pass").0,
            Error::Exception
        );
        assert_output(
            "try:\n    raise KeyboardInterrupt\nexcept KeyboardInterrupt:\n    print('stop')",
            "stop\n",
        );
        assert_output(
            "try:\n    raise KeyboardInterrupt\nexcept Exception:\n    print('no')\nexcept BaseException:\n    print('base')",
            "base\n",
        );

        // The step limit can't be caught either
        let mut out = MockOut::new();
        let mut vm = Interpreter::new(&mut out);
        vm.set_step_limit(Some(50));
        assert_eq!(
            vm.eval("try:\n    while True:\n        pass\nexcept:\n    print('caught')"),
            Err(Error::StepLimit)
        );
        assert_eq!(out.as_str(), "");
    }

    #[test]
    fn test_uncaught_exception_line_and_text() {
        struct Text(std::string::String);
        impl Output for Text {
            fn write_str(&mut self, s: &str) {
                self.0.push_str(s);
            }
        }
        let mut out = MockOut::new();
        let mut heap = [0u8; 2048];
        let mut vm = Interpreter::new(&mut out).with_heap(&mut heap);
        let src = "x = 1\ntry:\n    y = 2\n    raise ValueError('bad', x)\nfinally:\n    z = 3\n";
        assert_eq!(vm.eval(src), Err(Error::Exception));
        assert_eq!(vm.error_line(), Some(4));
        let mut text = Text(std::string::String::new());
        vm.write_exception(Error::Exception, &mut text);
        assert_eq!(text.0, "ValueError: ('bad', 1)");

        // A caught error doesn't leave a stale line behind
        assert_eq!(
            vm.eval("try:\n    1 // 0\nexcept ZeroDivisionError:\n    pass\nx = y_missing"),
            Err(Error::UnknownIdentifier)
        );
        assert_eq!(vm.error_line(), Some(5));
        let mut text = Text(std::string::String::new());
        vm.write_exception(Error::ZeroDivision, &mut text);
        assert_eq!(text.0, "ZeroDivisionError: division by zero");
        let mut text = Text(std::string::String::new());
        vm.write_exception(Error::Interrupted, &mut text);
        assert_eq!(text.0, "KeyboardInterrupt");

        // A bare `raise` outside a handler
        assert_eq!(vm.eval("raise"), Err(Error::Exception));
        let mut text = Text(std::string::String::new());
        vm.write_exception(Error::Exception, &mut text);
        assert_eq!(text.0, "RuntimeError: No active exception to reraise");
    }

    #[test]
    fn test_gc_keeps_pending_exceptions() {
        // The exception waits while `finally` makes lots of garbage
        let src = "def f():\n    try:\n        raise ValueError('k' * 20)\n    finally:\n        \
                   i = 0\n        while i < 100:\n            t = [str(i)] * 3\n            i += 1\n\
                   try:\n    f()\nexcept ValueError as e:\n    print(e)\n\
                   def g():\n    try:\n        return 'r' * 30\n    finally:\n        \
                   j = 0\n        while j < 100:\n            u = 'u' * 40\n            j += 1\n\
                   print(g())";
        let (out, result, collections) = run_with_heap(src, 512);
        assert_eq!(result, Ok(()));
        assert_eq!(
            out,
            std::format!("{}\n{}\n", "k".repeat(20), "r".repeat(30))
        );
        assert!(collections > 5, "only {} collections", collections);
    }

    #[test]
    fn test_gc_with_bytes() {
        // hexlify/unhexlify round trips in a small heap: the inputs must
        // survive the collections that the outputs cause
        let src = "import binascii\nkeep = b'\\x01' * 8\ni = 0\nok = 0\nwhile i < 100:\n    h = binascii.hexlify(keep + bytes(1))\n    if binascii.unhexlify(h.decode()) == keep + b'\\x00':\n        ok += 1\n    i += 1\nprint(ok, h, keep.hex())";
        let (out, result, collections) = run_with_heap(src, 192);
        assert_eq!(result, Ok(()));
        assert_eq!(out, "100 b'010101010101010100' 0101010101010101\n");
        assert!(collections > 5, "only {} collections", collections);
    }

    #[test]
    fn test_gc_reclaims_garbage_and_keeps_live_strings() {
        // 200 strings of 23 bytes in a 256-byte heap: only works with collection
        let src = "keep = 'k' + 'eep'\ni = 0\nwhile i < 200:\n    tmp = 'x' * 20 + str(i)\n    i += 1\nprint(keep, tmp[-3:], len(tmp))";
        let (out, result, collections) = run_with_heap(src, 256);
        assert_eq!(result, Ok(()));
        assert_eq!(out, "keep 199 23\n");
        assert!(collections > 5, "only {} collections", collections);

        // A string only on the value stack survives a collection caused by a call
        let src = "def junk(n):\n    i = 0\n    while i < n:\n        t = 'y' * 30\n        i += 1\n    return 'r'\ns = 'a' * 10 + junk(50) + 'b' * 10\nprint(s)";
        let (out, result, collections) = run_with_heap(src, 256);
        assert_eq!(result, Ok(()));
        assert_eq!(out, "aaaaaaaaaarbbbbbbbbbb\n");
        assert!(collections > 0);

        // Growing a string with += in a loop
        let src = "s = ''\ni = 0\nwhile i < 50:\n    s += 'ab'\n    i += 1\nprint(len(s), s[:6])";
        let (out, result, _) = run_with_heap(src, 512);
        assert_eq!(result, Ok(()));
        assert_eq!(out, "100 ababab\n");
    }

    #[test]
    fn test_heap_limits() {
        // Live data larger than the heap
        let (_, result, _) = run_with_heap("a = 'x' * 50\nb = 'y' * 50", 64);
        assert_eq!(result, Err(Error::OutOfMemory));
        // Without a heap, source strings (and slices of them) still work
        let mut out = MockOut::new();
        let mut vm = Interpreter::new(&mut out);
        assert_eq!(
            vm.eval("print('hello'[1:3], len('abc'), 'b' in 'abc')"),
            Ok(())
        );
        assert_eq!(vm.eval("x = 'a' + 'b'"), Err(Error::OutOfMemory));
        assert_eq!(vm.eval("x = '\\n'"), Err(Error::OutOfMemory));
        let (used, capacity, objects, _) = vm.heap_stats();
        assert_eq!((used, capacity, objects), (0, 0, 0));
        assert_eq!(out.as_str(), "el 3 True\n");
    }

    #[test]
    fn test_heap_strings_with_hardware_and_repl() {
        let mut out = MockOut::new();
        let mut hw = TableHardware {
            last_text: [0; 16],
            text_len: 0,
            pins: [0; 4],
        };
        let mut mem = [0u8; 256];
        let mut vm = Interpreter::with_hardware(&mut out, &mut hw).with_heap(&mut mem);
        vm.eval("text('val=' + str(4 * 10))").unwrap();
        vm.set_interactive(true);
        vm.eval("'a\\tb' + '!'").unwrap();
        vm.eval("str(7)").unwrap();
        assert_eq!(&hw.last_text[..hw.text_len], b"val=40");
        assert_eq!(out.as_str(), "'a\\tb!'\n'7'\n");
    }

    #[test]
    fn test_unknown_variable_error() {
        let mut out = MockOut::new();
        let mut vm = Interpreter::new(&mut out);

        assert_eq!(
            vm.eval("print(undefined_var)"),
            Err(Error::UnknownIdentifier)
        );
        assert_eq!(vm.eval("x = missing"), Err(Error::UnknownIdentifier));
        assert_eq!(vm.eval("x = 1 + missing"), Err(Error::UnknownIdentifier));
    }

    #[test]
    fn test_unknown_function_error() {
        let mut out = MockOut::new();
        let mut vm = Interpreter::new(&mut out);

        assert_eq!(vm.eval("unknown_func(123)"), Err(Error::UnknownIdentifier));
    }

    #[test]
    fn test_out_of_memory_when_exceeding_max_vars() {
        let mut out = MockOut::new();
        let mut vm = Interpreter::new(&mut out);

        let fill_script: std::string::String = (0..MAX_VARS)
            .map(|i| std::format!("v{} = 1\n", i))
            .collect();
        // The source must outlive the interpreter's use of it
        let fill_script: &'static str = std::boxed::Box::leak(fill_script.into_boxed_str());
        assert_eq!(vm.eval(fill_script), Ok(()));
        assert_eq!(vm.eval("v0 = 99"), Ok(()));
        assert_eq!(vm.eval("overflow = 1"), Err(Error::OutOfMemory));
    }

    #[test]
    fn test_print_zero_and_empty_call() {
        let mut out = MockOut::new();
        let mut vm = Interpreter::new(&mut out);

        vm.eval("print(0)\nprint()").unwrap();
        assert_eq!(out.as_str(), "0\n\n");
    }

    #[test]
    fn test_propagates_lexer_and_parser_errors() {
        let mut out = MockOut::new();
        let mut vm = Interpreter::new(&mut out);

        assert_eq!(
            vm.eval("print('unterminated"),
            Err(Error::UnterminatedString)
        );
        assert_eq!(vm.eval("123 = 456"), Err(Error::InvalidSyntax));
        assert_eq!(vm.eval("x = "), Err(Error::UnexpectedToken));
    }
}
