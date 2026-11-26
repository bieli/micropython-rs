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
    "float",
    "NoneType",
    "function",
    "builtin_function_or_method",
    "module",
    "TextIOWrapper",
    "object",
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
    "open",
    "classmethod",
    "staticmethod",
    "super",
    "hex",
    "bin",
    "oct",
    "round",
    "divmod",
    "pow",
    "_irq_handler",
];
const FIRST_FUNCTION: u8 = 15;
/// Types that can't be called (`NoneType()`, `function()`, `module()`).
const FIRST_UNCALLABLE_TYPE: u8 = 9;

/// `import builtins`: its attributes are the built-in functions, types and
/// exception classes (see `Interpreter::builtins_attr`).
pub(crate) const BUILTINS_MODULE: crate::Module = crate::Module {
    name: "builtins",
    doc: "Built-in functions, types and exceptions",
    functions: &[],
    constants: &[],
};

/// `import sys`: `path`, `version`, `version_info`, `platform`,
/// `byteorder`, `maxsize`, `argv`, `modules` (see `Interpreter::sys_attr`)
/// and `exit()`.
#[cfg(feature = "stdlib")]
pub(crate) const SYS_MODULE: crate::Module = crate::Module {
    name: "sys",
    doc: "System information (path, version, platform, ...), exit()",
    functions: &[crate::HwFn::with_args(
        "exit",
        0,
        1,
        "[code]",
        "Stop the program (raises SystemExit)",
    )],
    constants: &[("maxsize", i64::MAX)],
};

