// ============================================================================
// BUILT-INS AS VALUES: built-in functions and types (`len`, `int`, `str`...),
// `isinstance`/`type`, `hasattr`/`getattr`, `callable`, `id`, `dir`,
// `globals()` and the `builtins` module
// ============================================================================
//
// A built-in function or type used as a value (`key=len`, `isinstance(x,
// int)`, `type(x) == str`) is `Value::Builtin(i)`, an index into `NAMES`.
// Calling one goes to the same code as calling the name directly.
//
// `globals()` is `Value::Globals`: a live view of the global variables (and
// `def` functions), like a dict. `globals().clear()` removes them all.

use super::{Interpreter, Value};
use crate::heap::Kind;
use crate::{Error, HardwareBridge, Output};

/// Built-in functions and types. Types come first (`FIRST_FUNCTION`).
const NAMES: &[&str] = &[
    // types: `type()` returns them, `isinstance()` checks them
    "int",
    "bool",
    "str",
    "bytes",
    "list",
    "tuple",
    "dict",
    "range",
    "NoneType",
    "function",
    "builtin_function_or_method",
    "module",
    // functions
    "print",
    "len",
    "abs",
    "min",
    "max",
    "repr",
    "ord",
    "chr",
    "sorted",
    "reversed",
    "sum",
    "any",
    "all",
    "enumerate",
    "zip",
    "isinstance",
    "type",
    "hasattr",
    "getattr",
    "callable",
    "id",
    "dir",
    "globals",
    "help",
    "format",
];
const FIRST_FUNCTION: u8 = 12;
/// Types that can't be called (`NoneType()`, `function()`, `module()`).
const FIRST_UNCALLABLE_TYPE: u8 = 8;

/// `import builtins`: its attributes are the built-in functions, types and
/// exception classes (see `Interpreter::builtins_attr`).
pub(crate) const BUILTINS_MODULE: crate::Module = crate::Module {
    name: "builtins",
    doc: "Built-in functions, types and exceptions",
    functions: &[],
    constants: &[],
};

pub(super) fn builtin_by_name(name: &str) -> Option<u8> {
    NAMES.iter().position(|&n| n == name).map(|i| i as u8)
}

pub(crate) fn builtin_name(i: u8) -> &'static str {
    NAMES.get(i as usize).copied().unwrap_or("?")
}

pub(crate) fn is_type(i: u8) -> bool {
    i < FIRST_FUNCTION
}

/// Writes `<class 'int'>` or `<built-in function len>`.
pub(crate) fn write_builtin(i: u8, sink: &mut dyn FnMut(&str)) {
    if is_type(i) {
        for part in ["<class '", builtin_name(i), "'>"] {
            sink(part);
        }
    } else {
        sink("<built-in function ");
        sink(builtin_name(i));
        sink(">");
    }
}

