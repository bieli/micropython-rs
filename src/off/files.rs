// Without the `files` feature: `open()` raises `NotImplementedError` and there
// is no `os` module, so no file objects exist. (The application can still use
// its `FileSystem` itself, e.g. the firmware's main.py storage.)

use super::{Interpreter, Value};
use crate::heap::ObjRef;
use crate::{Error, HardwareBridge, Output};

impl<'a, O: Output, H: HardwareBridge> Interpreter<'a, O, H> {
    pub(super) fn os_call(&mut self, _name: &str, _args: &[Value<'a>]) -> Result<Value<'a>, Error> {
        Err(Error::NotEnabled)
    }

    pub(super) fn open(&mut self, _args: &[Value<'a>]) -> Result<Value<'a>, Error> {
        Err(Error::NotEnabled)
    }

    pub(super) fn file_method(
        &mut self,
        _receiver: Value<'a>,
        _name: &str,
        _args: &[Value<'a>],
    ) -> Option<Result<Value<'a>, Error>> {
        None
    }

    pub(super) fn file_attr(&self, _v: Value<'a>, _name: &str) -> Option<Value<'a>> {
        None
    }

    pub(super) fn file_next_line(&mut self, _r: ObjRef) -> Result<Option<Value<'a>>, Error> {
        Err(Error::NotEnabled)
    }

    pub(super) fn close_if_file(&mut self, _v: Value<'a>) -> Result<(), Error> {
        Ok(())
    }
}
