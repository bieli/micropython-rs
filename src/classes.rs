// ============================================================================
// CLASSES: `class` statements, instances, methods, `@classmethod`,
// `@staticmethod`, `super()` and your own exception classes
// ============================================================================
//
// A class is a heap object laid out like a tuple `(name, base, dict)`; an
// instance is `(class, dict)`. The dicts are ordinary dict objects. The body
// of a `class` statement runs like a function call: its local variables
// (assignments, `def` methods, `import`s) become the class dict.
//
// Attribute lookup: the instance dict, then the class and its bases. A
// function found in a class becomes a method: called on an instance, the
// instance is passed as `self`; a `classmethod` gets the class instead.
//
// A class derived from an exception class (`class MyError(ValueError)`)
// makes exception objects (see `exceptions.rs`) that remember the class, so
// `raise MyError('x')` / `except MyError` work. Such classes can't have
// `__init__` (exception objects have no attribute dict).

use super::{Interpreter, Kwargs, Value};
use crate::ast_parser::MAX_CALL_ARGS;
use crate::heap::{Kind, ObjRef};
use crate::{Error, HardwareBridge, Output};

const C_NAME: usize = 0;
const C_BASE: usize = 1;
const C_DICT: usize = 2;
const I_CLASS: usize = 0;
const I_DICT: usize = 1;
/// Longest inheritance chain followed (guards against loops).
const MAX_BASES: usize = 16;

impl<'a, O: Output, H: HardwareBridge> Interpreter<'a, O, H> {
    pub(super) fn as_class(&self, v: Value<'a>) -> Option<ObjRef> {
        match v {
            Value::Obj(r) if self.heap.kind(r) == Kind::Class => Some(r),
            _ => None,
        }
    }

    pub(super) fn as_instance(&self, v: Value<'a>) -> Option<ObjRef> {
        match v {
            Value::Obj(r) if self.heap.kind(r) == Kind::Instance => Some(r),
            _ => None,
        }
    }

    /// The class of an instance (or of an exception object of a user class).
    pub(super) fn class_of(&self, v: Value<'a>) -> Option<ObjRef> {
        match v {
            Value::Obj(r) if self.heap.kind(r) == Kind::Instance => {
                self.as_class(self.slot(r, I_CLASS))
            }
            Value::Obj(r) if self.heap.kind(r) == Kind::Exception => self.as_class(self.slot(r, 0)),
            _ => None,
        }
    }

    /// The built-in exception class a user class derives from, if any.
    pub(super) fn exception_root(&self, class: ObjRef) -> Option<u8> {
        let mut c = class;
        for _ in 0..MAX_BASES {
            match self.slot(c, C_BASE) {
                Value::ExcClass(e) => return Some(e),
                base => c = self.as_class(base)?,
            }
        }
        None
    }

    /// True if `class` is `of` or derives from it.
    pub(super) fn derives_from(&self, class: ObjRef, of: ObjRef) -> bool {
        let mut c = class;
        for _ in 0..MAX_BASES {
            if c == of {
                return true;
            }
            match self.as_class(self.slot(c, C_BASE)) {
                Some(base) => c = base,
                None => return false,
            }
        }
        false
    }