impl<'a, O: Output, H: HardwareBridge> Interpreter<'a, O, H> {
    /// The built-in type of a value (`type(v)`); exceptions have their class.
    pub(super) fn type_of(&self, v: Value<'a>) -> Value<'a> {
        if let Some(class) = self.exc_class(v) {
            return Value::ExcClass(class);
        }
        let name = match v {
            Value::None => "NoneType",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Str(_) | Value::HStr(_) => "str",
            Value::Bytes(_) | Value::HBytes(_) => "bytes",
            Value::Obj(r) => match self.heap.kind(r) {
                Kind::List => "list",
                Kind::Tuple => "tuple",
                Kind::Dict => "dict",
                _ => "range",
            },
            Value::Globals => "dict",
            Value::Module(_) => "module",
            Value::Func(_) => "function",
            Value::NativeFn { .. } | Value::Builtin(_) => "builtin_function_or_method",
            Value::ExcClass(_) => "type",
        };
        builtin_by_name(name).map_or(Value::None, Value::Builtin)
    }

    /// `isinstance(v, spec)`: `spec` is a type, an exception class or a tuple of them.
    fn isinstance(&self, v: Value<'a>, spec: Value<'a>) -> Result<bool, Error> {
        match spec {
            Value::Builtin(t) if is_type(t) => {
                let actual = self.type_of(v);
                // `bool` is a subclass of `int`
                Ok(actual == spec || builtin_name(t) == "int" && actual == Value::Builtin(1))
            }
            Value::ExcClass(_) => match self.exc_class(v) {
                Some(class) => self.exc_matches(class, spec),
                None => Ok(false),
            },
            Value::Obj(r) if self.heap.kind(r) == Kind::Tuple => {
                for i in 0..self.count(r) {
                    if self.isinstance(v, self.slot(r, i))? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            _ => Err(Error::TypeError),
        }
    }

    /// `obj.name` as a value, or `None` if there's no such attribute
    /// (modules, exception objects, `__name__` of classes and functions).
    pub(super) fn get_attr(
        &mut self,
        obj: Value<'a>,
        name: &str,
    ) -> Result<Option<Value<'a>>, Error> {
        if name == "__name__" {
            let n = match obj {
                Value::Builtin(i) => Some(builtin_name(i)),
                Value::ExcClass(c) => Some(super::exceptions::class_name(c)),
                Value::Module(m) => self.module(m).map(|m| m.name),
                Value::Func(f) => {
                    self.funcs[f as usize].map(|f| if f.lambda { "<lambda>" } else { f.name })
                }
                _ => None,
            };
            return Ok(n.map(Value::Str));
        }
        match obj {
            Value::Module(m) => Ok(self.module_attr(m, name).ok()),
            Value::Obj(_) if self.exc_class(obj).is_some() => match self.exc_attr(obj, name) {
                Ok(v) => Ok(Some(v)),
                Err(Error::AttributeError) => Ok(None),
                Err(e) => Err(e),
            },
            _ => Ok(None),
        }
    }

    /// True if `obj.name(...)` is a method of a string, bytes or container.
    fn has_method(&self, obj: Value<'a>, name: &str) -> bool {
        const STR: &[&str] = &[
            "upper",
            "lower",
            "strip",
            "lstrip",
            "rstrip",
            "startswith",
            "endswith",
            "find",
            "count",
            "replace",
            "split",
            "join",
            "encode",
            "isdigit",
            "isalpha",
            "isspace",
            "isalnum",
            "isupper",
            "islower",
            "format",
        ];
        const BYTES: &[&str] = &["hex", "decode", "startswith", "endswith", "find"];
        const LIST: &[&str] = &[
            "append", "extend", "insert", "pop", "remove", "index", "count", "clear", "reverse",
            "sort", "copy",
        ];
        const DICT: &[&str] = &[
            "get",
            "pop",
            "setdefault",
            "update",
            "keys",
            "values",
            "items",
            "clear",
            "copy",
        ];
        let table: &[&str] = match obj {
            Value::Str(_) | Value::HStr(_) => STR,
            Value::Bytes(_) | Value::HBytes(_) => BYTES,
            Value::Globals => DICT,
            _ => match self.obj_kind(obj) {
                Some(Kind::List) => LIST,
                Some(Kind::Tuple) => &["index", "count"],
                Some(Kind::Dict) => DICT,
                _ => &[],
            },
        };
        table.contains(&name)
    }

    /// `builtins.name`: built-in functions, types and exception classes.
    pub(super) fn builtins_attr(name: &str) -> Option<Value<'a>> {
        builtin_by_name(name)
            .map(Value::Builtin)
            .or_else(|| super::exceptions::class_by_name(name).map(Value::ExcClass))
    }

    /// Calls a built-in that takes values of any kind; `None` if `name`
    /// isn't one of them.
    pub(super) fn value_builtin(
        &mut self,
        name: &str,
        args: &[Value<'a>],
    ) -> Option<Result<Value<'a>, Error>> {
        Some(match (name, args) {
            ("isinstance", [v, spec]) => self.isinstance(*v, *spec).map(Value::Bool),
            ("type", [v]) => Ok(self.type_of(*v)),
            ("callable", [v]) => Ok(Value::Bool(
                matches!(
                    v,
                    Value::Func(_) | Value::NativeFn { .. } | Value::ExcClass(_)
                ) || matches!(v, Value::Builtin(i) if *i < FIRST_UNCALLABLE_TYPE || *i >= FIRST_FUNCTION),
            )),
            ("hasattr", [obj, attr]) => {
                let Some(attr) = self.str_of(*attr) else {
                    return Some(Err(Error::TypeError));
                };
                let mut name_buf = [0u8; 32];
                let Some(attr) = copy_name(attr, &mut name_buf) else {
                    return Some(Ok(Value::Bool(false)));
                };
                if self.has_method(*obj, attr) {
                    return Some(Ok(Value::Bool(true)));
                }
                self.get_attr(*obj, attr).map(|v| Value::Bool(v.is_some()))
            }
            ("getattr", [obj, attr] | [obj, attr, _]) => {
                let Some(attr) = self.str_of(*attr) else {
                    return Some(Err(Error::TypeError));
                };
                let mut name_buf = [0u8; 32];
                let found = match copy_name(attr, &mut name_buf) {
                    Some(attr) => self.get_attr(*obj, attr),
                    None => Ok(None),
                };
                match (found, args.get(2)) {
                    (Ok(Some(v)), _) => Ok(v),
                    (Ok(None), Some(default)) => Ok(*default),
                    (Ok(None), None) => Err(Error::AttributeError),
                    (Err(e), _) => Err(e),
                }
            }
            // Unique while the object lives: heap objects by handle, source
            // strings by address, everything else by its value
            ("id", [v]) => Ok(Value::Int(match *v {
                Value::HStr(r) | Value::HBytes(r) | Value::Obj(r) => 0x1_0000 + r.index() as i64,
                Value::Str(s) => s.as_ptr() as usize as i64,
                Value::Bytes(b) => b.as_ptr() as usize as i64,
                Value::Int(n) => n.wrapping_mul(2).wrapping_add(1),
                Value::None => 0x10,
                Value::Bool(b) => 0x12 + b as i64 * 2,
                Value::Globals => 0x20,
                Value::Module(m) => 0x100 + m as i64 * 2,
                Value::Func(f) => 0x400 + f as i64 * 2,
                Value::Builtin(b) => 0x800 + b as i64 * 2,
                Value::ExcClass(c) => 0xC00 + c as i64 * 2,
                Value::NativeFn { module, func } => {
                    0x1000 + (module as i64 * 256 + func as i64) * 2
                }
            })),
            ("globals", []) => Ok(Value::Globals),
            ("dir", []) => self.globals_view("keys", &[]),
            ("dir", [obj]) => self.dir(*obj),
            _ => return None,
        })
    }

    /// `dir(obj)`: the names of a module, or the methods of a value.
    fn dir(&mut self, obj: Value<'a>) -> Result<Value<'a>, Error> {
        let list = self.new_container(Kind::List, 0)?;
        let base = self.sp;
        self.push(Value::Obj(list))?;
        let result = (|| {
            self.list_append(list, Value::Str("__name__"))?;
            if let Value::Module(m) = obj {
                let module = self.module(m).ok_or(Error::TypeError)?;
                for f in module.functions {
                    self.list_append(list, Value::Str(f.name))?;
                }
                for &(name, _) in module.constants {
                    self.list_append(list, Value::Str(name))?;
                }
            }
            Ok(Value::Obj(list))
        })();
        self.sp = base;
        result
    }

    // ---- globals() -------------------------------------------------------

    /// Number of global variables (all variables at the top level).
    fn globals_count(&self) -> usize {
        if self.call_depth == 0 {
            self.nvars
        } else {
            self.globals_len
        }
    }

    /// A dict with the global variables and `def` functions (a snapshot).
    pub(super) fn globals_dict(&mut self) -> Result<Value<'a>, Error> {
        let n = self.globals_count();
        let d = self.new_container(Kind::Dict, n)?;
        let base = self.sp;
        self.push(Value::Obj(d))?;
        let result = (|| {
            for i in 0..n {
                let (name, v) = self.vars[i];
                self.dict_set(d, Value::Str(name), v)?;
            }
            for i in 0..self.funcs.len() {
                if let Some(f) = self.funcs[i] {
                    if !f.lambda {
                        self.dict_set(d, Value::Str(f.name), Value::Func(i as u8))?;
                    }
                }
            }
            Ok(Value::Obj(d))
        })();
        self.sp = base;
        result
    }

    /// Methods of `globals()`: `clear()` changes the globals; the others
    /// work on a snapshot dict.
    pub(super) fn globals_view(
        &mut self,
        name: &str,
        args: &[Value<'a>],
    ) -> Result<Value<'a>, Error> {
        if name == "clear" && args.is_empty() {
            let n = self.globals_count();
            self.vars.copy_within(n..self.nvars, 0);
            self.nvars -= n;
            if self.call_depth > 0 {
                self.globals_len = 0;
            }
            for f in self.funcs.iter_mut() {
                if f.is_some_and(|f| !f.lambda) {
                    *f = None;
                }
            }
            return Ok(Value::None);
        }
        let d = self.globals_dict()?;
        let base = self.sp;
        self.push(d)?;
        let result = self
            .container_method(d, name, args)
            .unwrap_or(Err(Error::AttributeError));
        self.sp = base;
        result
    }

    /// `globals()[name]`.
    pub(super) fn globals_get(&self, key: Value<'a>) -> Result<Value<'a>, Error> {
        let name = self.str_of(key).ok_or(Error::KeyError)?;
        let n = self.globals_count();
        if let Some(i) = self.find_in(0..n, name) {
            return Ok(self.vars[i].1);
        }
        self.funcs
            .iter()
            .position(|f| f.is_some_and(|f| !f.lambda && f.name == name))
            .map(|i| Value::Func(i as u8))
            .ok_or(Error::KeyError)
    }

    /// `name in globals()`.
    pub(super) fn globals_contains(&self, key: Value<'a>) -> bool {
        self.globals_get(key).is_ok()
    }

    /// `globals()[name] = v`: the name must be written in the script
    /// (variable names live as long as the script source).
    pub(super) fn globals_set(&mut self, key: Value<'a>, v: Value<'a>) -> Result<(), Error> {
        let Value::Str(name) = key else {
            return Err(Error::TypeError);
        };
        if self.call_depth == 0 {
            self.set_var(name, v)
        } else {
            self.set_global(name, v)
        }
    }

    /// `del globals()[name]`.
    pub(super) fn globals_del(&mut self, key: Value<'a>) -> Result<(), Error> {
        let name = self.str_of(key).ok_or(Error::KeyError)?;
        let n = self.globals_count();
        if let Some(i) = self.find_in(0..n, name) {
            self.vars.copy_within(i + 1..self.nvars, i);
            self.nvars -= 1;
            if self.call_depth > 0 {
                self.globals_len -= 1;
            }
            return Ok(());
        }
        let slot = self
            .funcs
            .iter()
            .position(|f| f.is_some_and(|f| !f.lambda && f.name == name))
            .ok_or(Error::KeyError)?;
        self.funcs[slot] = None;
        Ok(())
    }

    /// `help('modules')`: one module per line (the format Thonny reads).
    pub(super) fn print_modules(&mut self) {
        for i in 0..self.module_count() as u8 {
            if let Some(m) = self.module(i) {
                self.out.write_str(m.name);
                self.out.write_str("\n");
            }
        }
        self.out.write_str("Plus any modules on the filesystem\n");
    }
}

/// Copies an attribute name out of the heap, so that `self` can be borrowed
/// mutably while it's used (no attribute name is longer than 32 bytes).
fn copy_name<'n>(name: &str, buf: &'n mut [u8; 32]) -> Option<&'n str> {
    let bytes = name.as_bytes();
    if bytes.len() > buf.len() {
        return None;
    }
    buf[..bytes.len()].copy_from_slice(bytes);
    core::str::from_utf8(&buf[..bytes.len()]).ok()
}
