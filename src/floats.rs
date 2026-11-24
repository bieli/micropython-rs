// ============================================================================
// FLOATS: `1.5`, `7 / 2`, `float()`, `round()`, and the `math` module
// ============================================================================
//
// Double precision (`f64`), as MicroPython's ports with
// `MICROPY_FLOAT_IMPL_DOUBLE` (its ESP32 port uses single precision by
// default). Like MicroPython, an int meeting a float is converted to a float
// first (CPython compares and divides big ints exactly; the results only
// differ beyond 2**53). `repr()` gives the shortest text that reads back as
// the same float, laid out like CPython (`0.1`, `1e+16`, `1.5e-05`), and the
// math functions come from `libm` (a Rust port of musl's).

use super::{Interpreter, Value};
use crate::ast_parser::BinOp;
use crate::{Error, HardwareBridge, HwFn, Module, Output};
use core::cmp::Ordering;
use core::fmt::Write;

/// The value of an int, bool or float as a float.
pub(super) fn as_f64(v: Value<'_>) -> Option<f64> {
    match v {
        Value::Float(f) => Some(f),
        Value::Int(n) => Some(n as f64),
        Value::Bool(b) => Some(b as u8 as f64),
        _ => None,
    }
}

/// Arithmetic that gives a float: a float operand, `/`, or `**` with a
/// negative exponent. `None` for everything else (left to the int code).
#[cfg_attr(feature = "iram", link_section = ".rwtext")]
pub(super) fn float_binop<'a>(
    op: BinOp,
    lhs: Value<'a>,
    rhs: Value<'a>,
) -> Option<Result<Value<'a>, Error>> {
    let negative_pow = op == BinOp::Pow && rhs.as_int().is_some_and(|b| b < 0);
    let floats = matches!(lhs, Value::Float(_)) || matches!(rhs, Value::Float(_));
    if !floats && op != BinOp::Div && !negative_pow {
        return None;
    }
    let (Some(a), Some(b)) = (as_f64(lhs), as_f64(rhs)) else {
        return Some(Err(Error::TypeError));
    };
    Some(float_arith(op, a, b).map(Value::Float))
}

fn float_arith(op: BinOp, a: f64, b: f64) -> Result<f64, Error> {
    Ok(match op {
        BinOp::Add => a + b,
        BinOp::Sub => a - b,
        BinOp::Mul => a * b,
        BinOp::Div => {
            if b == 0.0 {
                return Err(Error::ZeroDivision);
            }
            a / b
        }
        BinOp::FloorDiv => divmod(a, b)?.0,
        BinOp::Mod => divmod(a, b)?.1,
        BinOp::Pow => power(a, b)?,
        // `&`, `|`, `^`, `<<`, `>>` aren't defined for floats
        _ => return Err(Error::TypeError),
    })
}

/// `a // b` and `a % b` with Python's floor semantics (CPython's
/// `float_divmod`): the remainder has the sign of `b`.
pub(super) fn divmod(a: f64, b: f64) -> Result<(f64, f64), Error> {
    if b == 0.0 {
        return Err(Error::ZeroDivision);
    }
    let mut m = libm::fmod(a, b);
    let mut div = (a - m) / b;
    if m != 0.0 {
        if (b < 0.0) != (m < 0.0) {
            m += b;
            div -= 1.0;
        }
    } else {
        m = libm::copysign(0.0, b);
    }
    let floordiv = if div != 0.0 {
        let f = libm::floor(div);
        if div - f > 0.5 {
            f + 1.0
        } else {
            f
        }
    } else {
        libm::copysign(0.0, a / b)
    };
    Ok((floordiv, m))
}

