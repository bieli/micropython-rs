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
// Memory use is fixed: no heap, and native recursion is bounded by
// `MAX_NESTING` (nested blocks) and `MAX_CALL_DEPTH` (nested calls).

use core::cmp::Ordering as CmpOrdering;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::ast_parser::{
    BinOp, CmpOp, Code, Expr, Item, Parser, Stmt, MAX_CALL_ARGS, MAX_EXPR_ITEMS, MAX_PARAMS,
};
use crate::{Arg, Error, HardwareBridge, Module, NoHardware, Output};

/// Variable slots, shared by globals and the locals of active function calls.
pub const MAX_VARS: usize = 32;
/// Number of `def` functions that can exist at the same time.
pub const MAX_FUNCS: usize = 8;
/// Maximum depth of nested user function calls (recursion included).
pub const MAX_CALL_DEPTH: usize = 8;
/// Maximum nesting of blocks (`if`/`while`/function bodies) being run at once.
pub const MAX_NESTING: usize = 24;
/// RPN evaluation stack depth. With calls, every item of an expression can be
/// an operand waiting on the stack (`f(1, 2, 3, ...)`), so it matches the item limit.
const MAX_EVAL_STACK: usize = MAX_EXPR_ITEMS;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value<'a> {
    None,
    Bool(bool),
    Int(i64),
    Str(&'a str),
    /// An imported native module (index into `HardwareBridge::modules()`).
    Module(u8),
    /// A function of a native module, e.g. after `from display import line`.
    NativeFn {
        module: u8,
        func: u8,
    },
}

impl Value<'_> {
    /// Python truth value: `None`, `False`, `0` and `''` are false.
    fn is_truthy(self) -> bool {
        match self {
            Value::None => false,
            Value::Bool(b) => b,
            Value::Int(n) => n != 0,
            Value::Str(s) => !s.is_empty(),
            Value::Module(_) | Value::NativeFn { .. } => true,
        }
    }

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
}

/// A `def` function: its signature and a parser positioned at its body.
#[derive(Debug, Clone, Copy)]
struct FuncDef<'a> {
    name: &'a str,
    params: [&'a str; MAX_PARAMS],
    count: usize,
    /// Indentation of the `def` line; the body is indented deeper.
    indent: u32,
    body: Parser<'a>,
}

