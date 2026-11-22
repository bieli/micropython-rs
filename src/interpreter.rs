// ============================================================================
// INTERPRETER / EVALUATOR
// ============================================================================

use core::sync::atomic::{AtomicBool, Ordering};

use crate::ast_parser::{BinOp, Code, Expr, Item, Parser, Stmt, MAX_CALL_ARGS, MAX_EXPR_ITEMS};
use crate::{Error, HardwareBridge, NoHardware, Output};

const MAX_VARS: usize = 32;
/// RPN evaluation stack depth. With calls, every item of an expression can be
/// an operand waiting on the stack (`f(1, 2, 3, ...)`), so it matches the item limit.
const MAX_EVAL_STACK: usize = MAX_EXPR_ITEMS;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value<'a> {
    None,
    Str(&'a str),
    Int(i64),
}

pub struct Interpreter<'a, O: Output, H: HardwareBridge = NoHardware> {
    out: &'a mut O,
    hw: Option<&'a mut H>,
    vars: [Option<(&'a str, Value<'a>)>; MAX_VARS],
    /// Line of the last error returned by `eval`, if any.
    error_line: Option<u32>,
    /// Maximum statements one `eval` call may execute (`None` = unlimited).
    step_limit: Option<u32>,
    /// Set from outside (e.g. a UART Ctrl-C handler) to stop a running script.
    abort: Option<&'a AtomicBool>,
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
            vars: [None; MAX_VARS],
            error_line: None,
            step_limit: None,
            abort: None,
        }
    }

    /// Limits how many statements a single `eval` call may execute.
    /// When the limit is hit, `eval` returns `Error::StepLimit`.
    /// Protects the device from scripts that never finish.
    pub fn set_step_limit(&mut self, limit: Option<u32>) {
        self.step_limit = limit;
    }

    /// Registers a flag that stops the running script when set to `true`
    /// (checked before every statement), making `eval` return `Error::Interrupted`.
    /// The flag is only read, so the owner clears it before the next run.
    pub fn set_abort_flag(&mut self, flag: &'a AtomicBool) {
        self.abort = Some(flag);
    }

    /// 1-based line where the error returned by the last `eval` call happened,
    /// or `None` if it succeeded.
    pub const fn error_line(&self) -> Option<u32> {
        self.error_line
    }

    pub fn eval(&mut self, source: &'a str) -> Result<(), Error> {
        let mut line = 1;
        let result = self.run(source, &mut line);
        self.error_line = result.err().map(|_| line);
        result
    }

    /// Runs `source`, keeping `line` pointed at the code currently being handled.
    fn run(&mut self, source: &'a str, line: &mut u32) -> Result<(), Error> {
        let mut parser = Parser::new(source)?;
        let mut code = Code::new();
        let mut steps: u32 = 0;

        loop {
            let parsed = parser.parse_next_stmt(&mut code);
            // Parse errors point at the offending token, runtime errors at the statement
            *line = match parsed {
                Ok(_) => parser.stmt_line(),
                Err(_) => parser.line(),
            };
            let Some(stmt) = parsed? else {
                return Ok(());
            };

            if self.abort.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                return Err(Error::Interrupted);
            }
            steps += 1;
            if self.step_limit.is_some_and(|limit| steps > limit) {
                return Err(Error::StepLimit);
            }

            self.exec_stmt(stmt, &code)?;
        }
    }

    fn set_var(&mut self, name: &'a str, val: Value<'a>) -> Result<(), Error> {
        for slot in self.vars.iter_mut().flatten() {
            if slot.0 == name {
                slot.1 = val;
                return Ok(());
            }
        }
        for slot in self.vars.iter_mut() {
            if slot.is_none() {
                *slot = Some((name, val));
                return Ok(());
            }
        }
        Err(Error::OutOfMemory)
    }

    fn get_var(&self, name: &str) -> Result<Value<'a>, Error> {
        for (var_name, val) in self.vars.iter().flatten() {
            if *var_name == name {
                return Ok(*val);
            }
        }
        Err(Error::UnknownIdentifier)
    }

    /// Evaluates one RPN expression with a fixed-size value stack.
    fn eval_expr(&mut self, code: &Code<'a>, expr: Expr) -> Result<Value<'a>, Error> {
        let mut stack = [Value::None; MAX_EVAL_STACK];
        let mut sp = 0;

        for item in code.items(expr) {
            let val = match *item {
                Item::Int(n) => Value::Int(n),
                Item::Str(s) => Value::Str(s),
                Item::Ident(name) => self.get_var(name)?,
                Item::Neg => {
                    sp -= 1;
                    match stack[sp] {
                        Value::Int(n) => Value::Int(n.checked_neg().ok_or(Error::Overflow)?),
                        _ => return Err(Error::TypeError),
                    }
                }
                Item::Bin(op) => {
                    sp -= 2;
                    match (stack[sp], stack[sp + 1]) {
                        (Value::Int(a), Value::Int(b)) => Value::Int(int_binop(op, a, b)?),
                        _ => return Err(Error::TypeError),
                    }
                }
                Item::Call { name, argc } => {
                    sp -= argc as usize;
                    self.call_function(name, &stack[sp..sp + argc as usize])?
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

    fn exec_stmt(&mut self, stmt: Stmt<'a>, code: &Code<'a>) -> Result<(), Error> {
        match stmt {
            Stmt::Assign { name, expr } => {
                let val = self.eval_expr(code, expr)?;
                self.set_var(name, val)
            }
            Stmt::Expr(expr) => self.eval_expr(code, expr).map(|_| ()),
        }
    }

    /// Calls a built-in (`print`) or a hardware function. Arguments are
    /// already evaluated, so a failing argument means nothing is printed or
    /// sent to hardware (like Python).
    fn call_function(&mut self, name: &str, args: &[Value<'_>]) -> Result<Value<'a>, Error> {
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

        let hw = self.hw.as_mut().ok_or(Error::UnknownIdentifier)?;
        let mut int_args = [0i64; MAX_CALL_ARGS];
        for (slot, val) in int_args.iter_mut().zip(args) {
            match val {
                Value::Int(n) => *slot = *n,
                _ => return Err(Error::TypeError),
            }
        }

        Ok(match hw.call(name, &int_args[..args.len()])? {
            Some(n) => Value::Int(n),
            None => Value::None,
        })
    }

    fn print_value(&mut self, val: Value<'_>) {
        match val {
            Value::None => self.out.write_str("None"),
            Value::Str(s) => self.out.write_str(s),
            Value::Int(n) => self.write_i64(n),
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
    };
    result.ok_or(Error::Overflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockOut {
        buf: [u8; 128],
        len: usize,
    }

    impl MockOut {
        const fn new() -> Self {
            Self {
                buf: [0; 128],
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
