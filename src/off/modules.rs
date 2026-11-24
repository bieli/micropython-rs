// Without the `modules` feature: only native modules can be imported, so
// `import sensors` raises `ImportError` as for a module that isn't there.

use super::{Interpreter, Value};
use crate::heap::ObjRef;
use crate::{Error, HardwareBridge, Output};

impl<'a, O: Output, H: HardwareBridge> Interpreter<'a, O, H> {
    pub(super) fn module_dict(&self, _v: Value<'a>) -> Option<ObjRef> {
        None
    }

    pub(super) fn py_module_attr(&self, _v: Value<'a>, _name: &str) -> Option<Option<Value<'a>>> {
        None
    }

    pub(super) fn import_py(&mut self, _name: &'a str) -> Result<Value<'a>, Error> {
        Err(Error::ImportError)
    }

    pub(super) fn module_del(&mut self, _dict: ObjRef, _name: &str) -> Result<(), Error> {
        Err(Error::UnknownIdentifier)
    }
}
