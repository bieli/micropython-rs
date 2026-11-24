// Without the `classes` feature: no class can be defined (`class` raises
// `NotImplementedError`), so there are no instances, and these entry points
// of classes.rs only say "not a class" or "not enabled".

use super::{Interpreter, Kwargs, Value};
use crate::heap::{Kind, ObjRef};
use crate::{Error, HardwareBridge, Output};

impl<'a, O: Output, H: HardwareBridge> Interpreter<'a, O, H> {
    pub(super) fn class_of(&self, _v: Value<'a>) -> Option<ObjRef> {
        None
    }

    pub(super) fn exception_root(&self, _class: ObjRef) -> Option<u8> {
        None
    }

    pub(super) fn derives_from(&self, _class: ObjRef, _of: ObjRef) -> bool {
        false
    }

    pub(super) fn has_class_method(&self, _obj: Value<'a>, _name: &str) -> bool {
        false
    }

    pub(super) fn object_attr(&self, _obj: Value<'a>, _name: &str) -> Option<Option<Value<'a>>> {
        None
    }

    pub(super) fn set_attr(
        &mut self,
        _obj: Value<'a>,
        _name: &'a str,
        _v: Value<'a>,
    ) -> Result<(), Error> {
        Err(Error::AttributeError)
    }

    pub(super) fn object_method(
        &mut self,
        _obj: Value<'a>,
        _name: &str,
        _args: &[Value<'a>],
        _kwargs: Kwargs<'_, 'a>,
    ) -> Option<Result<Value<'a>, Error>> {
        None
    }

    pub(super) fn instantiate(
        &mut self,
        _class: ObjRef,
        _args: &[Value<'a>],
        _kwargs: Kwargs<'_, 'a>,
    ) -> Result<Value<'a>, Error> {
        Err(Error::NotEnabled)
    }

    /// `classmethod(f)` / `staticmethod(f)`.
    pub(super) fn method_wrapper(
        &mut self,
        _kind: Kind,
        _f: Value<'a>,
    ) -> Result<Value<'a>, Error> {
        Err(Error::NotEnabled)
    }

    /// `super()`.
    pub(super) fn make_super(&mut self) -> Result<Value<'a>, Error> {
        Err(Error::NotEnabled)
    }

    pub(super) fn call_dunder(
        &mut self,
        _obj: Value<'a>,
        _name: &str,
        _args: &[Value<'a>],
    ) -> Option<Result<Value<'a>, Error>> {
        None
    }

    pub(super) fn call_dunder_kw(
        &mut self,
        _obj: Value<'a>,
        _name: &str,
        _args: &[Value<'a>],
        _kwargs: Kwargs<'_, 'a>,
    ) -> Option<Result<Value<'a>, Error>> {
        None
    }

    pub(super) fn custom_str(
        &mut self,
        _obj: Value<'a>,
        _repr: bool,
    ) -> Option<Result<Value<'a>, Error>> {
        None
    }
}