/// `a ** b` for floats, with Python's errors.
fn power(a: f64, b: f64) -> Result<f64, Error> {
    if a == 0.0 && b < 0.0 {
        return Err(Error::ZeroDivision); // 0.0 cannot be raised to a negative power
    }
    if a < 0.0 && a.is_finite() && b.is_finite() && libm::floor(b) != b {
        // CPython gives a complex number; MicroPython without complex numbers
        // (and this interpreter) can't
        return Err(Error::ValueError);
    }
    let r = libm::pow(a, b);
    if r.is_infinite() && a.is_finite() && b.is_finite() {
        return Err(Error::Overflow);
    }
    Ok(r)
}

/// Ordering of two numbers when one is a float (`None` when either is NaN:
/// then `<`, `>`, `==` are all false). `None` outside: not a float comparison.
pub(super) fn float_order(lhs: Value<'_>, rhs: Value<'_>) -> Option<Option<Ordering>> {
    if !matches!(lhs, Value::Float(_)) && !matches!(rhs, Value::Float(_)) {
        return None;
    }
    let (a, b) = (as_f64(lhs)?, as_f64(rhs)?);
    Some(a.partial_cmp(&b))
}

/// `-x` of a float.
pub(super) fn negate(v: Value<'_>) -> Option<Value<'_>> {
    match v {
        Value::Float(f) => Some(Value::Float(-f)),
        _ => None,
    }
}

/// A fixed buffer to format numbers into, without allocating.
pub(super) struct Buf<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

impl<const N: usize> Buf<N> {
    pub(super) fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }

    pub(super) fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }

    fn push(&mut self, s: &str) -> Result<(), Error> {
        let end = self.len + s.len();
        self.bytes
            .get_mut(self.len..end)
            .ok_or(Error::Overflow)?
            .copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

impl<const N: usize> Write for Buf<N> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.push(s).map_err(|_| core::fmt::Error)
    }
}

/// The shortest digits that read back as `x` (> 0, finite), and the position
/// of the decimal point: `x = 0.DIGITS * 10**point`.
fn shortest_digits(x: f64, digits: &mut Buf<32>) -> i32 {
    let mut sci = Buf::<40>::new();
    let _ = write!(sci, "{:e}", x); // `1.2345e-5`: Rust's shortest round trip
    let text = sci.as_str();
    let (mantissa, exp) = text.split_once('e').unwrap_or((text, "0"));
    for part in mantissa.split('.') {
        let _ = digits.push(part);
    }
    exp.parse::<i32>().unwrap_or(0) + 1
}

/// `repr()` / `str()` of a float, as CPython writes it.
pub(super) fn write_float(x: f64, sink: &mut dyn FnMut(&str)) {
    if x.is_nan() {
        return sink("nan");
    }
    if x.is_sign_negative() {
        sink("-");
    }
    let x = x.abs();
    if x.is_infinite() {
        return sink("inf");
    }
    if x == 0.0 {
        return sink("0.0");
    }
    let mut digits = Buf::<32>::new();
    let point = shortest_digits(x, &mut digits);
    let d = digits.as_str();
    let n = d.len() as i32;
    if -4 < point && point <= 16 {
        // Fixed notation: `0.001`, `12.5`, `1000.0`
        if point <= 0 {
            sink("0.");
            for _ in 0..-point {
                sink("0");
            }
            sink(d);
        } else if point >= n {
            sink(d);
            for _ in 0..point - n {
                sink("0");
            }
            sink(".0");
        } else {
            sink(&d[..point as usize]);
            sink(".");
            sink(&d[point as usize..]);
        }
    } else {
        // Exponent notation: `1e+16`, `2.5e-05`
        sink(&d[..1]);
        if n > 1 {
            sink(".");
            sink(&d[1..]);
        }
        write_exponent(point - 1, sink);
    }
}

/// `e+05`, `e-12`, `e+308`: at least two digits, as in C and Python.
fn write_exponent(exp: i32, sink: &mut dyn FnMut(&str)) {
    sink(if exp < 0 { "e-" } else { "e+" });
    let mut buf = Buf::<8>::new();
    let _ = write!(buf, "{:02}", exp.unsigned_abs());
    sink(buf.as_str());
}