/// `import gc`: MicroPython's `collect()`, `mem_free()`, `mem_alloc()`,
/// `enable()`/`disable()`/`isenabled()` (collection can't be switched off:
/// it runs whenever the heap is full), and `objects_free()`, this
/// interpreter's own: free entries of the heap's object table, which can run
/// out while bytes are still free.
#[cfg(feature = "stdlib")]
pub(crate) const GC_MODULE: crate::Module = crate::Module {
    name: "gc",
    doc: "The heap: collect(), mem_free(), mem_alloc(), objects_free()",
    functions: &[
        crate::HwFn::new("collect", "", "Free unreachable objects now"),
        crate::HwFn::new("mem_free", "", "Free heap bytes"),
        crate::HwFn::new(
            "mem_alloc",
            "",
            "Heap bytes in use (garbage too, until collect())",
        ),
        crate::HwFn::new(
            "objects_free",
            "",
            "Free object-table entries (not in MicroPython)",
        ),
        crate::HwFn::new("enable", "", "Does nothing: collection is always on"),
        crate::HwFn::new("disable", "", "Does nothing: collection is always on"),
        crate::HwFn::new("isenabled", "", "True"),
    ],
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
        if let Some(class) = self.class_of(v) {
            return Value::Obj(class);
        }
        if let Some(class) = self.exc_class(v) {
            return Value::ExcClass(class);
        }
        let name = match v {
            Value::None => "NoneType",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::Str(_) | Value::HStr(_) => "str",
            Value::Bytes(_) | Value::HBytes(_) => "bytes",
            Value::Obj(r) => match self.heap.kind(r) {
                Kind::List => "list",
                Kind::Tuple => "tuple",
                Kind::Dict => "dict",
                Kind::File => "TextIOWrapper",
                Kind::BoundMethod => "builtin_function_or_method",
                Kind::Class => "type",
                Kind::PyModule => "module",
                _ if false => "object",
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
            Value::Obj(r) if self.heap.kind(r) == Kind::Class => {
                Ok(self.class_of(v).is_some_and(|c| self.derives_from(c, r)))
            }
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
        if let Some(found) = self.object_attr(obj, name) {
            return Ok(found);
        }
        if let Some(found) = self.py_module_attr(obj, name) {
            return Ok(found);
        }
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
            Value::Module(m) if self.module(m).is_some_and(|m| m.name == "sys") => {
                match self.sys_attr(name)? {
                    Some(v) => Ok(Some(v)),
                    None => Ok(self.module_attr(m, name).ok()),
                }
            }
            Value::Module(m) => Ok(self.module_attr(m, name).ok()),
            Value::Obj(_) if self.file_attr(obj, name).is_some() => Ok(self.file_attr(obj, name)),
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
        if self.has_class_method(obj, name) {
            return true;
        }
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
                Some(Kind::File) => &[
                    "read",
                    "readline",
                    "readlines",
                    "write",
                    "close",
                    "flush",
                    "tell",
                    "seek",
                ],
                _ => &[],
            },
        };
        table.contains(&name)
    }

    /// `obj.name` for a method (`r = f.read`, `up = s.upper`): a bound
    /// method object, or `None` if `obj` has no such method.
    pub(super) fn bound_method(
        &mut self,
        obj: Value<'a>,
        name: &'a str,
    ) -> Result<Option<Value<'a>>, Error> {
        if !self.has_method(obj, name) {
            return Ok(None);
        }
        let base = self.sp;
        let result = (|| {
            self.push(obj)?;
            self.push(Value::Str(name))?;
            self.build(Kind::Tuple, 2)?;
            let v = self.stack[self.sp - 1];
            if let Value::Obj(r) = v {
                self.heap.set_kind(r, Kind::BoundMethod);
            }
            Ok(Some(v))
        })();
        self.sp = base;
        result
    }

    /// Calls a bound method object.
    pub(super) fn call_bound(
        &mut self,
        method: crate::heap::ObjRef,
        args: &[Value<'a>],
        kwargs: super::Kwargs<'_, 'a>,
    ) -> Result<Value<'a>, Error> {
        let receiver = self.slot(method, 0);
        let mut buf = [0u8; 32];
        let name = self.str_of(self.slot(method, 1)).unwrap_or("");
        let name = copy_name(name, &mut buf).ok_or(Error::AttributeError)?;
        if kwargs.is_empty() {
            self.call_method(receiver, name, args)
        } else {
            self.call_method_kw(receiver, name, args, kwargs)
        }
    }

    /// `sys.name` (values that aren't numbers, made when they're read).
    pub(super) fn sys_attr(&mut self, name: &str) -> Result<Option<Value<'a>>, Error> {
        let parts: &[Value<'a>] = match name {
            "path" => &[Value::Str(""), Value::Str("/lib")],
            "argv" => &[],
            "version_info" => &[Value::Int(3), Value::Int(4), Value::Int(0)],
            "version" => {
                return Ok(Some(Value::Str(concat!(
                    "3.4.0; micropython-rs ",
                    env!("CARGO_PKG_VERSION")
                ))))
            }
            "platform" => return Ok(Some(Value::Str("micropython-rs"))),
            "byteorder" => return Ok(Some(Value::Str("little"))),
            "modules" => {
                return self
                    .new_container(Kind::Dict, 0)
                    .map(|d| Some(Value::Obj(d)))
            }
            _ => return Ok(None),
        };
        let base = self.sp;
        let result = (|| {
            for &p in parts {
                self.push(p)?;
            }
            let kind = if name == "version_info" {
                Kind::Tuple
            } else {
                Kind::List
            };
            self.build(kind, parts.len())?;
            Ok(Some(self.stack[self.sp - 1]))
        })();
        self.sp = base;
        result
    }

    /// `builtins.name`: built-in functions, types and exception classes.
    pub(super) fn builtins_attr(name: &str) -> Option<Value<'a>> {
        builtin_by_name(name)
            .map(Value::Builtin)
            .or_else(|| super::exceptions::class_by_name(name).map(Value::ExcClass))
    }

    /// Calls a built-in that takes values of any kind; `None` if `name`
    /// isn't one of them.
    #[inline(never)]
    pub(super) fn value_builtin(
        &mut self,
        name: &str,
        args: &[Value<'a>],
    ) -> Option<Result<Value<'a>, Error>> {
        Some(match (name, args) {
            ("isinstance", [v, spec]) => self.isinstance(*v, *spec).map(Value::Bool),
            ("float", _) => self.float_builtin(args),
            ("_irq_handler", [f]) => {
                self.set_event_handler(*f);
                Ok(Value::None)
            }
            ("round", _) => self.round_builtin(args),
            ("divmod", [a, b]) => self.divmod_builtin(*a, *b),
            ("pow", [a, b]) => super::binop(crate::ast_parser::BinOp::Pow, *a, *b),
            ("pow", [a, b, m]) => match (a.as_int(), b.as_int(), m.as_int()) {
                (Some(base), Some(exp), Some(m)) if exp >= 0 && m != 0 => {
                    Ok(Value::Int(pow_mod(base, exp, m)))
                }
                (Some(_), Some(_), Some(0)) => Err(Error::ValueError),
                _ => Err(Error::TypeError),
            },
            ("type", [v]) => Ok(self.type_of(*v)),
            ("callable", [v]) => Ok(Value::Bool(
                matches!(
                    v,
                    Value::Func(_) | Value::NativeFn { .. } | Value::ExcClass(_)
                ) || matches!(v, Value::Builtin(i) if *i < FIRST_UNCALLABLE_TYPE || *i >= FIRST_FUNCTION)
                    || matches!(self.obj_kind(*v), Some(Kind::Class | Kind::BoundMethod)),
            )),
            ("classmethod", [f]) => self.method_wrapper(Kind::ClassMethod, *f),
            ("staticmethod", [f]) => self.method_wrapper(Kind::StaticMethod, *f),
            ("super", []) => self.make_super(),
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
                Value::Float(f) => (f.to_bits() >> 2) as i64 | 0x4000_0000_0000_0000,
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
                    if f.named() {
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
    #[inline(never)]
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
                if f.is_some_and(|f| f.named()) {
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
            .position(|f| f.is_some_and(|f| f.named() && f.name == name))
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
            .position(|f| f.is_some_and(|f| f.named() && f.name == name))
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

/// `pow(base, exp, m)`: modular power without overflow (Python's sign rules:
/// the result has the sign of `m`).
fn pow_mod(base: i64, mut exp: i64, m: i64) -> i64 {
    let m = m as i128;
    let mut result: i128 = 1 % m;
    let mut b = (base as i128).rem_euclid(m);
    while exp > 0 {
        if exp & 1 == 1 {
            result = (result * b).rem_euclid(m);
        }
        b = (b * b).rem_euclid(m);
        exp >>= 1;
    }
    // rem_euclid gives 0..|m|; Python's % takes the sign of m
    let r = result.rem_euclid(m.abs());
    (if m < 0 && r != 0 { r + m } else { r }) as i64
}
