// ============================================================================
// PYTHON MODULES: `import sensors` runs `sensors.py` from the file system
// ============================================================================
//
// A module's source is read into the module memory the application gives
// the interpreter (`with_module_memory`): append-only, because variables and
// functions point into the source text, so it must not move (the heap can't
// hold it). A module is imported once; later imports reuse it (`sys.modules`).
//
// A module object is laid out like a tuple `(name, dict)`. Its code runs with
// the dict as its global namespace: top-level assignments, `def`, `class` and
// `import` write into it. Functions remember the module they're defined in
// (`FuncDef::module`), so their global names are looked up in its dict, not
// in the main script's globals.

use super::{Code, Ctx, Interpreter, Parser, Value};
use crate::flatfs::MAX_NAME_LEN;
use crate::heap::{Kind, ObjRef};
use crate::{Error, HardwareBridge, Output};

impl<'a, O: Output, H: HardwareBridge> Interpreter<'a, O, H> {
    /// The dict of a module object.
    pub(super) fn module_dict(&self, v: Value<'a>) -> Option<ObjRef> {
        match v {
            Value::Obj(r) if self.heap.kind(r) == Kind::PyModule => match self.slot(r, 1) {
                Value::Obj(d) => Some(d),
                _ => None,
            },
            _ => None,
        }
    }

    /// `module.name` of an imported `.py` module.
    pub(super) fn py_module_attr(&self, v: Value<'a>, name: &str) -> Option<Option<Value<'a>>> {
        let dict = self.module_dict(v)?;
        Some(self.dict_get_str(dict, name))
    }

    /// `import name` of a `.py` module: imports it the first time.
    #[inline(never)]
    pub(super) fn import_py(&mut self, name: &'a str) -> Result<Value<'a>, Error> {
        if let Value::Obj(cache) = self.py_modules {
            if let Some(module) = self.dict_get_str(cache, name) {
                return Ok(module);
            }
        }
        let source = self.load_source(name)?;

        // The module object: (name, dict), cached before its code runs (so an
        // import cycle gets the half-made module, like Python)
        let base = self.sp;
        let made = (|| {
            if self.py_modules == Value::None {
                self.py_modules = Value::Obj(self.new_container(Kind::Dict, 0)?);
            }
            let dict = self.new_container(Kind::Dict, 0)?;
            self.push(Value::Obj(dict))?;
            self.dict_set(dict, Value::Str("__name__"), Value::Str(name))?;
            self.push(Value::Str(name))?;
            self.push(Value::Obj(dict))?;
            self.build(Kind::Tuple, 2)?;
            let module = self.stack[self.sp - 1];
            if let Value::Obj(r) = module {
                self.heap.set_kind(r, Kind::PyModule);
            }
            if let Value::Obj(cache) = self.py_modules {
                self.dict_set(cache, Value::Str(name), module)?;
            }
            Ok((module, dict))
        })();
        self.sp = base;
        let (module, dict) = made?;

        // Run its code with the module dict as the global namespace
        let saved = (self.module_env, self.module_top);
        (self.module_env, self.module_top) = (Value::Obj(dict), true);
        let result = (|| {
            let mut parser = Parser::new(source)?;
            let mut code = Code::new();
            self.exec_block(&mut parser, 0, true, Ctx::default(), &mut code)
        })();
        (self.module_env, self.module_top) = saved;
        if let Err(err) = result {
            // A module that failed isn't kept (the next import tries again)
            if let Value::Obj(cache) = self.py_modules {
                if let Ok(Some(i)) = self.dict_find(cache, Value::Str(name)) {
                    self.dict_remove(cache, i);
                }
            }
            return Err(err);
        }
        Ok(module)
    }

    /// Reads `name.py` into the module memory and returns its text.
    fn load_source(&mut self, name: &str) -> Result<&'a str, Error> {
        let mut file = [0u8; MAX_NAME_LEN];
        let len = name.len() + 3;
        if len > file.len() {
            return Err(Error::ImportError);
        }
        file[..name.len()].copy_from_slice(name.as_bytes());
        file[name.len()..len].copy_from_slice(b".py");
        let file = core::str::from_utf8(&file[..len]).map_err(|_| Error::ImportError)?;

        let files = self.files.ok_or(Error::ImportError)?;
        let size = match files.borrow_mut().file_size(file) {
            Ok(Some(size)) => size as usize,
            _ => return Err(Error::ImportError), // no module named ...
        };
        let memory = self.module_memory.take().unwrap_or(&mut []);
        if memory.len() < size {
            self.module_memory = Some(memory);
            return Err(Error::OutOfMemory); // the module memory is full
        }
        let (text, rest) = memory.split_at_mut(size);
        self.module_memory = Some(rest);
        let got = files.borrow_mut().read_at(file, 0, text);
        if got != Ok(size) {
            return Err(Error::IoError);
        }
        let text: &'a [u8] = text;
        core::str::from_utf8(text).map_err(|_| Error::ImportError)
    }

    /// `del name` at the top level of a module.
    pub(super) fn module_del(&mut self, dict: ObjRef, name: &str) -> Result<(), Error> {
        let i = (0..self.count(dict))
            .find(|&i| self.str_of(self.slot(dict, 2 * i)) == Some(name))
            .ok_or(Error::UnknownIdentifier)?;
        self.dict_remove(dict, i);
        Ok(())
    }
}