pub struct Interpreter<'a, O: Output, H: HardwareBridge = NoHardware> {
    out: &'a mut O,
    hw: Option<&'a mut H>,
    /// Globals first, then one frame of locals per active function call.
    vars: [(&'a str, Value<'a>); MAX_VARS],
    nvars: usize,
    /// First variable slot of the innermost function call.
    frame_base: usize,
    /// Number of globals while a function call is active.
    globals_len: usize,
    call_depth: usize,
    nesting: usize,
    funcs: [Option<FuncDef<'a>>; MAX_FUNCS],
    /// Line of the last error returned by `eval`, if any.
    error_line: Option<u32>,
    /// Maximum statements one `eval` call may execute (`None` = unlimited).
    step_limit: Option<u32>,
    steps: u32,
    /// Set from outside (e.g. a UART Ctrl-C handler) to stop a running script.
    abort: Option<&'a AtomicBool>,
    /// REPL mode: top-level expression statements print their value.
    interactive: bool,
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
            frame_base: 0,
            globals_len: 0,
            call_depth: 0,
            nesting: 0,
            funcs: [None; MAX_FUNCS],
            error_line: None,
            step_limit: None,
            steps: 0,
            abort: None,
            interactive: false,
        }
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

    fn next_indent(&mut self, parser: &mut Parser<'a>) -> Result<Option<u32>, Error> {
        let result = parser.next_indent();
        if result.is_err() {
            self.note_error(parser.line());
        }
        result
    }

    /// Runs (or, with `run == false`, only parses) the statements of a block
    /// whose statements are indented by exactly `indent` columns.
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
    fn exec_body(
        &mut self,
        parser: &mut Parser<'a>,
        header_indent: u32,
        run: bool,
        ctx: Ctx,
        code: &mut Code<'a>,
    ) -> Result<Flow<'a>, Error> {
        if self.nesting >= MAX_NESTING {
            self.note_error(parser.line());
            return Err(Error::OutOfMemory);
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
            Stmt::AugAssign { name, op, expr } => {
                if run {
                    // Like Python, `x += 1` needs `x` in the current scope
                    // (a global can't be updated from inside a function).
                    let current = self.get_scope_var(name)?;
                    let rhs = self.eval_expr(code, expr)?;
                    let val = binop(op, current, rhs)?;
                    self.set_var(name, val)?;
                }
            }
            Stmt::Expr(expr) => {
                if run {
                    let val = self.eval_expr(code, expr)?;
                    if self.interactive && self.call_depth == 0 && val != Value::None {
                        self.print_repr(val);
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
            Stmt::Elif(_) | Stmt::Else => return Err(Error::InvalidSyntax),
            Stmt::If(cond) => return self.exec_if(cond, parser, indent, run, ctx, code),
            Stmt::While(cond) => {
                return self.exec_while(cond, start, parser, indent, run, ctx, code)
            }
            Stmt::Import { names, count } => {
                if run {
                    for &(module, bound) in &names[..count] {
                        let index = self.find_module(module).ok_or(Error::ImportError)?;
                        self.set_var(bound, Value::Module(index))?;
                    }
                }
            }
            Stmt::FromImport {
                module,
                names,
                count,
            } => {
                if run {
                    let index = self.find_module(module).ok_or(Error::ImportError)?;
                    for &(name, bound) in &names[..count] {
                        let val = self
                            .module_attr(index, name)
                            .map_err(|_| Error::ImportError)?;
                        self.set_var(bound, val)?;
                    }
                }
            }
            Stmt::Def {
                name,
                params,
                count,
            } => {
                // Nested functions would need closures; keep functions top-level
                if ctx.in_func {
                    return Err(Error::InvalidSyntax);
                }
                let func = FuncDef {
                    name,
                    params,
                    count,
                    indent,
                    body: *parser,
                };
                // Parse (but don't run) the body to find where it ends
                let body_ctx = Ctx {
                    in_loop: false,
                    in_func: true,
                };
                self.exec_body(parser, indent, false, body_ctx, code)?;
                if run {
                    self.define_func(func)?;
                }
            }
        }
        Ok(Flow::Normal)
    }

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
        let mut taken = run && self.eval_expr(code, cond)?.is_truthy();
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
                    val?.is_truthy()
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
            let go = run && self.eval_expr(code, cond)?.is_truthy();
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

    // ------------------------------------------------------------------
    // Variables and functions
    // ------------------------------------------------------------------

    /// Slots of the current scope: all globals at the top level, or the
    /// locals of the innermost function call.
    fn scope_start(&self) -> usize {
        if self.call_depth == 0 {
            0
        } else {
            self.frame_base
        }
    }

    fn find_in(&self, range: core::ops::Range<usize>, name: &str) -> Option<usize> {
        range.into_iter().find(|&i| self.vars[i].0 == name)
    }

    fn set_var(&mut self, name: &'a str, val: Value<'a>) -> Result<(), Error> {
        match self.find_in(self.scope_start()..self.nvars, name) {
            Some(i) => self.vars[i].1 = val,
            None => self.push_var(name, val)?,
        }
        Ok(())
    }

    fn push_var(&mut self, name: &'a str, val: Value<'a>) -> Result<(), Error> {
        if self.nvars >= MAX_VARS {
            return Err(Error::OutOfMemory);
        }
        self.vars[self.nvars] = (name, val);
        self.nvars += 1;
        Ok(())
    }

    /// Looks a name up in the current scope only.
    fn get_scope_var(&self, name: &str) -> Result<Value<'a>, Error> {
        self.find_in(self.scope_start()..self.nvars, name)
            .map(|i| self.vars[i].1)
            .ok_or(Error::UnknownIdentifier)
    }

    /// Looks a name up in the current scope, then in the globals.
    /// Looks a name up in the current scope, then in the globals, then in
    /// the hardware constants (`OUT`, `IN`, ...).
    fn get_var(&self, name: &str) -> Result<Value<'a>, Error> {
        self.get_scope_var(name)
            .or_else(|err| {
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
    }

    fn define_func(&mut self, func: FuncDef<'a>) -> Result<(), Error> {
        let slot = match self
            .funcs
            .iter()
            .position(|f| f.is_some_and(|f| f.name == func.name))
        {
            Some(i) => i,
            None => self
                .funcs
                .iter()
                .position(Option::is_none)
                .ok_or(Error::OutOfMemory)?,
        };
        self.funcs[slot] = Some(func);
        Ok(())
    }

    fn find_func(&self, name: &str) -> Option<FuncDef<'a>> {
        self.funcs
            .iter()
            .flatten()
            .find(|f| f.name == name)
            .copied()
    }

    /// Calls a `def` function: binds the arguments as new locals and runs the body.
    fn call_user_func(
        &mut self,
        func: FuncDef<'a>,
        args: &[Value<'a>],
    ) -> Result<Value<'a>, Error> {
        if args.len() != func.count {
            return Err(Error::TypeError);
        }
        if self.call_depth >= MAX_CALL_DEPTH {
            return Err(Error::RecursionLimit);
        }

        let saved_base = self.frame_base;
        if self.call_depth == 0 {
            self.globals_len = self.nvars;
        }
        let base = self.nvars;
        for (&param, &val) in func.params[..func.count].iter().zip(args) {
            if let Err(err) = self.push_var(param, val) {
                self.nvars = base;
                return Err(err);
            }
        }
        self.frame_base = base;
        self.call_depth += 1;

        let mut parser = func.body;
        let mut code = Code::new();
        let body_ctx = Ctx {
            in_loop: false,
            in_func: true,
        };
        let result = self.exec_body(&mut parser, func.indent, true, body_ctx, &mut code);

        // Drop the locals and return to the caller's scope
        self.call_depth -= 1;
        self.frame_base = saved_base;
        self.nvars = base;

        Ok(match result? {
            Flow::Return(val) => val,
            _ => Value::None,
        })
    }

    // ------------------------------------------------------------------
    // Expressions
    // ------------------------------------------------------------------

    /// Evaluates one RPN expression with a fixed-size value stack.
    fn eval_expr(&mut self, code: &Code<'a>, expr: Expr) -> Result<Value<'a>, Error> {
        let items = code.items(expr);
        let mut stack = [Value::None; MAX_EVAL_STACK];
        let mut sp = 0;
        let mut i = 0;

        while i < items.len() {
            let item = items[i];
            i += 1;
            let val = match item {
                Item::Int(n) => Value::Int(n),
                Item::Str(s) => Value::Str(s),
                Item::Bool(b) => Value::Bool(b),
                Item::None => Value::None,
                Item::Ident(name) => self.get_var(name)?,
                Item::Neg | Item::Invert => {
                    sp -= 1;
                    let n = stack[sp].as_int().ok_or(Error::TypeError)?;
                    Value::Int(if item == Item::Neg {
                        n.checked_neg().ok_or(Error::Overflow)?
                    } else {
                        !n // `~n` == `-n - 1`, never overflows
                    })
                }
                Item::Not => {
                    sp -= 1;
                    Value::Bool(!stack[sp].is_truthy())
                }
                Item::Bin(op) => {
                    sp -= 2;
                    binop(op, stack[sp], stack[sp + 1])?
                }
                Item::Cmp(op) => {
                    sp -= 2;
                    Value::Bool(compare(op, stack[sp], stack[sp + 1])?)
                }
                Item::CmpChain { op, skip } => {
                    sp -= 2;
                    let rhs = stack[sp + 1];
                    if compare(op, stack[sp], rhs)? {
                        rhs // left operand of the next comparison
                    } else {
                        i += skip as usize;
                        Value::Bool(false)
                    }
                }
                Item::AndJump { skip } | Item::OrJump { skip } => {
                    let truthy = stack[sp - 1].is_truthy();
                    let is_and = matches!(item, Item::AndJump { .. });
                    if truthy != is_and {
                        // Result decided: keep this operand, skip the right side
                        i += skip as usize;
                    } else {
                        sp -= 1;
                    }
                    continue;
                }
                Item::Call { name, argc } => {
                    sp -= argc as usize;
                    let mut args = [Value::None; MAX_CALL_ARGS];
                    args[..argc as usize].copy_from_slice(&stack[sp..sp + argc as usize]);
                    self.call_function(name, &args[..argc as usize])?
                }
                Item::GetAttr(attr) => {
                    sp -= 1;
                    let module = as_module(stack[sp])?;
                    self.module_attr(module, attr)?
                }
                Item::CallAttr { attr, argc } => {
                    sp -= argc as usize;
                    let mut args = [Value::None; MAX_CALL_ARGS];
                    args[..argc as usize].copy_from_slice(&stack[sp..sp + argc as usize]);
                    sp -= 1;
                    let module = as_module(stack[sp])?;
                    let func = self
                        .module_func(module, attr)
                        .ok_or(Error::AttributeError)?;
                    self.call_native(module, func, &args[..argc as usize])?
                }
            };
            if sp >= MAX_EVAL_STACK {
                return Err(Error::OutOfMemory);
            }
            stack[sp] = val;
            sp += 1;
        }

        // The parser only emits well-formed expressions, which leave exactly one value.
        if sp != 1 {
            return Err(Error::InvalidSyntax);
        }
        Ok(stack[0])
    }

    /// Calls a `def` function, the built-in `print`, or a hardware function.
    /// Arguments are already evaluated, so a failing argument means nothing is
    /// printed or sent to hardware (like Python).
    fn call_function(&mut self, name: &str, args: &[Value<'a>]) -> Result<Value<'a>, Error> {
        // A variable holding a module function (`from display import line`)
        if let Ok(Value::NativeFn { module, func }) = self.get_var(name) {
            return self.call_native(module, func, args);
        }
        if let Some(func) = self.find_func(name) {
            return self.call_user_func(func, args);
        }

        if name == "print" {
            for (i, val) in args.iter().enumerate() {
                if i > 0 {
                    self.out.write_str(" ");
                }
                self.print_value(*val);
            }
            self.out.write_str("\n");
            return Ok(Value::None);
        }

        if name == "help" {
            match args {
                [] => self.print_help(),
                [Value::Module(module)] => self.print_module_help(*module),
                _ => return Err(Error::TypeError),
            }
            return Ok(Value::None);
        }

        let hw = self.hw.as_mut().ok_or(Error::UnknownIdentifier)?;

        // Check the name and argument count against the bridge's function table
        let table = hw.functions();
        if !table.is_empty() {
            let spec = table
                .iter()
                .find(|f| f.name == name)
                .ok_or(Error::UnknownIdentifier)?;
            if args.len() < spec.min_args as usize || args.len() > spec.max_args as usize {
                return Err(Error::TypeError);
            }
        }

        let mut hw_args = [Arg::Int(0); MAX_CALL_ARGS];
        for (slot, val) in hw_args.iter_mut().zip(args) {
            *slot = match *val {
                Value::Str(s) => Arg::Str(s),
                other => Arg::Int(other.as_int().ok_or(Error::TypeError)?),
            };
        }

        Ok(match hw.call_args(name, &hw_args[..args.len()])? {
            Some(n) => Value::Int(n),
            None => Value::None,
        })
    }

    // ------------------------------------------------------------------
    // Native modules
    // ------------------------------------------------------------------

    fn modules(&self) -> &[Module] {
        self.hw.as_ref().map_or(&[], |hw| hw.modules())
    }

    fn find_module(&self, name: &str) -> Option<u8> {
        self.modules()
            .iter()
            .position(|m| m.name == name)
            .map(|i| i as u8)
    }

    fn module_func(&self, module: u8, name: &str) -> Option<u8> {
        self.modules()
            .get(module as usize)?
            .functions
            .iter()
            .position(|f| f.name == name)
            .map(|i| i as u8)
    }

    /// `module.name`: a constant, or a function value.
    fn module_attr(&self, module: u8, name: &str) -> Result<Value<'a>, Error> {
        if let Some(func) = self.module_func(module, name) {
            return Ok(Value::NativeFn { module, func });
        }
        self.modules()
            .get(module as usize)
            .and_then(|m| m.constants.iter().find(|(n, _)| *n == name))
            .map(|&(_, val)| Value::Int(val))
            .ok_or(Error::AttributeError)
    }

    /// Calls a native module function after checking its argument count.
    fn call_native(
        &mut self,
        module: u8,
        func: u8,
        args: &[Value<'a>],
    ) -> Result<Value<'a>, Error> {
        let (module_name, spec) = {
            let m = &self.modules()[module as usize];
            (m.name, m.functions[func as usize])
        };
        if args.len() < spec.min_args as usize || args.len() > spec.max_args as usize {
            return Err(Error::TypeError);
        }
        let mut hw_args = [Arg::Int(0); MAX_CALL_ARGS];
        for (slot, val) in hw_args.iter_mut().zip(args) {
            *slot = match *val {
                Value::Str(s) => Arg::Str(s),
                other => Arg::Int(other.as_int().ok_or(Error::TypeError)?),
            };
        }
        let hw = self.hw.as_mut().ok_or(Error::UnknownIdentifier)?;
        Ok(
            match hw.call_module(module_name, spec.name, &hw_args[..args.len()])? {
                Some(n) => Value::Int(n),
                None => Value::None,
            },
        )
    }

    /// `help(module)`: its functions and constants.
    fn print_module_help(&mut self, module: u8) {
        let Some(&m) = self.modules().get(module as usize) else {
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
        self.out.write_str("Built-in:\n  print(...)\n  help()\n");

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

        let Some(hw) = self.hw.as_ref() else {
            return;
        };
        let (functions, constants, modules) = (hw.functions(), hw.constants(), hw.modules());
        if !modules.is_empty() {
            self.out
                .write_str("Modules (import them, then help(module)):\n ");
            for m in modules {
                self.out.write_str(" ");
                self.out.write_str(m.name);
            }
            self.out.write_str("\n");
        }
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

    fn print_value(&mut self, val: Value<'_>) {
        match val {
            Value::None => self.out.write_str("None"),
            Value::Bool(true) => self.out.write_str("True"),
            Value::Bool(false) => self.out.write_str("False"),
            Value::Str(s) => self.out.write_str(s),
            Value::Int(n) => self.write_i64(n),
            Value::Module(module) => {
                let name = self.modules().get(module as usize).map_or("?", |m| m.name);
                for part in ["<module '", name, "'>"] {
                    self.out.write_str(part);
                }
            }
            Value::NativeFn { module, func } => {
                let (m, f) = self.modules().get(module as usize).map_or(("?", "?"), |m| {
                    (
                        m.name,
                        m.functions.get(func as usize).map_or("?", |f| f.name),
                    )
                });
                for part in ["<function ", m, ".", f, ">"] {
                    self.out.write_str(part);
                }
            }
        }
    }

    /// Prints a value the way the Python REPL shows it: strings in quotes.
    fn print_repr(&mut self, val: Value<'_>) {
        match val {
            Value::Str(s) => {
                // Like Python: single quotes, unless the text contains them
                let quote = if s.contains('\'') && !s.contains('"') {
                    "\""
                } else {
                    "'"
                };
                self.out.write_str(quote);
                self.out.write_str(s);
                self.out.write_str(quote);
            }
            other => self.print_value(other),
        }
    }

    fn write_i64(&mut self, value: i64) {
        if value == 0 {
            self.out.write_str("0");
            return;
        }
        let mut buf = [0u8; 20];
        let mut idx = buf.len();
        let negative = value < 0;
        // `unsigned_abs` also handles i64::MIN, whose negation does not fit in i64
        let mut n = value.unsigned_abs();
        while n > 0 {
            idx -= 1;
            buf[idx] = b'0' + (n % 10) as u8;
            n /= 10;
        }
        if negative {
            idx -= 1;
            buf[idx] = b'-';
        }
        if let Ok(s) = core::str::from_utf8(&buf[idx..]) {
            self.out.write_str(s);
        }
    }
}

/// The module in `val`, for `val.attr`; other values have no attributes.
fn as_module(val: Value<'_>) -> Result<u8, Error> {
    match val {
        Value::Module(module) => Ok(module),
        _ => Err(Error::AttributeError),
    }
}

/// Binary operator on values. Ints and bools mix like in Python (`True + 1 == 2`);
/// `&`, `|`, `^` on two bools give a bool.
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

/// Comparison with Python semantics: numbers (ints/bools) and strings compare
/// among themselves; `==`/`!=` between other types are simply false/true;
/// ordering between other types is a `TypeError`.
fn compare(op: CmpOp, lhs: Value<'_>, rhs: Value<'_>) -> Result<bool, Error> {
    let ordering = match (lhs, rhs) {
        (Value::Str(a), Value::Str(b)) => a.cmp(b),
        _ => match (lhs.as_int(), rhs.as_int()) {
            (Some(a), Some(b)) => a.cmp(&b),
            _ => {
                // None == None, and a module or function equals itself
                let equal = matches!((lhs, rhs), (Value::None, Value::None))
                    || matches!((lhs, rhs), (Value::Module(_) | Value::NativeFn { .. }, _))
                        && lhs == rhs;
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
    use super::*;

    struct MockOut {
        buf: [u8; 512],
        len: usize,
    }

    impl MockOut {
        const fn new() -> Self {
            Self {
                buf: [0; 512],
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

        let mut vm = Interpreter::new(out);
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
        assert_eval_err("'a' * 3", Error::TypeError);
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
        assert_eq!(vm.error_line(), Some(2));
        assert_eq!(
            vm.eval("a = 1\nb = 2\nc = 'open"),
            Err(Error::UnterminatedString)
        );
        assert_eq!(vm.error_line(), Some(3));
        assert_eq!(
            vm.eval("print(1)\nprint(@)"),
            Err(Error::UnexpectedChar('@'))
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
        let mut vm = Interpreter::new(out);
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
        let mut vm = Interpreter::new(&mut out);
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
        assert_eq!(run_err("s = 'a'\ns += 'b'").0, Error::TypeError);
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
        // Nested def and duplicate params are rejected
        assert_eq!(
            run_err("def f():\n    def g():\n        pass").0,
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
        assert!(interp <= 4096, "Interpreter is {} bytes", interp);
        assert!(code <= 1024, "Code is {} bytes", code);
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
        // 30 nested ifs exceed MAX_NESTING without overflowing the native stack
        let mut buf = [0u8; 2048];
        let mut len = 0;
        for depth in 0..30 {
            for _ in 0..depth {
                buf[len] = b' ';
                len += 1;
            }
            let line = b"if 1:\n";
            buf[len..len + line.len()].copy_from_slice(line);
            len += line.len();
        }
        for _ in 0..30 {
            buf[len] = b' ';
            len += 1;
        }
        buf[len..len + 4].copy_from_slice(b"pass");
        len += 4;
        let src = core::str::from_utf8(&buf[..len]).unwrap();
        assert_eq!(run_err(src).0, Error::OutOfMemory);
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
            "Built-in:\n  print(...)\n  help()\n\
             Script functions:\n  blink(times, ms)\n\
             Hardware:\n  pin_write(pin, value) - Set an output pin\n  pin_read(pin) - Read a pin\n  \
             text(s, [x, y]) - Show text\n  ticks_ms()\n\
             Constants:\n  HIGH LOW LED\n"
        );

        // Without hardware, only built-ins are listed
        let mut out = MockOut::new();
        Interpreter::new(&mut out).eval("help()").unwrap();
        assert_eq!(out.as_str(), "Built-in:\n  print(...)\n  help()\n");
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
        assert!(out
            .as_str()
            .ends_with("Modules (import them, then help(module)):\n  gfx clock\n"));
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

        let mut buf = [0u8; 512];
        let mut len = 0;
        for i in 0..MAX_VARS {
            let prefix = b"v";
            buf[len..len + prefix.len()].copy_from_slice(prefix);
            len += prefix.len();

            if i >= 10 {
                buf[len] = b'0' + (i / 10) as u8;
                len += 1;
            }
            buf[len] = b'0' + (i % 10) as u8;
            len += 1;

            let suffix = b" = 1\n";
            buf[len..len + suffix.len()].copy_from_slice(suffix);
            len += suffix.len();
        }

        let fill_script = core::str::from_utf8(&buf[..len]).unwrap();
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