// The format spec engine (format.rs, the `format` feature) uses these
/// A float formatted for a format spec type (`f F e E g G %`, or `\0` for
/// none with a precision), without sign or padding: digits of `|x|`.
#[cfg_attr(not(feature = "format"), allow(dead_code))]
pub(super) fn format_float<const N: usize>(
    x: f64,
    ty: char,
    precision: Option<usize>,
    alt: bool,
    out: &mut Buf<N>,
) -> Result<(), Error> {
    let x = x.abs();
    let upper = matches!(ty, 'F' | 'E' | 'G');
    if !x.is_finite() {
        let text = match (x.is_nan(), upper) {
            (true, false) => "nan",
            (true, true) => "NAN",
            (false, false) => "inf",
            (false, true) => "INF",
        };
        out.push(text)?;
        return if ty == '%' { out.push("%") } else { Ok(()) };
    }
    if ty == '\0' && precision.is_none() {
        // No type and no precision: like `str()`
        let mut failed = false;
        write_float(x, &mut |s| failed |= out.push(s).is_err());
        return if failed { Err(Error::Overflow) } else { Ok(()) };
    }
    match ty {
        'f' | 'F' | '%' => {
            let p = precision.unwrap_or(6);
            let v = if ty == '%' { x * 100.0 } else { x };
            write!(out, "{:.*}", p, v).map_err(|_| Error::Overflow)?;
            if alt && p == 0 {
                out.push(".")?;
            }
            if ty == '%' {
                out.push("%")?;
            }
        }
        'e' | 'E' => exponent_form(x, precision.unwrap_or(6), alt, upper, out)?,
        _ => {
            // `g` (and no type with a precision): fixed or exponent, whichever
            // is shorter, without trailing zeros
            let p = precision.unwrap_or(6).max(1);
            let exp = decimal_exponent(x, p);
            let limit = if ty == '\0' { p as i32 - 1 } else { p as i32 };
            let exp_form = exp < -4 || exp >= limit;
            if exp_form {
                exponent_form(x, p - 1, alt, upper, out)?;
            } else {
                let decimals = (p as i32 - 1 - exp).max(0) as usize;
                write!(out, "{:.*}", decimals, x).map_err(|_| Error::Overflow)?;
            }
            if !alt {
                strip_zeros(out);
            }
            if ty == '\0' && !exp_form && !out.as_str().contains('.') {
                out.push(".0")?; // `'{:.3}'.format(12.0)` is `12.0`
            }
        }
    }
    Ok(())
}

/// The exponent of `x` written with `p` significant digits (after rounding:
/// 9.99 with 2 digits is 1.0e+01).
#[cfg_attr(not(feature = "format"), allow(dead_code))]
fn decimal_exponent(x: f64, p: usize) -> i32 {
    if x == 0.0 {
        return 0;
    }
    let mut sci = Buf::<48>::new();
    let _ = write!(sci, "{:.*e}", p - 1, x);
    let text = sci.as_str();
    text.split_once('e')
        .and_then(|(_, e)| e.parse().ok())
        .unwrap_or(0)
}

/// `1.234560e+05`: Rust writes `1.23456e5`, Python pads the exponent.
#[cfg_attr(not(feature = "format"), allow(dead_code))]
fn exponent_form<const N: usize>(
    x: f64,
    p: usize,
    alt: bool,
    upper: bool,
    out: &mut Buf<N>,
) -> Result<(), Error> {
    let mut sci = Buf::<400>::new();
    write!(sci, "{:.*e}", p, x).map_err(|_| Error::Overflow)?;
    let text = sci.as_str();
    let (mantissa, exp) = text.split_once('e').unwrap_or((text, "0"));
    out.push(mantissa)?;
    if alt && p == 0 {
        out.push(".")?;
    }
    let mut failed = false;
    write_exponent(exp.parse().unwrap_or(0), &mut |s| {
        let s = if upper && s.starts_with('e') {
            if s == "e-" {
                "E-"
            } else {
                "E+"
            }
        } else {
            s
        };
        failed |= out.push(s).is_err();
    });
    if failed {
        Err(Error::Overflow)
    } else {
        Ok(())
    }
}

