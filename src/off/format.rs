// Without the `format` feature: `%` on strings, `str.format`, f-strings,
// `format()` and `hex()`/`bin()`/`oct()` raise `NotImplementedError`.
// (`str()`, `repr()` and `print()` don't need it.)

use super::{Interpreter, Kwargs, Value};
use crate::{Error, HardwareBridge, Output};

/// A format spec (`:>8`), which can't be used here.
#[derive(Default)]
pub(super) struct Spec {
    _none: (),
}

impl Spec {
    pub(super) fn parse(_text: &str) -> Result<Self, Error> {
        Err(Error::NotEnabled)
    }
}

impl<'a, O: Output, H: HardwareBridge> Interpreter<'a, O, H> {
    pub(super) fn format_one(
        &mut self,
        _v: Value<'a>,
        _conv: char,
        _spec: &Spec,
        _min_digits: usize,
    ) -> Result<Value<'a>, Error> {
        Err(Error::NotEnabled)
    }

    /// An f-string.
    pub(super) fn join_pieces(&mut self, _base: usize) -> Result<Value<'a>, Error> {
        Err(Error::NotEnabled)
    }

    pub(super) fn percent_format(
        &mut self,
        _fmt: Value<'a>,
        _args: Value<'a>,
    ) -> Result<Value<'a>, Error> {
        Err(Error::NotEnabled)
    }

    pub(super) fn str_format(
        &mut self,
        _fmt: Value<'a>,
        _args: &[Value<'a>],
        _kwargs: Kwargs<'_, 'a>,
    ) -> Result<Value<'a>, Error> {
        Err(Error::NotEnabled)
    }

    pub(super) fn int_text(&mut self, _v: Value<'a>, _ty: char) -> Result<Value<'a>, Error> {
        Err(Error::NotEnabled)
    }

    pub(super) fn format_builtin(
        &mut self,
        _value: Value<'a>,
        _spec: Value<'a>,
    ) -> Result<Value<'a>, Error> {
        Err(Error::NotEnabled)
    }
}
