// Without the `float` feature (the `minimal` level): float literals and `/`
// raise `NotImplementedError` (like MicroPython built without floats, where
// they're errors too), there's no `math` module, and `round()`, `divmod()` and
// `pow()` work on ints only. No float value can exist, so the float paths of
// the rest of the interpreter never run.

// Some of these are only called by the format engine (the `format` feature)
#![allow(dead_code)]

use super::{Interpreter, Value};
use crate::ast_parser::BinOp;
use crate::{Error, HardwareBridge, Output};
use core::cmp::Ordering;

pub(super) fn as_float(_v: Value<'_>) -> Option<crate::Float> {
    None
}

/// `/`, and `**` with a negative exponent, would give a float.
pub(super) fn float_binop<'a>(
    op: BinOp,
    _lhs: Value<'a>,
    rhs: Value<'a>,
) -> Option<Result<Value<'a>, Error>> {
    let negative_pow = op == BinOp::Pow && rhs.as_int().is_some_and(|b| b < 0);
    (op == BinOp::Div || negative_pow).then_some(Err(Error::NotEnabled))
}

pub(super) fn float_order(_lhs: Value<'_>, _rhs: Value<'_>) -> Option<Option<Ordering>> {
    None
}

pub(super) fn negate(_v: Value<'_>) -> Option<Value<'_>> {
    None
}

pub(super) fn write_float(_x: crate::Float, sink: &mut dyn FnMut(&str)) {
    sink("?");
}

pub(super) fn float_to_int(_x: crate::Float) -> Result<i64, Error> {
    Err(Error::NotEnabled)
}

pub(super) fn math_constant(_name: &str) -> Option<crate::Float> {
    None
}

/// For the format engine, which only calls it with the `float` feature.
pub(super) struct Buf<const N: usize>;

impl<const N: usize> Buf<N> {
    pub(super) fn new() -> Self {
        Self
    }

    pub(super) fn as_str(&self) -> &str {
        ""
    }
}

pub(super) fn format_float<const N: usize>(
    _x: f64,
    _ty: char,
    _precision: Option<usize>,
    _alt: bool,
    _out: &mut Buf<N>,
) -> Result<(), Error> {
    Err(Error::NotEnabled)
}

impl<'a, O: Output, H: HardwareBridge> Interpreter<'a, O, H> {
    pub(super) fn math_call(
        &mut self,
        _name: &str,
        _args: &[Value<'a>],
    ) -> Result<Value<'a>, Error> {
        Err(Error::NotEnabled)
    }

    pub(super) fn float_builtin(&self, _args: &[Value<'a>]) -> Result<Value<'a>, Error> {
        Err(Error::NotEnabled)
    }

    /// `round(n)` / `round(n, k)` of an int.
    pub(super) fn round_builtin(&self, args: &[Value<'a>]) -> Result<Value<'a>, Error> {
        match args {
            [x] | [x, Value::None] => x.as_int().map(Value::Int).ok_or(Error::TypeError),
            [x, k] => {
                let (n, k) = (
                    x.as_int().ok_or(Error::TypeError)?,
                    k.as_int().ok_or(Error::TypeError)?,
                );
                if k >= 0 {
                    return Ok(Value::Int(n));
                }
                if k < -18 {
                    return Ok(Value::Int(0));
                }
                let unit = 10i128.pow((-k) as u32);
                let (q, r) = ((n as i128).div_euclid(unit), (n as i128).rem_euclid(unit));
                let q = match (2 * r).cmp(&unit) {
                    Ordering::Greater => q + 1,
                    Ordering::Equal if q % 2 != 0 => q + 1,
                    _ => q,
                };
                i64::try_from(q * unit)
                    .map(Value::Int)
                    .map_err(|_| Error::Overflow)
            }
            _ => Err(Error::TypeError),
        }
    }

    /// `divmod(a, b)` of ints.
    pub(super) fn divmod_builtin(
        &mut self,
        a: Value<'a>,
        b: Value<'a>,
    ) -> Result<Value<'a>, Error> {
        let q = super::binop(BinOp::FloorDiv, a, b)?;
        let r = super::binop(BinOp::Mod, a, b)?;
        self.new_tuple(&[q, r])
    }
}