/// `1.500000` -> `1.5`, `2.000` -> `2`, `1.50e+03` -> `1.5e+03`.
#[cfg_attr(not(feature = "format"), allow(dead_code))]
fn strip_zeros<const N: usize>(out: &mut Buf<N>) {
    let text = out.as_str();
    let (mantissa_end, has_exp) = match text.find(['e', 'E']) {
        Some(i) => (i, true),
        None => (text.len(), false),
    };
    if !text[..mantissa_end].contains('.') {
        return;
    }
    let trimmed = text[..mantissa_end]
        .trim_end_matches('0')
        .trim_end_matches('.')
        .len();
    if has_exp {
        let tail = mantissa_end..out.len;
        out.bytes.copy_within(tail, trimmed);
        out.len = trimmed + (out.len - mantissa_end);
    } else {
        out.len = trimmed;
    }
}

/// `float(text)`: `'1.5'`, `' -2e3 '`, `'inf'`, `'nan'`, `'1_000.5'`.
pub(super) fn parse_float(text: &str) -> Result<f64, Error> {
    let t = text.trim();
    let (sign, body) = match t.as_bytes().first() {
        Some(b'-') => (-1.0, &t[1..]),
        Some(b'+') => (1.0, &t[1..]),
        _ => (1.0, t),
    };
    let special = match body.to_ascii_lowercase_buf().as_str() {
        "inf" | "infinity" => Some(f64::INFINITY),
        "nan" => Some(f64::NAN),
        _ => None,
    };
    if let Some(v) = special {
        return Ok(sign * v);
    }
    // Digits with single underscores between them, as in literals
    let mut buf = Buf::<64>::new();
    let bytes = body.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'_' {
            let digit = |j: usize| bytes.get(j).is_some_and(u8::is_ascii_digit);
            if i == 0 || !digit(i - 1) || !digit(i + 1) {
                return Err(Error::ValueError);
            }
            continue;
        }
        if !(b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-')) {
            return Err(Error::ValueError);
        }
        buf.push(core::str::from_utf8(&[b]).unwrap_or(""))
            .map_err(|_| Error::ValueError)?;
    }
    let value: f64 = buf.as_str().parse().map_err(|_| Error::ValueError)?;
    Ok(sign * value)
}

/// Lowercase copy of a short ASCII string (for `inf`/`nan`).
trait LowerBuf {
    fn to_ascii_lowercase_buf(&self) -> Buf<16>;
}

impl LowerBuf for str {
    fn to_ascii_lowercase_buf(&self) -> Buf<16> {
        let mut buf = Buf::<16>::new();
        if self.len() <= 16 {
            for c in self.chars() {
                let mut tmp = [0u8; 4];
                let _ = buf.push(c.to_ascii_lowercase().encode_utf8(&mut tmp));
            }
        }
        buf
    }
}

/// `int(x)` of a float: towards zero, like Python.
pub(super) fn float_to_int(x: f64) -> Result<i64, Error> {
    if x.is_nan() {
        return Err(Error::ValueError); // cannot convert float NaN to integer
    }
    let t = libm::trunc(x);
    // The i64 range: [-2**63, 2**63)
    if !(-9.223_372_036_854_776e18..9.223_372_036_854_776e18).contains(&t) {
        return Err(Error::Overflow);
    }
    Ok(t as i64)
}

/// Round half to even (`round(2.5)` is 2), as Python does.
fn round_half_even(x: f64) -> f64 {
    let r = libm::round(x);
    if (r - x).abs() == 0.5 && libm::fmod(r, 2.0) != 0.0 {
        r - libm::copysign(1.0, x)
    } else {
        r
    }
}