    /// The value of `name` in a dict object (keys are strings here).
    pub(super) fn dict_get_str(&self, dict: ObjRef, name: &str) -> Option<Value<'a>> {
        (0..self.count(dict))
            .find(|&i| self.str_of(self.slot(dict, 2 * i)) == Some(name))
            .map(|i| self.slot(dict, 2 * i + 1))
    }

    /// `name` in a class or its bases.
    fn class_lookup(&self, class: ObjRef, name: &str) -> Option<Value<'a>> {
        let mut c = class;
        for _ in 0..MAX_BASES {
            let Value::Obj(dict) = self.slot(c, C_DICT) else {
                return None;
            };
            if let Some(v) = self.dict_get_str(dict, name) {
                return Some(v);
            }
            c = self.as_class(self.slot(c, C_BASE))?;
        }
        None
    }

    fn wrapper_kind(&self, v: Value<'a>) -> Option<(Kind, Value<'a>)> {
        match v {
            Value::Obj(r)
                if matches!(self.heap.kind(r), Kind::ClassMethod | Kind::StaticMethod) =>
            {
                Some((self.heap.kind(r), self.slot(r, 0)))
            }
            _ => None,
        }
    }

    /// True if `obj.name` is a method to bind (see `bound_method`).
    pub(super) fn has_class_method(&self, obj: Value<'a>, name: &str) -> bool {
        let class = match (
            self.as_class(obj),
            self.class_of(obj),
            self.super_parts(obj),
        ) {
            (Some(c), _, _) => c,
            (_, Some(c), _) => c,
            (_, _, Some((c, _))) => c,
            _ => return false,
        };
        if let Some(inst) = self.as_instance(obj) {
            if let Value::Obj(dict) = self.slot(inst, I_DICT) {
                if self.dict_get_str(dict, name).is_some() {
                    return false; // instance attributes aren't methods
                }
            }
        }
        match self.class_lookup(class, name) {
            Some(Value::Func(_)) => self.as_class(obj).is_none(),
            Some(v) => matches!(self.wrapper_kind(v), Some((Kind::ClassMethod, _))),
            None => false,
        }
    }

    /// Attributes of classes, instances and `super()` (methods excepted:
    /// those become bound methods).
    pub(super) fn object_attr(&self, obj: Value<'a>, name: &str) -> Option<Option<Value<'a>>> {
        if let Some(class) = self.as_class(obj) {
            if name == "__name__" {
                return Some(Some(self.slot(class, C_NAME)));
            }
            let v = self.class_lookup(class, name);
            return Some(match v.and_then(|v| self.wrapper_kind(v)) {
                Some((Kind::StaticMethod, f)) => Some(f),
                Some(_) => None, // a classmethod: bound
                None => v,
            });
        }
        if let Some(inst) = self.as_instance(obj) {
            let class = self.as_class(self.slot(inst, I_CLASS))?;
            if name == "__class__" {
                return Some(Some(Value::Obj(class)));
            }
            if let Value::Obj(dict) = self.slot(inst, I_DICT) {
                if let Some(v) = self.dict_get_str(dict, name) {
                    return Some(Some(v));
                }
            }
            let v = self.class_lookup(class, name);
            return Some(match v {
                Some(Value::Func(_)) => None, // a method: bound
                Some(v) => match self.wrapper_kind(v) {
                    Some((Kind::StaticMethod, f)) => Some(f),
                    Some(_) => None,
                    None => Some(v),
                },
                None => None,
            });
        }
        if self.super_parts(obj).is_some() {
            return Some(None);
        }
        None
    }

    /// `obj.name = v` for instances and classes.
    pub(super) fn set_attr(
        &mut self,
        obj: Value<'a>,
        name: &'a str,
        v: Value<'a>,
    ) -> Result<(), Error> {
        let dict = if let Some(inst) = self.as_instance(obj) {
            self.slot(inst, I_DICT)
        } else if let Some(class) = self.as_class(obj) {
            self.slot(class, C_DICT)
        } else if let Some(module) = self.module_dict(obj) {
            Value::Obj(module) // `sensors.LIMIT = 10`
        } else {
            return Err(Error::AttributeError);
        };
        let Value::Obj(dict) = dict else {
            return Err(Error::AttributeError);
        };
        self.dict_set(dict, Value::Str(name), v)
    }

    /// Calls `f` with `first` before `args` (`self` or `cls`).
    fn call_with_first(
        &mut self,
        f: Value<'a>,
        first: Value<'a>,
        args: &[Value<'a>],
        kwargs: Kwargs<'_, 'a>,
    ) -> Result<Value<'a>, Error> {
        if args.len() >= MAX_CALL_ARGS {
            return Err(Error::TooManyArguments);
        }
        let mut all = [Value::None; MAX_CALL_ARGS];
        all[0] = first;
        all[1..=args.len()].copy_from_slice(args);
        // For `super()` in the method: its class and its `self`
        let owner = self.owner_class(first, f);
        let saved = (self.method_class, self.method_self);
        (self.method_class, self.method_self) = (owner, first);
        let result = self.call_value(f, &all[..=args.len()], kwargs);
        (self.method_class, self.method_self) = saved;
        result
    }

    /// `obj.name(args)` for classes, instances and `super()`; `None` if
    /// `obj` is none of them.
    #[inline(never)]
    pub(super) fn object_method(
        &mut self,
        obj: Value<'a>,
        name: &str,
        args: &[Value<'a>],
        kwargs: Kwargs<'_, 'a>,
    ) -> Option<Result<Value<'a>, Error>> {
        // `super().name(...)`: look up from the base, bind the original object
        let (class, receiver, is_class) = if let Some((base, this)) = self.super_parts(obj) {
            (base, this, self.as_class(this).is_some())
        } else if let Some(class) = self.as_class(obj) {
            (class, obj, true)
        } else {
            let class = self
                .class_of(obj)
                .filter(|_| self.as_instance(obj).is_some())?;
            (class, obj, false)
        };
        if let Some(inst) = self.as_instance(obj) {
            // A function stored on the instance itself isn't a method
            if let Value::Obj(dict) = self.slot(inst, I_DICT) {
                if let Some(v) = self.dict_get_str(dict, name) {
                    return Some(self.call_value(v, args, kwargs));
                }
            }
        }
        let Some(v) = self.class_lookup(class, name) else {
            return Some(Err(Error::AttributeError));
        };
        Some(match (v, self.wrapper_kind(v)) {
            (Value::Func(_), _) if !is_class => self.call_with_first(v, receiver, args, kwargs),
            (_, Some((Kind::ClassMethod, f))) => {
                let cls = match self.as_class(receiver) {
                    Some(c) => Value::Obj(c),
                    None => self.class_of(receiver).map_or(Value::None, Value::Obj),
                };
                self.call_with_first(f, cls, args, kwargs)
            }
            (_, Some((_, f))) => self.call_value(f, args, kwargs),
            _ => self.call_value(v, args, kwargs),
        })
    }

    /// `Class(args)`: a new instance, initialized by `__init__`.
    #[inline(never)]
    pub(super) fn instantiate(
        &mut self,
        class: ObjRef,
        args: &[Value<'a>],
        kwargs: Kwargs<'_, 'a>,
    ) -> Result<Value<'a>, Error> {
        let base = self.sp;
        let result = (|| {
            if self.exception_root(class).is_some() {
                // An exception object that remembers its class
                if !kwargs.is_empty() {
                    return Err(Error::TypeError);
                }
                self.push(Value::Obj(class))?;
                for &a in args {
                    self.push(a)?;
                }
                self.build(Kind::Tuple, args.len() + 1)?;
                let exc = self.stack[self.sp - 1];
                if let Value::Obj(r) = exc {
                    self.heap.set_kind(r, Kind::Exception);
                }
                return Ok(exc);
            }
            let dict = self.new_container(Kind::Dict, 0)?;
            self.push(Value::Obj(class))?;
            self.push(Value::Obj(dict))?;
            self.build(Kind::Tuple, 2)?;
            let inst = self.stack[self.sp - 1];
            if let Value::Obj(r) = inst {
                self.heap.set_kind(r, Kind::Instance);
            }
            match self.class_lookup(class, "__init__") {
                Some(init @ Value::Func(_)) => {
                    let ret = self.call_with_first(init, inst, args, kwargs)?;
                    if ret != Value::None {
                        return Err(Error::TypeError); // __init__ should return None
                    }
                }
                Some(_) => return Err(Error::TypeError),
                None if !args.is_empty() || !kwargs.is_empty() => return Err(Error::TypeError),
                None => {}
            }
            Ok(inst)
        })();
        self.sp = base;
        result
    }

    /// `classmethod(f)` / `staticmethod(f)`.
    pub(super) fn method_wrapper(&mut self, kind: Kind, f: Value<'a>) -> Result<Value<'a>, Error> {
        let base = self.sp;
        let result = (|| {
            self.push(f)?;
            self.build(Kind::Tuple, 1)?;
            let w = self.stack[self.sp - 1];
            if let Value::Obj(r) = w {
                self.heap.set_kind(r, kind);
            }
            Ok(w)
        })();
        self.sp = base;
        result
    }

    /// `super()` inside a method: the base of the method's class, bound to
    /// its `self` (or `cls`).
    #[inline(never)]
    pub(super) fn make_super(&mut self) -> Result<Value<'a>, Error> {
        let (Some(class), this) = (self.as_class(self.method_class), self.method_self) else {
            let class = super::exceptions::class_by_name("RuntimeError").unwrap_or(0);
            return Err(self.raise_new(class, Some(Value::Str("super(): no arguments"))));
        };
        let Some(base) = self.as_class(self.slot(class, C_BASE)) else {
            return Err(Error::AttributeError); // no user-defined base to call
        };
        let b = self.sp;
        let result = (|| {
            self.push(Value::Obj(base))?;
            self.push(this)?;
            self.build(Kind::Tuple, 2)?;
            let s = self.stack[self.sp - 1];
            if let Value::Obj(r) = s {
                self.heap.set_kind(r, Kind::Super);
            }
            Ok(s)
        })();
        self.sp = b;
        result
    }

    /// `(base class, bound object)` of a `super()` object.
    fn super_parts(&self, v: Value<'a>) -> Option<(ObjRef, Value<'a>)> {
        match v {
            Value::Obj(r) if self.heap.kind(r) == Kind::Super => {
                Some((self.as_class(self.slot(r, 0))?, self.slot(r, 1)))
            }
            _ => None,
        }
    }

    /// The class a function was found in, for `super()`: searched in the
    /// class of `this` and its bases.
    pub(super) fn owner_class(&self, this: Value<'a>, f: Value<'a>) -> Value<'a> {
        let start = self.as_class(this).or_else(|| self.class_of(this));
        let Some(mut c) = start else {
            return Value::None;
        };
        for _ in 0..MAX_BASES {
            if let Value::Obj(dict) = self.slot(c, C_DICT) {
                let n = self.count(dict);
                if (0..n).any(|i| {
                    let v = self.slot(dict, 2 * i + 1);
                    v == f || self.wrapper_kind(v).is_some_and(|(_, w)| w == f)
                }) {
                    return Value::Obj(c);
                }
            }
            match self.as_class(self.slot(c, C_BASE)) {
                Some(base) => c = base,
                None => break,
            }
        }
        Value::None
    }

    /// Calls `obj.__name__(args)` if the class of the instance `obj` defines it.
    pub(super) fn call_dunder(
        &mut self,
        obj: Value<'a>,
        name: &str,
        args: &[Value<'a>],
    ) -> Option<Result<Value<'a>, Error>> {
        let inst = self.as_instance(obj)?;
        let class = self.as_class(self.slot(inst, I_CLASS))?;
        let f @ Value::Func(_) = self.class_lookup(class, name)? else {
            return None;
        };
        Some(self.call_with_first(f, obj, args, &[]))
    }

    /// Like `call_dunder`, with keyword arguments.
    pub(super) fn call_dunder_kw(
        &mut self,
        obj: Value<'a>,
        name: &str,
        args: &[Value<'a>],
        kwargs: Kwargs<'_, 'a>,
    ) -> Option<Result<Value<'a>, Error>> {
        let inst = self.as_instance(obj)?;
        let class = self.as_class(self.slot(inst, I_CLASS))?;
        let f @ Value::Func(_) = self.class_lookup(class, name)? else {
            return None;
        };
        Some(self.call_with_first(f, obj, args, kwargs))
    }

    /// `str(obj)` / `repr(obj)` through `__str__` / `__repr__`, if defined.
    #[inline(never)]
    pub(super) fn custom_str(
        &mut self,
        obj: Value<'a>,
        repr: bool,
    ) -> Option<Result<Value<'a>, Error>> {
        self.as_instance(obj)?;
        let result = match repr {
            false => self
                .call_dunder(obj, "__str__", &[])
                .or_else(|| self.call_dunder(obj, "__repr__", &[])),
            true => self.call_dunder(obj, "__repr__", &[]),
        }?;
        Some(result.and_then(|s| {
            if Self::is_str(s) {
                Ok(s)
            } else {
                Err(Error::TypeError)
            }
        }))
    }
}