/// `math` module functions taking one float and returning one, with
/// CPython's errors: a NaN from a number is a domain error (ValueError), an
/// infinity from a finite number an OverflowError for the functions that can
/// overflow and a domain error for the others (`log(0)`).
fn math1(name: &str, x: f64) -> Option<Result<f64, Error>> {
    let (r, can_overflow) = match name {
        "sqrt" => (libm::sqrt(x), false),
        "exp" => (libm::exp(x), true),
        "expm1" => (libm::expm1(x), true),
        "log2" => (libm::log2(x), false),
        "log10" => (libm::log10(x), false),
        "sin" => (libm::sin(x), false),
        "cos" => (libm::cos(x), false),
        "tan" => (libm::tan(x), false),
        "asin" => (libm::asin(x), false),
        "acos" => (libm::acos(x), false),
        "atan" => (libm::atan(x), false),
        "sinh" => (libm::sinh(x), true),
        "cosh" => (libm::cosh(x), true),
        "tanh" => (libm::tanh(x), false),
        "asinh" => (libm::asinh(x), false),
        "acosh" => (libm::acosh(x), false),
        "atanh" => (libm::atanh(x), false),
        "fabs" => (libm::fabs(x), false),
        "erf" => (libm::erf(x), false),
        "erfc" => (libm::erfc(x), false),
        "gamma" => (libm::tgamma(x), true),
        "lgamma" => (libm::lgamma(x), true),
        "degrees" => (x * (180.0 / core::f64::consts::PI), true),
        "radians" => (x * (core::f64::consts::PI / 180.0), false),
        _ => return None,
    };
    Some(checked(x, r, can_overflow))
}

fn checked(x: f64, r: f64, can_overflow: bool) -> Result<f64, Error> {
    if r.is_nan() && !x.is_nan() {
        return Err(Error::ValueError); // math domain error
    }
    if r.is_infinite() && x.is_finite() {
        return Err(if can_overflow {
            Error::Overflow
        } else {
            Error::ValueError
        });
    }
    Ok(r)
}

/// `import math`: MicroPython's math module (and CPython's names).
pub(crate) const MATH: Module = Module {
    name: "math",
    doc: "Floating point math (also pi, e, tau, inf, nan)",
    functions: &[
        HwFn::new("sqrt", "x", ""),
        HwFn::with_args("pow", 2, 2, "x, y", ""),
        HwFn::new("exp", "x", ""),
        HwFn::new("expm1", "x", ""),
        HwFn::with_args("log", 1, 2, "x, [base]", ""),
        HwFn::new("log2", "x", ""),
        HwFn::new("log10", "x", ""),
        HwFn::new("sin", "x", ""),
        HwFn::new("cos", "x", ""),
        HwFn::new("tan", "x", ""),
        HwFn::new("asin", "x", ""),
        HwFn::new("acos", "x", ""),
        HwFn::new("atan", "x", ""),
        HwFn::with_args("atan2", 2, 2, "y, x", ""),
        HwFn::new("sinh", "x", ""),
        HwFn::new("cosh", "x", ""),
        HwFn::new("tanh", "x", ""),
        HwFn::new("asinh", "x", ""),
        HwFn::new("acosh", "x", ""),
        HwFn::new("atanh", "x", ""),
        HwFn::new("floor", "x", ""),
        HwFn::new("ceil", "x", ""),
        HwFn::new("trunc", "x", ""),
        HwFn::new("fabs", "x", ""),
        HwFn::with_args("fmod", 2, 2, "x, y", ""),
        HwFn::new("modf", "x", ""),
        HwFn::new("frexp", "x", ""),
        HwFn::with_args("ldexp", 2, 2, "x, i", ""),
        HwFn::with_args("copysign", 2, 2, "x, y", ""),
        HwFn::new("isfinite", "x", ""),
        HwFn::new("isinf", "x", ""),
        HwFn::new("isnan", "x", ""),
        HwFn::new("degrees", "x", ""),
        HwFn::new("radians", "x", ""),
        HwFn::new("erf", "x", ""),
        HwFn::new("erfc", "x", ""),
        HwFn::new("gamma", "x", ""),
        HwFn::new("lgamma", "x", ""),
        HwFn::new("factorial", "n", ""),
        HwFn::with_args("hypot", 2, 2, "x, y", ""),
    ],
    constants: &[],
};

/// `math.pi` and the other float constants.
pub(super) fn math_constant(name: &str) -> Option<f64> {
    Some(match name {
        "pi" => core::f64::consts::PI,
        "e" => core::f64::consts::E,
        "tau" => core::f64::consts::TAU,
        "inf" => f64::INFINITY,
        "nan" => f64::NAN,
        _ => return None,
    })
}

impl<'a, O: Output, H: HardwareBridge> Interpreter<'a, O, H> {
    /// A float argument (ints and bools too).
    fn float_arg(v: Value<'a>) -> Result<f64, Error> {
        as_f64(v).ok_or(Error::TypeError)
    }

    /// `math.name(args)`.
    pub(super) fn math_call(&mut self, name: &str, args: &[Value<'a>]) -> Result<Value<'a>, Error> {
        let x = Self::float_arg(args[0])?;
        let y = || args.get(1).copied().map(Self::float_arg).transpose();
        if let Some(r) = math1(name, x) {
            return r.map(Value::Float);
        }
        let float = |r: f64| Ok(Value::Float(r));
        match name {
            "floor" | "ceil" | "trunc" => {
                if let Value::Int(_) | Value::Bool(_) = args[0] {
                    return Ok(Value::Int(args[0].as_int().unwrap_or(0)));
                }
                let r = match name {
                    "floor" => libm::floor(x),
                    "ceil" => libm::ceil(x),
                    _ => libm::trunc(x),
                };
                float_to_int(r).map(Value::Int)
            }
            "log" => {
                let ln = checked(x, libm::log(x), false)?;
                match y()? {
                    None => float(ln),
                    Some(base) => {
                        let lb = checked(base, libm::log(base), false)?;
                        if lb == 0.0 {
                            return Err(Error::ZeroDivision);
                        }
                        float(ln / lb)
                    }
                }
            }
            "pow" => {
                let b = y()?.unwrap_or(0.0);
                if x == 0.0 && b < 0.0 {
                    return Err(Error::ValueError); // math.pow: domain error, not ZeroDivision
                }
                float(power(x, b)?)
            }
            "atan2" => float(libm::atan2(x, y()?.unwrap_or(0.0))),
            "fmod" => {
                let b = y()?.unwrap_or(0.0);
                if b == 0.0 || x.is_infinite() {
                    return Err(Error::ValueError);
                }
                float(libm::fmod(x, b))
            }
            "copysign" => float(libm::copysign(x, y()?.unwrap_or(0.0))),
            "hypot" => float(libm::hypot(x, y()?.unwrap_or(0.0))),
            "ldexp" => {
                let i = args[1].as_int().ok_or(Error::TypeError)?;
                let i = i.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
                float(checked(x, libm::ldexp(x, i), true)?)
            }
            "isfinite" => Ok(Value::Bool(x.is_finite())),
            "isinf" => Ok(Value::Bool(x.is_infinite())),
            "isnan" => Ok(Value::Bool(x.is_nan())),
            "modf" => {
                let (frac, int) = libm::modf(x);
                self.new_tuple(&[Value::Float(frac), Value::Float(int)])
            }
            "frexp" => {
                let (m, e) = libm::frexp(x);
                self.new_tuple(&[Value::Float(m), Value::Int(e as i64)])
            }
            "factorial" => {
                let n = args[0].as_int().ok_or(Error::TypeError)?;
                if n < 0 {
                    return Err(Error::ValueError);
                }
                (1..=n)
                    .try_fold(1i64, |acc, k| acc.checked_mul(k))
                    .map(Value::Int)
                    .ok_or(Error::Overflow)
            }
            _ => Err(Error::UnknownIdentifier),
        }
    }

    /// `float()`, `float(x)`.
    pub(super) fn float_builtin(&self, args: &[Value<'a>]) -> Result<Value<'a>, Error> {
        match args {
            [] => Ok(Value::Float(0.0)),
            [v] => match as_f64(*v) {
                Some(f) => Ok(Value::Float(f)),
                None => {
                    let text = self.str_of(*v).ok_or(Error::TypeError)?;
                    parse_float(text).map(Value::Float)
                }
            },
            _ => Err(Error::TypeError),
        }
    }

    /// `round(x)` and `round(x, n)`.
    pub(super) fn round_builtin(&self, args: &[Value<'a>]) -> Result<Value<'a>, Error> {
        let (x, ndigits) = match args {
            [x] => (*x, None),
            [x, Value::None] => (*x, None),
            [x, n] => (*x, Some(n.as_int().ok_or(Error::TypeError)?)),
            _ => return Err(Error::TypeError),
        };
        match (x, ndigits) {
            (Value::Float(f), None) => float_to_int(round_half_even(f)).map(Value::Int),
            (Value::Float(f), Some(n)) => Ok(Value::Float(round_float(f, n))),
            (Value::Int(_) | Value::Bool(_), None) => Ok(Value::Int(x.as_int().unwrap_or(0))),
            (Value::Int(_) | Value::Bool(_), Some(n)) => {
                round_int(x.as_int().unwrap_or(0), n).map(Value::Int)
            }
            _ => Err(Error::TypeError),
        }
    }

    /// `divmod(a, b)`.
    pub(super) fn divmod_builtin(
        &mut self,
        a: Value<'a>,
        b: Value<'a>,
    ) -> Result<Value<'a>, Error> {
        if let (Value::Int(_) | Value::Bool(_), Value::Int(_) | Value::Bool(_)) = (a, b) {
            let q = super::binop(BinOp::FloorDiv, a, b)?;
            let r = super::binop(BinOp::Mod, a, b)?;
            return self.new_tuple(&[q, r]);
        }
        let (x, y) = (Self::float_arg(a)?, Self::float_arg(b)?);
        let (q, r) = divmod(x, y)?;
        self.new_tuple(&[Value::Float(q), Value::Float(r)])
    }
}

/// `round(x, n)` of a float: correctly rounded to `n` decimals (through the
/// exact decimal text, like CPython), or to tens/hundreds for negative `n`.
fn round_float(x: f64, n: i64) -> f64 {
    if !x.is_finite() || n > 300 {
        return x;
    }
    if n >= 0 {
        let mut text = Buf::<400>::new();
        if write!(text, "{:.*}", n as usize, x).is_err() {
            return x;
        }
        return text.as_str().parse().unwrap_or(x);
    }
    let scale = libm::pow(10.0, (-n).min(400) as f64);
    if scale.is_infinite() {
        return libm::copysign(0.0, x);
    }
    round_half_even(x / scale) * scale
}

/// `round(n, k)` of an int: `k >= 0` leaves it, `k < 0` rounds to tens etc.
/// (half to even).
fn round_int(n: i64, k: i64) -> Result<i64, Error> {
    if k >= 0 {
        return Ok(n);
    }
    if k < -18 {
        return Ok(0);
    }
    let unit = 10i128.pow((-k) as u32);
    let n = n as i128;
    let (q, r) = (n.div_euclid(unit), n.rem_euclid(unit));
    let q = match (2 * r).cmp(&unit) {
        Ordering::Greater => q + 1,
        Ordering::Equal if q % 2 != 0 => q + 1,
        _ => q,
    };
    i64::try_from(q * unit).map_err(|_| Error::Overflow)
}
