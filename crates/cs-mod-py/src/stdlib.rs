//! Python 3 builtins, string/list/dict/set methods, f-string formatting, and `math`/`random`/`json` modules.

use std::collections::BTreeMap;

use crate::bindings;
use crate::error::PyError;
use crate::json_codec::{json_dumps, json_loads};
use crate::value::{HeapObj, NativeObjKind, PyKey, PyVal};
use crate::vm::{HostBridge, MAX_SEQ_LEN, MAX_STR_LEN, PyVm, normalize_index};

pub fn install_builtins_and_stdlib(vm: &mut PyVm) -> Result<(), PyError> {
    let builtin_names = [
        "len",
        "range",
        "min",
        "max",
        "abs",
        "int",
        "float",
        "str",
        "bool",
        "round",
        "enumerate",
        "sorted",
        "reversed",
        "sum",
        "any",
        "all",
        "isinstance",
        "type",
        "print",
        "list",
        "tuple",
        "dict",
        "set",
        "zip",
        "map",
        "filter",
        "chr",
        "ord",
        "hex",
        "oct",
        "bin",
        "pow",
        "divmod",
        "hasattr",
        "getattr",
        "setattr",
        "callable",
        "repr",
        "format",
    ];
    for name in builtin_names {
        let id = vm.alloc(HeapObj::NativeFn(format!("builtin.{name}")))?;
        vm.builtins.insert(name.to_string(), PyVal::Handle(id));
    }

    let exc_types = [
        "BaseException",
        "Exception",
        "ValueError",
        "TypeError",
        "KeyError",
        "IndexError",
        "AttributeError",
        "NameError",
        "ZeroDivisionError",
        "RuntimeError",
        "AssertionError",
        "LookupError",
        "ArithmeticError",
        "PermissionError",
        "NotImplementedError",
        "StopIteration",
        "RecursionError",
        "MemoryError",
        "ImportError",
        "ModuleNotFoundError",
    ];
    for exc in exc_types {
        let id = vm.alloc(HeapObj::Native(NativeObjKind::ExceptionType(
            exc.to_string(),
        )))?;
        vm.builtins.insert(exc.to_string(), PyVal::Handle(id));
    }

    // `math` module
    let mut math_attrs = BTreeMap::new();
    math_attrs.insert("pi".to_string(), PyVal::Float(std::f64::consts::PI));
    math_attrs.insert("e".to_string(), PyVal::Float(std::f64::consts::E));
    math_attrs.insert("tau".to_string(), PyVal::Float(std::f64::consts::TAU));
    math_attrs.insert("inf".to_string(), PyVal::Float(f64::INFINITY));
    math_attrs.insert("nan".to_string(), PyVal::Float(f64::NAN));
    for f in [
        "sqrt", "sin", "cos", "tan", "asin", "acos", "atan", "atan2", "floor", "ceil", "trunc",
        "fabs", "pow", "log", "log2", "log10", "exp", "hypot", "dist", "degrees", "radians",
        "isnan", "isinf", "isfinite", "gcd", "fmod", "copysign",
    ] {
        let fid = vm.alloc(HeapObj::NativeFn(format!("math.{f}")))?;
        math_attrs.insert(f.to_string(), PyVal::Handle(fid));
    }
    let math_id = vm.alloc(HeapObj::Native(NativeObjKind::Module {
        name: "math".to_string(),
        attrs: math_attrs,
    }))?;
    vm.modules.insert("math".to_string(), PyVal::Handle(math_id));

    // `random` module
    let mut rand_attrs = BTreeMap::new();
    for f in [
        "seed", "random", "randint", "randrange", "uniform", "choice", "shuffle", "sample",
    ] {
        let fid = vm.alloc(HeapObj::NativeFn(format!("random.{f}")))?;
        rand_attrs.insert(f.to_string(), PyVal::Handle(fid));
    }
    let rand_id = vm.alloc(HeapObj::Native(NativeObjKind::Module {
        name: "random".to_string(),
        attrs: rand_attrs,
    }))?;
    vm.modules
        .insert("random".to_string(), PyVal::Handle(rand_id));

    // `json` module
    let mut json_attrs = BTreeMap::new();
    for f in ["dumps", "loads"] {
        let fid = vm.alloc(HeapObj::NativeFn(format!("json.{f}")))?;
        json_attrs.insert(f.to_string(), PyVal::Handle(fid));
    }
    let json_id = vm.alloc(HeapObj::Native(NativeObjKind::Module {
        name: "json".to_string(),
        attrs: json_attrs,
    }))?;
    vm.modules.insert("json".to_string(), PyVal::Handle(json_id));

    Ok(())
}

pub fn call_native_fn(
    vm: &mut PyVm,
    name: &str,
    args: Vec<PyVal>,
    kwargs: BTreeMap<String, PyVal>,
    mut host: Option<&mut HostBridge<'_>>,
) -> Result<PyVal, PyError> {
    if let Some(cs_fn) = name.strip_prefix("cs16.") {
        return bindings::call_cs16_fn(vm, cs_fn, args, kwargs, host);
    }
    if let Some(m_fn) = name.strip_prefix("math.") {
        return call_math_fn(vm, m_fn, args);
    }
    if let Some(r_fn) = name.strip_prefix("random.") {
        return call_random_fn(vm, r_fn, args);
    }
    if let Some(j_fn) = name.strip_prefix("json.") {
        return call_json_fn(vm, j_fn, args, kwargs);
    }
    let b_fn = name.strip_prefix("builtin.").unwrap_or(name);
    match b_fn {
        "len" => {
            check_argc(vm, "len", &args, 1, 1)?;
            let n = py_len(vm, &args[0], host.as_deref())?;
            Ok(PyVal::Int(n as i64))
        }
        "range" => {
            check_argc(vm, "range", &args, 1, 3)?;
            let (start, stop, step) = match args.len() {
                1 => (0, require_int(vm, &args[0], "range")?, 1),
                2 => (
                    require_int(vm, &args[0], "range")?,
                    require_int(vm, &args[1], "range")?,
                    1,
                ),
                _ => (
                    require_int(vm, &args[0], "range")?,
                    require_int(vm, &args[1], "range")?,
                    require_int(vm, &args[2], "range")?,
                ),
            };
            if step == 0 {
                return Err(vm.runtime_err("ValueError", "range() arg 3 must not be zero"));
            }
            let mut items = Vec::new();
            let mut cur = start;
            while (step > 0 && cur < stop) || (step < 0 && cur > stop) {
                vm.consume_fuel(1)?;
                if items.len() >= MAX_SEQ_LEN {
                    return Err(vm.runtime_err("MemoryError", "range() exceeds sequence limit"));
                }
                items.push(PyVal::Int(cur));
                cur = cur.saturating_add(step);
            }
            let id = vm.alloc(HeapObj::List(items))?;
            Ok(PyVal::Handle(id))
        }
        "abs" => {
            check_argc(vm, "abs", &args, 1, 1)?;
            match &args[0] {
                PyVal::Int(n) => Ok(PyVal::Int(n.abs())),
                PyVal::Bool(b) => Ok(PyVal::Int(i64::from(*b))),
                PyVal::Float(f) => Ok(PyVal::Float(f.abs())),
                _ => Err(vm.runtime_err("TypeError", "bad operand type for abs()")),
            }
        }
        "round" => {
            check_argc(vm, "round", &args, 1, 2)?;
            let Some(f) = args[0].as_float() else {
                return Err(vm.runtime_err("TypeError", "round() requires a number"));
            };
            if args.len() == 1 || matches!(args.get(1), Some(PyVal::None)) {
                Ok(PyVal::Int(f.round() as i64))
            } else {
                let digits = require_int(vm, &args[1], "round")?;
                let factor = 10f64.powi(digits.clamp(-300, 300) as i32);
                Ok(PyVal::Float((f * factor).round() / factor))
            }
        }
        "int" => {
            if args.is_empty() {
                return Ok(PyVal::Int(0));
            }
            let base = if args.len() >= 2 {
                Some(require_int(vm, &args[1], "int")?)
            } else {
                None
            };
            match &args[0] {
                PyVal::Int(n) => Ok(PyVal::Int(*n)),
                PyVal::Bool(b) => Ok(PyVal::Int(i64::from(*b))),
                PyVal::Float(f) => Ok(PyVal::Int(*f as i64)),
                PyVal::Str(s) => {
                    let t = s.trim();
                    let parsed = if let Some(b) = base {
                        if !(2..=36).contains(&b) && b != 0 {
                            return Err(vm.runtime_err("ValueError", "int() base must be >= 2 and <= 36"));
                        }
                        let (rad, slice) = if b == 0 {
                            if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
                                (16, h)
                            } else if let Some(o) = t.strip_prefix("0o").or_else(|| t.strip_prefix("0O")) {
                                (8, o)
                            } else if let Some(bi) = t.strip_prefix("0b").or_else(|| t.strip_prefix("0B")) {
                                (2, bi)
                            } else {
                                (10, t)
                            }
                        } else {
                            (b as u32, t)
                        };
                        i64::from_str_radix(&slice.replace('_', ""), rad)
                    } else {
                        t.replace('_', "").parse::<i64>()
                    };
                    parsed
                        .map(PyVal::Int)
                        .map_err(|_| vm.runtime_err("ValueError", format!("invalid literal for int(): '{s}'")))
                }
                _ => Err(vm.runtime_err("TypeError", "int() argument must be a string or number")),
            }
        }
        "float" => {
            if args.is_empty() {
                return Ok(PyVal::Float(0.0));
            }
            match &args[0] {
                PyVal::Float(f) => Ok(PyVal::Float(*f)),
                PyVal::Int(n) => Ok(PyVal::Float(*n as f64)),
                PyVal::Bool(b) => Ok(PyVal::Float(if *b { 1.0 } else { 0.0 })),
                PyVal::Str(s) => s
                    .trim()
                    .parse::<f64>()
                    .map(PyVal::Float)
                    .map_err(|_| vm.runtime_err("ValueError", format!("could not convert string to float: '{s}'"))),
                _ => Err(vm.runtime_err("TypeError", "float() argument must be a string or number")),
            }
        }
        "str" => {
            if args.is_empty() {
                Ok(PyVal::Str(String::new()))
            } else {
                Ok(PyVal::Str(vm.val_to_str(&args[0])))
            }
        }
        "repr" => {
            check_argc(vm, "repr", &args, 1, 1)?;
            Ok(PyVal::Str(vm.val_to_repr(&args[0])))
        }
        "format" => {
            check_argc(vm, "format", &args, 1, 2)?;
            let spec = args.get(1).and_then(PyVal::as_str).unwrap_or("");
            Ok(PyVal::Str(format_py_val(vm, &args[0], spec)?))
        }
        "bool" => {
            if args.is_empty() {
                Ok(PyVal::Bool(false))
            } else {
                Ok(PyVal::Bool(vm.is_truthy(&args[0])))
            }
        }
        "list" => {
            let items = if args.is_empty() {
                Vec::new()
            } else {
                vm.to_vec(&args[0])?
            };
            let id = vm.alloc(HeapObj::List(items))?;
            Ok(PyVal::Handle(id))
        }
        "tuple" => {
            let items = if args.is_empty() {
                Vec::new()
            } else {
                vm.to_vec(&args[0])?
            };
            Ok(PyVal::Tuple(items))
        }
        "set" => {
            let mut keys = Vec::new();
            if let Some(arg) = args.first() {
                for v in vm.to_vec(arg)? {
                    let k = PyKey::from_val(&v)
                        .ok_or_else(|| vm.runtime_err("TypeError", "unhashable type in set()"))?;
                    if !keys.contains(&k) {
                        keys.push(k);
                    }
                }
            }
            let id = vm.alloc(HeapObj::Set(keys))?;
            Ok(PyVal::Handle(id))
        }
        "dict" => {
            let mut entries: Vec<(PyKey, PyVal)> = Vec::new();
            if let Some(arg) = args.first() {
                if let PyVal::Handle(id) = arg
                    && let Some(HeapObj::Dict(d)) = vm.heap.get(*id)
                {
                    entries = d.clone();
                } else {
                    for pair in vm.to_vec(arg)? {
                        let kv = vm.to_vec(&pair)?;
                        if kv.len() != 2 {
                            return Err(vm.runtime_err("ValueError", "dict update sequence element has wrong length"));
                        }
                        let k = PyKey::from_val(&kv[0])
                            .ok_or_else(|| vm.runtime_err("TypeError", "unhashable dict key"))?;
                        if let Some(pos) = entries.iter().position(|(ek, _)| ek == &k) {
                            entries[pos].1 = kv[1].clone();
                        } else {
                            entries.push((k, kv[1].clone()));
                        }
                    }
                }
            }
            for (k, v) in kwargs {
                let pk = PyKey::Str(k);
                if let Some(pos) = entries.iter().position(|(ek, _)| ek == &pk) {
                    entries[pos].1 = v;
                } else {
                    entries.push((pk, v));
                }
            }
            let id = vm.alloc(HeapObj::Dict(entries))?;
            Ok(PyVal::Handle(id))
        }
        "min" | "max" => {
            let is_max = b_fn == "max";
            let key_fn = kwargs.get("key").cloned();
            let default_val = kwargs.get("default").cloned();
            let items = if args.len() == 1 {
                vm.to_vec(&args[0])?
            } else if args.len() > 1 {
                args
            } else {
                return Err(vm.runtime_err("TypeError", format!("{b_fn}() expected at least 1 argument")));
            };
            if items.is_empty() {
                return default_val.ok_or_else(|| {
                    vm.runtime_err("ValueError", format!("{b_fn}() arg is an empty sequence"))
                });
            }
            let mut best = items[0].clone();
            let mut best_key = match &key_fn {
                Some(kf) => vm.call_value(kf.clone(), vec![best.clone()], BTreeMap::new(), host.as_deref_mut())?,
                None => best.clone(),
            };
            for item in items.into_iter().skip(1) {
                vm.consume_fuel(1)?;
                let item_key = match &key_fn {
                    Some(kf) => vm.call_value(kf.clone(), vec![item.clone()], BTreeMap::new(), host.as_deref_mut())?,
                    None => item.clone(),
                };
                let ord = vm.val_cmp(&item_key, &best_key)?;
                let better = if is_max {
                    ord == std::cmp::Ordering::Greater
                } else {
                    ord == std::cmp::Ordering::Less
                };
                if better {
                    best = item;
                    best_key = item_key;
                }
            }
            Ok(best)
        }
        "sum" => {
            check_argc(vm, "sum", &args, 1, 2)?;
            let items = vm.to_vec(&args[0])?;
            let mut acc = args.get(1).cloned().unwrap_or(PyVal::Int(0));
            for it in items {
                vm.consume_fuel(1)?;
                acc = vm.eval_binop(crate::ast::BinOp::Add, acc, it, host.as_deref_mut())?;
            }
            Ok(acc)
        }
        "any" => {
            check_argc(vm, "any", &args, 1, 1)?;
            for it in vm.to_vec(&args[0])? {
                vm.consume_fuel(1)?;
                if vm.is_truthy(&it) {
                    return Ok(PyVal::Bool(true));
                }
            }
            Ok(PyVal::Bool(false))
        }
        "all" => {
            check_argc(vm, "all", &args, 1, 1)?;
            for it in vm.to_vec(&args[0])? {
                vm.consume_fuel(1)?;
                if !vm.is_truthy(&it) {
                    return Ok(PyVal::Bool(false));
                }
            }
            Ok(PyVal::Bool(true))
        }
        "enumerate" => {
            check_argc(vm, "enumerate", &args, 1, 2)?;
            let start = if let Some(s) = args.get(1) {
                require_int(vm, s, "enumerate")?
            } else if let Some(s) = kwargs.get("start") {
                require_int(vm, s, "enumerate")?
            } else {
                0
            };
            let items = vm.to_vec(&args[0])?;
            let out: Vec<PyVal> = items
                .into_iter()
                .enumerate()
                .map(|(i, v)| PyVal::Tuple(vec![PyVal::Int(start + i as i64), v]))
                .collect();
            let id = vm.alloc(HeapObj::List(out))?;
            Ok(PyVal::Handle(id))
        }
        "sorted" => {
            check_argc(vm, "sorted", &args, 1, 1)?;
            let items = vm.to_vec(&args[0])?;
            let key_fn = kwargs.get("key").cloned();
            let reverse = kwargs.get("reverse").is_some_and(|v| vm.is_truthy(v));
            let sorted = sort_py_items(vm, items, key_fn, reverse, host)?;
            let id = vm.alloc(HeapObj::List(sorted))?;
            Ok(PyVal::Handle(id))
        }
        "reversed" => {
            check_argc(vm, "reversed", &args, 1, 1)?;
            let mut items = vm.to_vec(&args[0])?;
            items.reverse();
            let id = vm.alloc(HeapObj::List(items))?;
            Ok(PyVal::Handle(id))
        }
        "zip" => {
            let mut vecs = Vec::with_capacity(args.len());
            for a in &args {
                vecs.push(vm.to_vec(a)?);
            }
            let min_len = vecs.iter().map(Vec::len).min().unwrap_or(0);
            let mut out = Vec::with_capacity(min_len);
            for i in 0..min_len {
                vm.consume_fuel(1)?;
                let tup: Vec<PyVal> = vecs.iter().map(|v| v[i].clone()).collect();
                out.push(PyVal::Tuple(tup));
            }
            let id = vm.alloc(HeapObj::List(out))?;
            Ok(PyVal::Handle(id))
        }
        "map" => {
            check_argc(vm, "map", &args, 2, 2)?;
            let f = args[0].clone();
            let items = vm.to_vec(&args[1])?;
            let mut out = Vec::with_capacity(items.len());
            for it in items {
                vm.consume_fuel(1)?;
                out.push(vm.call_value(f.clone(), vec![it], BTreeMap::new(), host.as_deref_mut())?);
            }
            let id = vm.alloc(HeapObj::List(out))?;
            Ok(PyVal::Handle(id))
        }
        "filter" => {
            check_argc(vm, "filter", &args, 2, 2)?;
            let f = args[0].clone();
            let items = vm.to_vec(&args[1])?;
            let mut out = Vec::new();
            for it in items {
                vm.consume_fuel(1)?;
                let keep = if matches!(f, PyVal::None) {
                    vm.is_truthy(&it)
                } else {
                    let r = vm.call_value(
                        f.clone(),
                        vec![it.clone()],
                        BTreeMap::new(),
                        host.as_deref_mut(),
                    )?;
                    vm.is_truthy(&r)
                };
                if keep {
                    out.push(it);
                }
            }
            let id = vm.alloc(HeapObj::List(out))?;
            Ok(PyVal::Handle(id))
        }
        "isinstance" => {
            check_argc(vm, "isinstance", &args, 2, 2)?;
            Ok(PyVal::Bool(check_isinstance(vm, &args[0], &args[1])))
        }
        "type" => {
            check_argc(vm, "type", &args, 1, 1)?;
            Ok(PyVal::Str(vm.type_name(&args[0])))
        }
        "print" => {
            let sep = kwargs.get("sep").and_then(PyVal::as_str).unwrap_or(" ");
            let parts: Vec<String> = args.iter().map(|v| vm.val_to_str(v)).collect();
            let line = parts.join(sep);
            if let Some(h) = host {
                if h.logs.len() < 128 {
                    h.logs.push(line);
                }
            } else if vm.pending_logs.len() < 128 {
                vm.pending_logs.push(line);
            }
            Ok(PyVal::None)
        }
        "chr" => {
            check_argc(vm, "chr", &args, 1, 1)?;
            let code = require_int(vm, &args[0], "chr")?;
            let ch = u32::try_from(code)
                .ok()
                .and_then(char::from_u32)
                .ok_or_else(|| vm.runtime_err("ValueError", "chr() arg not in range"))?;
            Ok(PyVal::Str(ch.to_string()))
        }
        "ord" => {
            check_argc(vm, "ord", &args, 1, 1)?;
            let s = args[0]
                .as_str()
                .ok_or_else(|| vm.runtime_err("TypeError", "ord() expected string of length 1"))?;
            let mut chars = s.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Ok(PyVal::Int(c as i64)),
                _ => Err(vm.runtime_err("TypeError", "ord() expected a character")),
            }
        }
        "hex" => {
            check_argc(vm, "hex", &args, 1, 1)?;
            let n = require_int(vm, &args[0], "hex")?;
            if n < 0 {
                Ok(PyVal::Str(format!("-0x{:x}", n.unsigned_abs())))
            } else {
                Ok(PyVal::Str(format!("0x{n:x}")))
            }
        }
        "oct" => {
            check_argc(vm, "oct", &args, 1, 1)?;
            let n = require_int(vm, &args[0], "oct")?;
            if n < 0 {
                Ok(PyVal::Str(format!("-0o{:o}", n.unsigned_abs())))
            } else {
                Ok(PyVal::Str(format!("0o{n:o}")))
            }
        }
        "bin" => {
            check_argc(vm, "bin", &args, 1, 1)?;
            let n = require_int(vm, &args[0], "bin")?;
            if n < 0 {
                Ok(PyVal::Str(format!("-0b{:b}", n.unsigned_abs())))
            } else {
                Ok(PyVal::Str(format!("0b{n:b}")))
            }
        }
        "pow" => {
            check_argc(vm, "pow", &args, 2, 3)?;
            let base = vm.eval_binop(crate::ast::BinOp::Pow, args[0].clone(), args[1].clone(), None)?;
            if let Some(m) = args.get(2) {
                vm.eval_binop(crate::ast::BinOp::Mod, base, m.clone(), None)
            } else {
                Ok(base)
            }
        }
        "divmod" => {
            check_argc(vm, "divmod", &args, 2, 2)?;
            let q = vm.eval_binop(crate::ast::BinOp::FloorDiv, args[0].clone(), args[1].clone(), None)?;
            let r = vm.eval_binop(crate::ast::BinOp::Mod, args[0].clone(), args[1].clone(), None)?;
            Ok(PyVal::Tuple(vec![q, r]))
        }
        "hasattr" => {
            check_argc(vm, "hasattr", &args, 2, 2)?;
            let attr = args[1]
                .as_str()
                .ok_or_else(|| vm.runtime_err("TypeError", "attribute name must be string"))?;
            Ok(PyVal::Bool(vm.get_attr(&args[0], attr, host).is_ok()))
        }
        "getattr" => {
            check_argc(vm, "getattr", &args, 2, 3)?;
            let attr = args[1]
                .as_str()
                .ok_or_else(|| vm.runtime_err("TypeError", "attribute name must be string"))?;
            match vm.get_attr(&args[0], attr, host) {
                Ok(v) => Ok(v),
                Err(e) => {
                    if let Some(def) = args.get(2) {
                        Ok(def.clone())
                    } else {
                        Err(e)
                    }
                }
            }
        }
        "setattr" => {
            check_argc(vm, "setattr", &args, 3, 3)?;
            let attr = args[1]
                .as_str()
                .ok_or_else(|| vm.runtime_err("TypeError", "attribute name must be string"))?;
            vm.set_attr(&args[0], attr, args[2].clone(), host)?;
            Ok(PyVal::None)
        }
        "callable" => {
            check_argc(vm, "callable", &args, 1, 1)?;
            let is_c = match &args[0] {
                PyVal::Handle(id) => matches!(
                    vm.heap.get(*id),
                    Some(
                        HeapObj::Function(_)
                            | HeapObj::BoundMethod { .. }
                            | HeapObj::NativeFn(_)
                            | HeapObj::BoundNativeMethod { .. }
                            | HeapObj::Class(_)
                            | HeapObj::Native(NativeObjKind::Decorator(_))
                            | HeapObj::Native(NativeObjKind::ExceptionType(_))
                    )
                ),
                _ => false,
            };
            Ok(PyVal::Bool(is_c))
        }
        _ => Err(vm.runtime_err("RuntimeError", format!("unknown builtin '{name}'"))),
    }
}

pub fn call_bound_native_method(
    vm: &mut PyVm,
    receiver: PyVal,
    method: &str,
    args: Vec<PyVal>,
    kwargs: BTreeMap<String, PyVal>,
    host: Option<&mut HostBridge<'_>>,
) -> Result<PyVal, PyError> {
    if let Some(m) = method.strip_prefix("str.") {
        return call_str_method(vm, receiver, m, args, kwargs);
    }
    if let Some(m) = method.strip_prefix("list.") {
        return call_list_method(vm, receiver, m, args, kwargs, host);
    }
    if let Some(m) = method.strip_prefix("dict.") {
        return call_dict_method(vm, receiver, m, args, kwargs);
    }
    if let Some(m) = method.strip_prefix("set.") {
        return call_set_method(vm, receiver, m, args);
    }
    if let Some(m) = method.strip_prefix("tuple.") {
        return call_tuple_method(vm, receiver, m, args);
    }
    bindings::call_native_method(vm, receiver, method, args, kwargs, host)
}

fn call_str_method(
    vm: &mut PyVm,
    receiver: PyVal,
    method: &str,
    args: Vec<PyVal>,
    kwargs: BTreeMap<String, PyVal>,
) -> Result<PyVal, PyError> {
    let PyVal::Str(s) = receiver else {
        return Err(vm.runtime_err("TypeError", "expected str"));
    };
    match method {
        "lower" => Ok(PyVal::Str(s.to_lowercase())),
        "upper" => Ok(PyVal::Str(s.to_uppercase())),
        "title" => {
            let mut out = String::with_capacity(s.len());
            let mut cap_next = true;
            for c in s.chars() {
                if c.is_alphabetic() {
                    if cap_next {
                        out.extend(c.to_uppercase());
                        cap_next = false;
                    } else {
                        out.extend(c.to_lowercase());
                    }
                } else {
                    out.push(c);
                    cap_next = true;
                }
            }
            Ok(PyVal::Str(out))
        }
        "capitalize" => {
            let mut chars = s.chars();
            let out = match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str().to_lowercase()),
                None => String::new(),
            };
            Ok(PyVal::Str(out))
        }
        "strip" => match args.first().and_then(PyVal::as_str) {
            Some(chars) => Ok(PyVal::Str(
                s.trim_matches(|c| chars.contains(c)).to_string(),
            )),
            None => Ok(PyVal::Str(s.trim().to_string())),
        },
        "lstrip" => match args.first().and_then(PyVal::as_str) {
            Some(chars) => Ok(PyVal::Str(
                s.trim_start_matches(|c| chars.contains(c)).to_string(),
            )),
            None => Ok(PyVal::Str(s.trim_start().to_string())),
        },
        "rstrip" => match args.first().and_then(PyVal::as_str) {
            Some(chars) => Ok(PyVal::Str(
                s.trim_end_matches(|c| chars.contains(c)).to_string(),
            )),
            None => Ok(PyVal::Str(s.trim_end().to_string())),
        },
        "split" => {
            let sep = args.first().and_then(|v| match v {
                PyVal::None => None,
                PyVal::Str(st) => Some(st.as_str()),
                _ => None,
            });
            let maxsplit = args
                .get(1)
                .or_else(|| kwargs.get("maxsplit"))
                .and_then(PyVal::as_int)
                .unwrap_or(-1);
            let parts: Vec<PyVal> = match sep {
                None => {
                    if maxsplit < 0 {
                        s.split_whitespace()
                            .map(|p| PyVal::Str(p.to_string()))
                            .collect()
                    } else {
                        let mut out = Vec::new();
                        let mut rest = s.trim_start();
                        for _ in 0..maxsplit {
                            if rest.is_empty() {
                                break;
                            }
                            match rest.find(char::is_whitespace) {
                                Some(idx) => {
                                    out.push(PyVal::Str(rest[..idx].to_string()));
                                    rest = rest[idx..].trim_start();
                                }
                                None => break,
                            }
                        }
                        if !rest.is_empty() {
                            out.push(PyVal::Str(rest.to_string()));
                        }
                        out
                    }
                }
                Some("") => return Err(vm.runtime_err("ValueError", "empty separator")),
                Some(sep_str) => {
                    if maxsplit < 0 {
                        s.split(sep_str).map(|p| PyVal::Str(p.to_string())).collect()
                    } else {
                        s.splitn((maxsplit as usize).saturating_add(1), sep_str)
                            .map(|p| PyVal::Str(p.to_string()))
                            .collect()
                    }
                }
            };
            let id = vm.alloc(HeapObj::List(parts))?;
            Ok(PyVal::Handle(id))
        }
        "rsplit" => {
            let sep = args.first().and_then(|v| match v {
                PyVal::None => None,
                PyVal::Str(st) => Some(st.as_str()),
                _ => None,
            });
            let maxsplit = args.get(1).and_then(PyVal::as_int).unwrap_or(-1);
            let mut parts: Vec<PyVal> = match sep {
                None => s
                    .split_whitespace()
                    .map(|p| PyVal::Str(p.to_string()))
                    .collect(),
                Some("") => return Err(vm.runtime_err("ValueError", "empty separator")),
                Some(sep_str) => {
                    if maxsplit < 0 {
                        s.split(sep_str).map(|p| PyVal::Str(p.to_string())).collect()
                    } else {
                        let mut v: Vec<PyVal> = s
                            .rsplitn((maxsplit as usize).saturating_add(1), sep_str)
                            .map(|p| PyVal::Str(p.to_string()))
                            .collect();
                        v.reverse();
                        v
                    }
                }
            };
            if sep.is_none() && maxsplit >= 0 && parts.len() > (maxsplit as usize + 1) {
                let keep = maxsplit as usize;
                let head_count = parts.len() - keep;
                let head: Vec<String> = parts
                    .drain(..head_count)
                    .map(|v| vm.val_to_str(&v))
                    .collect();
                parts.insert(0, PyVal::Str(head.join(" ")));
            }
            let id = vm.alloc(HeapObj::List(parts))?;
            Ok(PyVal::Handle(id))
        }
        "splitlines" => {
            let parts: Vec<PyVal> = s.lines().map(|l| PyVal::Str(l.to_string())).collect();
            let id = vm.alloc(HeapObj::List(parts))?;
            Ok(PyVal::Handle(id))
        }
        "join" => {
            check_argc(vm, "str.join", &args, 1, 1)?;
            let items = vm.to_vec(&args[0])?;
            let mut out = String::new();
            for (i, item) in items.iter().enumerate() {
                let piece = item
                    .as_str()
                    .ok_or_else(|| vm.runtime_err("TypeError", "sequence item expected str"))?;
                if i > 0 {
                    out.push_str(&s);
                }
                if out.len() + piece.len() > MAX_STR_LEN {
                    return Err(vm.runtime_err("MemoryError", "joined string exceeds limit"));
                }
                out.push_str(piece);
            }
            Ok(PyVal::Str(out))
        }
        "replace" => {
            check_argc(vm, "str.replace", &args, 2, 3)?;
            let old = require_str(vm, &args[0], "replace")?;
            let new = require_str(vm, &args[1], "replace")?;
            let res = if let Some(cnt) = args.get(2) {
                let n = require_int(vm, cnt, "replace")?;
                if n < 0 {
                    s.replace(old, new)
                } else {
                    s.replacen(old, new, n as usize)
                }
            } else {
                s.replace(old, new)
            };
            Ok(PyVal::Str(res))
        }
        "startswith" => {
            check_argc(vm, "str.startswith", &args, 1, 1)?;
            Ok(PyVal::Bool(match &args[0] {
                PyVal::Str(p) => s.starts_with(p.as_str()),
                PyVal::Tuple(ts) => ts.iter().any(|t| t.as_str().is_some_and(|p| s.starts_with(p))),
                _ => return Err(vm.runtime_err("TypeError", "startswith expects str or tuple of str")),
            }))
        }
        "endswith" => {
            check_argc(vm, "str.endswith", &args, 1, 1)?;
            Ok(PyVal::Bool(match &args[0] {
                PyVal::Str(p) => s.ends_with(p.as_str()),
                PyVal::Tuple(ts) => ts.iter().any(|t| t.as_str().is_some_and(|p| s.ends_with(p))),
                _ => return Err(vm.runtime_err("TypeError", "endswith expects str or tuple of str")),
            }))
        }
        "find" => {
            check_argc(vm, "str.find", &args, 1, 3)?;
            let sub = require_str(vm, &args[0], "find")?;
            let start = args.get(1).and_then(PyVal::as_int).unwrap_or(0).max(0) as usize;
            let char_idx = s
                .char_indices()
                .nth(start)
                .map(|(b, _)| b)
                .unwrap_or(s.len());
            match s[char_idx..].find(sub) {
                Some(b_off) => {
                    let c_off = s[char_idx..char_idx + b_off].chars().count();
                    Ok(PyVal::Int((start + c_off) as i64))
                }
                None => Ok(PyVal::Int(-1)),
            }
        }
        "rfind" => {
            check_argc(vm, "str.rfind", &args, 1, 1)?;
            let sub = require_str(vm, &args[0], "rfind")?;
            match s.rfind(sub) {
                Some(b_off) => Ok(PyVal::Int(s[..b_off].chars().count() as i64)),
                None => Ok(PyVal::Int(-1)),
            }
        }
        "index" => {
            check_argc(vm, "str.index", &args, 1, 1)?;
            let sub = require_str(vm, &args[0], "index")?;
            match s.find(sub) {
                Some(b_off) => Ok(PyVal::Int(s[..b_off].chars().count() as i64)),
                None => Err(vm.runtime_err("ValueError", "substring not found")),
            }
        }
        "count" => {
            check_argc(vm, "str.count", &args, 1, 1)?;
            let sub = require_str(vm, &args[0], "count")?;
            Ok(PyVal::Int(s.matches(sub).count() as i64))
        }
        "format" => {
            let mut out = s;
            for (i, a) in args.iter().enumerate() {
                let text = vm.val_to_str(a);
                let indexed = format!("{{{i}}}");
                if out.contains(&indexed) {
                    out = out.replace(&indexed, &text);
                } else if let Some(pos) = out.find("{}") {
                    out.replace_range(pos..pos + 2, &text);
                }
            }
            for (k, v) in kwargs {
                let text = vm.val_to_str(&v);
                out = out.replace(&format!("{{{k}}}"), &text);
            }
            Ok(PyVal::Str(out))
        }
        "isdigit" => Ok(PyVal::Bool(!s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))),
        "isalpha" => Ok(PyVal::Bool(!s.is_empty() && s.chars().all(char::is_alphabetic))),
        "isalnum" => Ok(PyVal::Bool(!s.is_empty() && s.chars().all(char::is_alphanumeric))),
        "isspace" => Ok(PyVal::Bool(!s.is_empty() && s.chars().all(char::is_whitespace))),
        "isupper" => Ok(PyVal::Bool(
            s.chars().any(char::is_uppercase) && !s.chars().any(char::is_lowercase),
        )),
        "islower" => Ok(PyVal::Bool(
            s.chars().any(char::is_lowercase) && !s.chars().any(char::is_uppercase),
        )),
        "zfill" => {
            check_argc(vm, "str.zfill", &args, 1, 1)?;
            let width = require_int(vm, &args[0], "zfill")?.max(0) as usize;
            let len = s.chars().count();
            if len >= width {
                Ok(PyVal::Str(s))
            } else {
                let pad = "0".repeat(width - len);
                if let Some(rest) = s.strip_prefix('-') {
                    Ok(PyVal::Str(format!("-{pad}{rest}")))
                } else if let Some(rest) = s.strip_prefix('+') {
                    Ok(PyVal::Str(format!("+{pad}{rest}")))
                } else {
                    Ok(PyVal::Str(format!("{pad}{s}")))
                }
            }
        }
        "ljust" | "rjust" | "center" => {
            check_argc(vm, method, &args, 1, 2)?;
            let width = require_int(vm, &args[0], method)?.max(0) as usize;
            let fill = args
                .get(1)
                .and_then(PyVal::as_str)
                .and_then(|f| f.chars().next())
                .unwrap_or(' ');
            let len = s.chars().count();
            if len >= width {
                return Ok(PyVal::Str(s));
            }
            let diff = width - len;
            let out = match method {
                "ljust" => format!("{s}{}", fill.to_string().repeat(diff)),
                "rjust" => format!("{}{s}", fill.to_string().repeat(diff)),
                _ => {
                    let left = diff / 2;
                    let right = diff - left;
                    format!(
                        "{}{s}{}",
                        fill.to_string().repeat(left),
                        fill.to_string().repeat(right)
                    )
                }
            };
            Ok(PyVal::Str(out))
        }
        "removeprefix" => {
            check_argc(vm, "str.removeprefix", &args, 1, 1)?;
            let p = require_str(vm, &args[0], "removeprefix")?;
            Ok(PyVal::Str(s.strip_prefix(p).unwrap_or(&s).to_string()))
        }
        "removesuffix" => {
            check_argc(vm, "str.removesuffix", &args, 1, 1)?;
            let p = require_str(vm, &args[0], "removesuffix")?;
            Ok(PyVal::Str(s.strip_suffix(p).unwrap_or(&s).to_string()))
        }
        _ => Err(vm.runtime_err(
            "AttributeError",
            format!("'str' object has no attribute '{method}'"),
        )),
    }
}

fn call_list_method(
    vm: &mut PyVm,
    receiver: PyVal,
    method: &str,
    args: Vec<PyVal>,
    kwargs: BTreeMap<String, PyVal>,
    host: Option<&mut HostBridge<'_>>,
) -> Result<PyVal, PyError> {
    let PyVal::Handle(id) = receiver else {
        return Err(vm.runtime_err("TypeError", "expected list"));
    };
    match method {
        "append" => {
            check_argc(vm, "list.append", &args, 1, 1)?;
            if let Some(HeapObj::List(items)) = vm.heap.get_mut(id) {
                if items.len() >= MAX_SEQ_LEN {
                    return Err(vm.runtime_err("MemoryError", "list length limit exceeded"));
                }
                items.push(args[0].clone());
            }
            Ok(PyVal::None)
        }
        "extend" => {
            check_argc(vm, "list.extend", &args, 1, 1)?;
            let extra = vm.to_vec(&args[0])?;
            if let Some(HeapObj::List(items)) = vm.heap.get_mut(id) {
                if items.len() + extra.len() > MAX_SEQ_LEN {
                    return Err(vm.runtime_err("MemoryError", "list length limit exceeded"));
                }
                items.extend(extra);
            }
            Ok(PyVal::None)
        }
        "insert" => {
            check_argc(vm, "list.insert", &args, 2, 2)?;
            let idx = require_int(vm, &args[0], "insert")?;
            if let Some(HeapObj::List(items)) = vm.heap.get_mut(id) {
                let len = items.len() as i64;
                let pos = if idx < 0 {
                    (len + idx).max(0) as usize
                } else {
                    (idx as usize).min(items.len())
                };
                items.insert(pos, args[1].clone());
            }
            Ok(PyVal::None)
        }
        "pop" => {
            check_argc(vm, "list.pop", &args, 0, 1)?;
            let idx = args.first().and_then(PyVal::as_int).unwrap_or(-1);
            let frames = &vm.frames;
            let popped = if let Some(HeapObj::List(items)) = vm.heap.get_mut(id) {
                if items.is_empty() {
                    return Err(PyError::runtime(
                        "IndexError",
                        "pop from empty list",
                        frames.clone(),
                    ));
                }
                let pos = normalize_index(idx, items.len()).ok_or_else(|| {
                    PyError::runtime("IndexError", "pop index out of range", frames.clone())
                })?;
                items.remove(pos)
            } else {
                PyVal::None
            };
            Ok(popped)
        }
        "remove" => {
            check_argc(vm, "list.remove", &args, 1, 1)?;
            let items_clone = match vm.heap.get(id) {
                Some(HeapObj::List(it)) => it.clone(),
                _ => return Ok(PyVal::None),
            };
            let pos = items_clone
                .iter()
                .position(|x| vm.vals_equal(x, &args[0]))
                .ok_or_else(|| vm.runtime_err("ValueError", "list.remove(x): x not in list"))?;
            if let Some(HeapObj::List(items)) = vm.heap.get_mut(id) {
                items.remove(pos);
            }
            Ok(PyVal::None)
        }
        "clear" => {
            if let Some(HeapObj::List(items)) = vm.heap.get_mut(id) {
                items.clear();
            }
            Ok(PyVal::None)
        }
        "index" => {
            check_argc(vm, "list.index", &args, 1, 1)?;
            let items = match vm.heap.get(id) {
                Some(HeapObj::List(it)) => it.clone(),
                _ => return Err(vm.runtime_err("ValueError", "not in list")),
            };
            let pos = items
                .iter()
                .position(|x| vm.vals_equal(x, &args[0]))
                .ok_or_else(|| vm.runtime_err("ValueError", "value is not in list"))?;
            Ok(PyVal::Int(pos as i64))
        }
        "count" => {
            check_argc(vm, "list.count", &args, 1, 1)?;
            let items = match vm.heap.get(id) {
                Some(HeapObj::List(it)) => it.clone(),
                _ => return Ok(PyVal::Int(0)),
            };
            let c = items.iter().filter(|x| vm.vals_equal(x, &args[0])).count();
            Ok(PyVal::Int(c as i64))
        }
        "reverse" => {
            if let Some(HeapObj::List(items)) = vm.heap.get_mut(id) {
                items.reverse();
            }
            Ok(PyVal::None)
        }
        "copy" => {
            let items = match vm.heap.get(id) {
                Some(HeapObj::List(it)) => it.clone(),
                _ => Vec::new(),
            };
            let nid = vm.alloc(HeapObj::List(items))?;
            Ok(PyVal::Handle(nid))
        }
        "sort" => {
            let items = match vm.heap.get(id) {
                Some(HeapObj::List(it)) => it.clone(),
                _ => return Ok(PyVal::None),
            };
            let key_fn = kwargs.get("key").cloned();
            let reverse = kwargs.get("reverse").is_some_and(|v| vm.is_truthy(v));
            let sorted = sort_py_items(vm, items, key_fn, reverse, host)?;
            if let Some(HeapObj::List(slot)) = vm.heap.get_mut(id) {
                *slot = sorted;
            }
            Ok(PyVal::None)
        }
        _ => Err(vm.runtime_err("AttributeError", format!("'list' has no attribute '{method}'"))),
    }
}

fn call_dict_method(
    vm: &mut PyVm,
    receiver: PyVal,
    method: &str,
    args: Vec<PyVal>,
    kwargs: BTreeMap<String, PyVal>,
) -> Result<PyVal, PyError> {
    let PyVal::Handle(id) = receiver else {
        return Err(vm.runtime_err("TypeError", "expected dict"));
    };
    match method {
        "get" => {
            check_argc(vm, "dict.get", &args, 1, 2)?;
            let default = args.get(1).cloned().unwrap_or(PyVal::None);
            let Some(k) = PyKey::from_val(&args[0]) else {
                return Ok(default);
            };
            if let Some(HeapObj::Dict(entries)) = vm.heap.get(id)
                && let Some((_, v)) = entries.iter().find(|(ek, _)| ek == &k)
            {
                return Ok(v.clone());
            }
            Ok(default)
        }
        "keys" => {
            let ks: Vec<PyVal> = match vm.heap.get(id) {
                Some(HeapObj::Dict(entries)) => entries.iter().map(|(k, _)| k.to_val()).collect(),
                _ => Vec::new(),
            };
            let nid = vm.alloc(HeapObj::List(ks))?;
            Ok(PyVal::Handle(nid))
        }
        "values" => {
            let vs: Vec<PyVal> = match vm.heap.get(id) {
                Some(HeapObj::Dict(entries)) => entries.iter().map(|(_, v)| v.clone()).collect(),
                _ => Vec::new(),
            };
            let nid = vm.alloc(HeapObj::List(vs))?;
            Ok(PyVal::Handle(nid))
        }
        "items" => {
            let pairs: Vec<PyVal> = match vm.heap.get(id) {
                Some(HeapObj::Dict(entries)) => entries
                    .iter()
                    .map(|(k, v)| PyVal::Tuple(vec![k.to_val(), v.clone()]))
                    .collect(),
                _ => Vec::new(),
            };
            let nid = vm.alloc(HeapObj::List(pairs))?;
            Ok(PyVal::Handle(nid))
        }
        "pop" => {
            check_argc(vm, "dict.pop", &args, 1, 2)?;
            let k = PyKey::from_val(&args[0])
                .ok_or_else(|| vm.runtime_err("TypeError", "unhashable dict key"))?;
            if let Some(HeapObj::Dict(entries)) = vm.heap.get_mut(id)
                && let Some(pos) = entries.iter().position(|(ek, _)| ek == &k)
            {
                return Ok(entries.remove(pos).1);
            }
            args.get(1)
                .cloned()
                .ok_or_else(|| vm.runtime_err("KeyError", "key not found"))
        }
        "setdefault" => {
            check_argc(vm, "dict.setdefault", &args, 1, 2)?;
            let default = args.get(1).cloned().unwrap_or(PyVal::None);
            let k = PyKey::from_val(&args[0])
                .ok_or_else(|| vm.runtime_err("TypeError", "unhashable dict key"))?;
            if let Some(HeapObj::Dict(entries)) = vm.heap.get_mut(id) {
                if let Some((_, v)) = entries.iter().find(|(ek, _)| ek == &k) {
                    return Ok(v.clone());
                }
                entries.push((k, default.clone()));
            }
            Ok(default)
        }
        "update" => {
            let mut incoming: Vec<(PyKey, PyVal)> = Vec::new();
            if let Some(arg) = args.first() {
                if let PyVal::Handle(oid) = arg
                    && let Some(HeapObj::Dict(other)) = vm.heap.get(*oid)
                {
                    incoming.extend(other.clone());
                } else {
                    for pair in vm.to_vec(arg)? {
                        let kv = vm.to_vec(&pair)?;
                        if kv.len() == 2
                            && let Some(k) = PyKey::from_val(&kv[0])
                        {
                            incoming.push((k, kv[1].clone()));
                        }
                    }
                }
            }
            for (k, v) in kwargs {
                incoming.push((PyKey::Str(k), v));
            }
            if let Some(HeapObj::Dict(entries)) = vm.heap.get_mut(id) {
                for (k, v) in incoming {
                    if let Some(pos) = entries.iter().position(|(ek, _)| ek == &k) {
                        entries[pos].1 = v;
                    } else {
                        entries.push((k, v));
                    }
                }
            }
            Ok(PyVal::None)
        }
        "clear" => {
            if let Some(HeapObj::Dict(entries)) = vm.heap.get_mut(id) {
                entries.clear();
            }
            Ok(PyVal::None)
        }
        "copy" => {
            let entries = match vm.heap.get(id) {
                Some(HeapObj::Dict(e)) => e.clone(),
                _ => Vec::new(),
            };
            let nid = vm.alloc(HeapObj::Dict(entries))?;
            Ok(PyVal::Handle(nid))
        }
        _ => Err(vm.runtime_err("AttributeError", format!("'dict' has no attribute '{method}'"))),
    }
}

fn call_set_method(
    vm: &mut PyVm,
    receiver: PyVal,
    method: &str,
    args: Vec<PyVal>,
) -> Result<PyVal, PyError> {
    let PyVal::Handle(id) = receiver else {
        return Err(vm.runtime_err("TypeError", "expected set"));
    };
    match method {
        "add" => {
            check_argc(vm, "set.add", &args, 1, 1)?;
            let k = PyKey::from_val(&args[0])
                .ok_or_else(|| vm.runtime_err("TypeError", "unhashable type"))?;
            if let Some(HeapObj::Set(items)) = vm.heap.get_mut(id)
                && !items.contains(&k)
            {
                items.push(k);
            }
            Ok(PyVal::None)
        }
        "discard" => {
            check_argc(vm, "set.discard", &args, 1, 1)?;
            if let Some(k) = PyKey::from_val(&args[0])
                && let Some(HeapObj::Set(items)) = vm.heap.get_mut(id)
            {
                items.retain(|x| x != &k);
            }
            Ok(PyVal::None)
        }
        "remove" => {
            check_argc(vm, "set.remove", &args, 1, 1)?;
            let k = PyKey::from_val(&args[0])
                .ok_or_else(|| vm.runtime_err("KeyError", "element not in set"))?;
            if let Some(HeapObj::Set(items)) = vm.heap.get_mut(id)
                && let Some(pos) = items.iter().position(|x| x == &k)
            {
                items.remove(pos);
                return Ok(PyVal::None);
            }
            Err(vm.runtime_err("KeyError", "element not in set"))
        }
        "pop" => {
            if let Some(HeapObj::Set(items)) = vm.heap.get_mut(id)
                && let Some(k) = items.pop()
            {
                return Ok(k.to_val());
            }
            Err(vm.runtime_err("KeyError", "pop from an empty set"))
        }
        "clear" => {
            if let Some(HeapObj::Set(items)) = vm.heap.get_mut(id) {
                items.clear();
            }
            Ok(PyVal::None)
        }
        "copy" => {
            let items = match vm.heap.get(id) {
                Some(HeapObj::Set(s)) => s.clone(),
                _ => Vec::new(),
            };
            let nid = vm.alloc(HeapObj::Set(items))?;
            Ok(PyVal::Handle(nid))
        }
        "update" => {
            check_argc(vm, "set.update", &args, 1, 1)?;
            let extra = vm.to_vec(&args[0])?;
            let mut keys = Vec::new();
            for v in extra {
                let k = PyKey::from_val(&v)
                    .ok_or_else(|| vm.runtime_err("TypeError", "unhashable type"))?;
                keys.push(k);
            }
            if let Some(HeapObj::Set(items)) = vm.heap.get_mut(id) {
                for k in keys {
                    if !items.contains(&k) {
                        items.push(k);
                    }
                }
            }
            Ok(PyVal::None)
        }
        "union" | "intersection" | "difference" | "issubset" | "issuperset" => {
            check_argc(vm, method, &args, 1, 1)?;
            let a = match vm.heap.get(id) {
                Some(HeapObj::Set(s)) => s.clone(),
                _ => Vec::new(),
            };
            let mut b = Vec::new();
            for v in vm.to_vec(&args[0])? {
                if let Some(k) = PyKey::from_val(&v) {
                    b.push(k);
                }
            }
            match method {
                "union" => {
                    let mut out = a;
                    for k in b {
                        if !out.contains(&k) {
                            out.push(k);
                        }
                    }
                    let nid = vm.alloc(HeapObj::Set(out))?;
                    Ok(PyVal::Handle(nid))
                }
                "intersection" => {
                    let out: Vec<PyKey> = a.into_iter().filter(|k| b.contains(k)).collect();
                    let nid = vm.alloc(HeapObj::Set(out))?;
                    Ok(PyVal::Handle(nid))
                }
                "difference" => {
                    let out: Vec<PyKey> = a.into_iter().filter(|k| !b.contains(k)).collect();
                    let nid = vm.alloc(HeapObj::Set(out))?;
                    Ok(PyVal::Handle(nid))
                }
                "issubset" => Ok(PyVal::Bool(a.iter().all(|k| b.contains(k)))),
                _ => Ok(PyVal::Bool(b.iter().all(|k| a.contains(k)))),
            }
        }
        _ => Err(vm.runtime_err("AttributeError", format!("'set' has no attribute '{method}'"))),
    }
}

fn call_tuple_method(
    vm: &mut PyVm,
    receiver: PyVal,
    method: &str,
    args: Vec<PyVal>,
) -> Result<PyVal, PyError> {
    let PyVal::Tuple(items) = receiver else {
        return Err(vm.runtime_err("TypeError", "expected tuple"));
    };
    match method {
        "count" => {
            check_argc(vm, "tuple.count", &args, 1, 1)?;
            let c = items.iter().filter(|x| vm.vals_equal(x, &args[0])).count();
            Ok(PyVal::Int(c as i64))
        }
        "index" => {
            check_argc(vm, "tuple.index", &args, 1, 1)?;
            let pos = items
                .iter()
                .position(|x| vm.vals_equal(x, &args[0]))
                .ok_or_else(|| vm.runtime_err("ValueError", "tuple.index(x): x not in tuple"))?;
            Ok(PyVal::Int(pos as i64))
        }
        _ => Err(vm.runtime_err("AttributeError", format!("'tuple' has no attribute '{method}'"))),
    }
}

fn call_math_fn(vm: &mut PyVm, name: &str, args: Vec<PyVal>) -> Result<PyVal, PyError> {
    let f1 = |vm: &PyVm, a: &[PyVal]| -> Result<f64, PyError> {
        check_argc(vm, name, a, 1, 1)?;
        a[0].as_float()
            .ok_or_else(|| vm.runtime_err("TypeError", format!("math.{name}() expects a number")))
    };
    let f2 = |vm: &PyVm, a: &[PyVal]| -> Result<(f64, f64), PyError> {
        check_argc(vm, name, a, 2, 2)?;
        let x = a[0]
            .as_float()
            .ok_or_else(|| vm.runtime_err("TypeError", format!("math.{name}() expects numbers")))?;
        let y = a[1]
            .as_float()
            .ok_or_else(|| vm.runtime_err("TypeError", format!("math.{name}() expects numbers")))?;
        Ok((x, y))
    };
    match name {
        "sqrt" => {
            let x = f1(vm, &args)?;
            if x < 0.0 {
                return Err(vm.runtime_err("ValueError", "math domain error"));
            }
            Ok(PyVal::Float(x.sqrt()))
        }
        "sin" => Ok(PyVal::Float(f1(vm, &args)?.sin())),
        "cos" => Ok(PyVal::Float(f1(vm, &args)?.cos())),
        "tan" => Ok(PyVal::Float(f1(vm, &args)?.tan())),
        "asin" => Ok(PyVal::Float(f1(vm, &args)?.asin())),
        "acos" => Ok(PyVal::Float(f1(vm, &args)?.acos())),
        "atan" => Ok(PyVal::Float(f1(vm, &args)?.atan())),
        "atan2" => {
            let (y, x) = f2(vm, &args)?;
            Ok(PyVal::Float(y.atan2(x)))
        }
        "floor" => Ok(PyVal::Int(f1(vm, &args)?.floor() as i64)),
        "ceil" => Ok(PyVal::Int(f1(vm, &args)?.ceil() as i64)),
        "trunc" => Ok(PyVal::Int(f1(vm, &args)?.trunc() as i64)),
        "fabs" => Ok(PyVal::Float(f1(vm, &args)?.abs())),
        "pow" => {
            let (x, y) = f2(vm, &args)?;
            Ok(PyVal::Float(x.powf(y)))
        }
        "log" => {
            check_argc(vm, "log", &args, 1, 2)?;
            let x = args[0]
                .as_float()
                .ok_or_else(|| vm.runtime_err("TypeError", "math.log() expects a number"))?;
            if x <= 0.0 {
                return Err(vm.runtime_err("ValueError", "math domain error"));
            }
            if let Some(b) = args.get(1) {
                let bf = b
                    .as_float()
                    .ok_or_else(|| vm.runtime_err("TypeError", "math.log() base must be a number"))?;
                Ok(PyVal::Float(x.log(bf)))
            } else {
                Ok(PyVal::Float(x.ln()))
            }
        }
        "log2" => Ok(PyVal::Float(f1(vm, &args)?.log2())),
        "log10" => Ok(PyVal::Float(f1(vm, &args)?.log10())),
        "exp" => Ok(PyVal::Float(f1(vm, &args)?.exp())),
        "hypot" => {
            let mut sum = 0.0f64;
            for a in &args {
                let v = a
                    .as_float()
                    .ok_or_else(|| vm.runtime_err("TypeError", "math.hypot() expects numbers"))?;
                sum += v * v;
            }
            Ok(PyVal::Float(sum.sqrt()))
        }
        "dist" => {
            check_argc(vm, "dist", &args, 2, 2)?;
            let p = vm.to_vec(&args[0])?;
            let q = vm.to_vec(&args[1])?;
            if p.len() != q.len() {
                return Err(vm.runtime_err("ValueError", "both points must have the same number of dimensions"));
            }
            let mut sum = 0.0f64;
            for (a, b) in p.iter().zip(q.iter()) {
                let af = a.as_float().ok_or_else(|| vm.runtime_err("TypeError", "point coordinates must be numeric"))?;
                let bf = b.as_float().ok_or_else(|| vm.runtime_err("TypeError", "point coordinates must be numeric"))?;
                let d = af - bf;
                sum += d * d;
            }
            Ok(PyVal::Float(sum.sqrt()))
        }
        "degrees" => Ok(PyVal::Float(f1(vm, &args)?.to_degrees())),
        "radians" => Ok(PyVal::Float(f1(vm, &args)?.to_radians())),
        "isnan" => Ok(PyVal::Bool(f1(vm, &args)?.is_nan())),
        "isinf" => Ok(PyVal::Bool(f1(vm, &args)?.is_infinite())),
        "isfinite" => Ok(PyVal::Bool(f1(vm, &args)?.is_finite())),
        "gcd" => {
            check_argc(vm, "gcd", &args, 2, 2)?;
            let mut a = require_int(vm, &args[0], "gcd")?.unsigned_abs();
            let mut b = require_int(vm, &args[1], "gcd")?.unsigned_abs();
            while b != 0 {
                let t = a % b;
                a = b;
                b = t;
            }
            Ok(PyVal::Int(a as i64))
        }
        "fmod" => {
            let (x, y) = f2(vm, &args)?;
            if y == 0.0 {
                return Err(vm.runtime_err("ValueError", "math domain error"));
            }
            Ok(PyVal::Float(x % y))
        }
        "copysign" => {
            let (x, y) = f2(vm, &args)?;
            Ok(PyVal::Float(x.copysign(y)))
        }
        _ => Err(vm.runtime_err("AttributeError", format!("module 'math' has no attribute '{name}'"))),
    }
}

fn call_random_fn(vm: &mut PyVm, name: &str, args: Vec<PyVal>) -> Result<PyVal, PyError> {
    match name {
        "seed" => {
            if let Some(v) = args.first() {
                let s = match v {
                    PyVal::Int(n) => *n as u64,
                    PyVal::Str(st) => {
                        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
                        for b in st.as_bytes() {
                            h ^= u64::from(*b);
                            h = h.wrapping_mul(0x100_0000_01b3);
                        }
                        h
                    }
                    _ => 0x2545_F491_4F6C_DD1D,
                };
                vm.rng_state = if s == 0 { 1 } else { s };
            }
            Ok(PyVal::None)
        }
        "random" => {
            let r = (vm.next_rand_u64() >> 11) as f64 / ((1u64 << 53) as f64);
            Ok(PyVal::Float(r))
        }
        "randint" => {
            check_argc(vm, "randint", &args, 2, 2)?;
            let a = require_int(vm, &args[0], "randint")?;
            let b = require_int(vm, &args[1], "randint")?;
            if a > b {
                return Err(vm.runtime_err("ValueError", "empty range for randint()"));
            }
            let span = (b as i128 - a as i128 + 1) as u64;
            let pick = a + (vm.next_rand_u64() % span) as i64;
            Ok(PyVal::Int(pick))
        }
        "randrange" => {
            check_argc(vm, "randrange", &args, 1, 3)?;
            let (start, stop, step) = match args.len() {
                1 => (0, require_int(vm, &args[0], "randrange")?, 1),
                2 => (
                    require_int(vm, &args[0], "randrange")?,
                    require_int(vm, &args[1], "randrange")?,
                    1,
                ),
                _ => (
                    require_int(vm, &args[0], "randrange")?,
                    require_int(vm, &args[1], "randrange")?,
                    require_int(vm, &args[2], "randrange")?,
                ),
            };
            if step == 0 {
                return Err(vm.runtime_err("ValueError", "zero step for randrange()"));
            }
            let count = if step > 0 && stop > start {
                ((stop - start - 1) / step + 1) as u64
            } else if step < 0 && start > stop {
                ((start - stop - 1) / (-step) + 1) as u64
            } else {
                return Err(vm.runtime_err("ValueError", "empty range for randrange()"));
            };
            let idx = (vm.next_rand_u64() % count) as i64;
            Ok(PyVal::Int(start + idx * step))
        }
        "uniform" => {
            check_argc(vm, "uniform", &args, 2, 2)?;
            let a = args[0].as_float().ok_or_else(|| vm.runtime_err("TypeError", "uniform() expects numbers"))?;
            let b = args[1].as_float().ok_or_else(|| vm.runtime_err("TypeError", "uniform() expects numbers"))?;
            let t = (vm.next_rand_u64() >> 11) as f64 / ((1u64 << 53) as f64);
            Ok(PyVal::Float(a + (b - a) * t))
        }
        "choice" => {
            check_argc(vm, "choice", &args, 1, 1)?;
            let seq = vm.to_vec(&args[0])?;
            if seq.is_empty() {
                return Err(vm.runtime_err("IndexError", "cannot choose from an empty sequence"));
            }
            let idx = (vm.next_rand_u64() as usize) % seq.len();
            Ok(seq[idx].clone())
        }
        "shuffle" => {
            check_argc(vm, "shuffle", &args, 1, 1)?;
            let PyVal::Handle(id) = args[0] else {
                return Err(vm.runtime_err("TypeError", "shuffle() expects a mutable list"));
            };
            let len = match vm.heap.get(id) {
                Some(HeapObj::List(it)) => it.len(),
                _ => return Err(vm.runtime_err("TypeError", "shuffle() expects a list")),
            };
            let mut swaps = Vec::new();
            for i in (1..len).rev() {
                let j = (vm.next_rand_u64() as usize) % (i + 1);
                swaps.push((i, j));
            }
            if let Some(HeapObj::List(items)) = vm.heap.get_mut(id) {
                for (i, j) in swaps {
                    items.swap(i, j);
                }
            }
            Ok(PyVal::None)
        }
        "sample" => {
            check_argc(vm, "sample", &args, 2, 2)?;
            let mut pool = vm.to_vec(&args[0])?;
            let k = require_int(vm, &args[1], "sample")?;
            if k < 0 || (k as usize) > pool.len() {
                return Err(vm.runtime_err("ValueError", "sample larger than population or negative"));
            }
            let mut out = Vec::with_capacity(k as usize);
            for _ in 0..(k as usize) {
                let idx = (vm.next_rand_u64() as usize) % pool.len();
                out.push(pool.swap_remove(idx));
            }
            let id = vm.alloc(HeapObj::List(out))?;
            Ok(PyVal::Handle(id))
        }
        _ => Err(vm.runtime_err("AttributeError", format!("module 'random' has no attribute '{name}'"))),
    }
}

fn call_json_fn(
    vm: &mut PyVm,
    name: &str,
    args: Vec<PyVal>,
    kwargs: BTreeMap<String, PyVal>,
) -> Result<PyVal, PyError> {
    match name {
        "dumps" => {
            check_argc(vm, "json.dumps", &args, 1, 1)?;
            let indent = kwargs
                .get("indent")
                .and_then(PyVal::as_int)
                .map(|n| n.clamp(0, 16) as usize);
            let sort_keys = kwargs
                .get("sort_keys")
                .is_some_and(|v| vm.is_truthy(v));
            let s = json_dumps(&vm.heap, &args[0], indent, sort_keys)
                .map_err(|e| vm.runtime_err("TypeError", e))?;
            Ok(PyVal::Str(s))
        }
        "loads" => {
            check_argc(vm, "json.loads", &args, 1, 1)?;
            let s = require_str(vm, &args[0], "json.loads")?;
            json_loads(&mut vm.heap, s).map_err(|e| vm.runtime_err("ValueError", e))
        }
        _ => Err(vm.runtime_err("AttributeError", format!("module 'json' has no attribute '{name}'"))),
    }
}

fn py_len(vm: &PyVm, val: &PyVal, host: Option<&HostBridge<'_>>) -> Result<usize, PyError> {
    match val {
        PyVal::Str(s) => Ok(s.chars().count()),
        PyVal::Tuple(items) => Ok(items.len()),
        PyVal::Handle(id) => match vm.heap.get(*id) {
            Some(HeapObj::List(items)) => Ok(items.len()),
            Some(HeapObj::Dict(entries)) => Ok(entries.len()),
            Some(HeapObj::Set(items)) => Ok(items.len()),
            Some(HeapObj::Native(NativeObjKind::Storage)) => {
                Ok(host.map_or(0, |h| h.storage_snapshot_map().len()))
            }
            _ => Err(vm.runtime_err(
                "TypeError",
                format!("object of type '{}' has no len()", vm.type_name(val)),
            )),
        },
        _ => Err(vm.runtime_err(
            "TypeError",
            format!("object of type '{}' has no len()", vm.type_name(val)),
        )),
    }
}

fn sort_py_items(
    vm: &mut PyVm,
    items: Vec<PyVal>,
    key_fn: Option<PyVal>,
    reverse: bool,
    mut host: Option<&mut HostBridge<'_>>,
) -> Result<Vec<PyVal>, PyError> {
    let mut keyed: Vec<(PyVal, PyVal)> = Vec::with_capacity(items.len());
    for item in items {
        vm.consume_fuel(1)?;
        let k = match &key_fn {
            Some(kf) => vm.call_value(kf.clone(), vec![item.clone()], BTreeMap::new(), host.as_deref_mut())?,
            None => item.clone(),
        };
        keyed.push((k, item));
    }
    // Insertion sort / merge sort so comparison errors propagate cleanly without panicking.
    let mut err: Option<PyError> = None;
    for i in 1..keyed.len() {
        vm.consume_fuel(1)?;
        let mut j = i;
        while j > 0 {
            let ord = match vm.val_cmp(&keyed[j - 1].0, &keyed[j].0) {
                Ok(o) => o,
                Err(e) => {
                    err = Some(e);
                    break;
                }
            };
            let swap = if reverse {
                ord == std::cmp::Ordering::Less
            } else {
                ord == std::cmp::Ordering::Greater
            };
            if swap {
                keyed.swap(j - 1, j);
                j -= 1;
            } else {
                break;
            }
        }
        if let Some(e) = err {
            return Err(e);
        }
    }
    Ok(keyed.into_iter().map(|(_, item)| item).collect())
}

fn check_isinstance(vm: &PyVm, val: &PyVal, ty: &PyVal) -> bool {
    if let PyVal::Tuple(ts) = ty {
        return ts.iter().any(|t| check_isinstance(vm, val, t));
    }
    if let PyVal::Handle(tid) = ty {
        match vm.heap.get(*tid) {
            Some(HeapObj::NativeFn(n)) => {
                let target = n.strip_prefix("builtin.").unwrap_or(n);
                return match target {
                    "bool" => matches!(val, PyVal::Bool(_)),
                    "int" => matches!(val, PyVal::Int(_) | PyVal::Bool(_)),
                    "float" => matches!(val, PyVal::Float(_)),
                    "str" => matches!(val, PyVal::Str(_)),
                    "tuple" => matches!(val, PyVal::Tuple(_)),
                    "list" => matches!(val, PyVal::Handle(id) if matches!(vm.heap.get(*id), Some(HeapObj::List(_)))),
                    "dict" => matches!(val, PyVal::Handle(id) if matches!(vm.heap.get(*id), Some(HeapObj::Dict(_)))),
                    "set" => matches!(val, PyVal::Handle(id) if matches!(vm.heap.get(*id), Some(HeapObj::Set(_)))),
                    "cs16.Player" => matches!(val, PyVal::Handle(id) if matches!(vm.heap.get(*id), Some(HeapObj::Native(NativeObjKind::Player(_))))),
                    "cs16.Menu" => matches!(val, PyVal::Handle(id) if matches!(vm.heap.get(*id), Some(HeapObj::Native(NativeObjKind::Menu(_))))),
                    "cs16.CustomWeapon" => matches!(val, PyVal::Handle(id) if matches!(vm.heap.get(*id), Some(HeapObj::Native(NativeObjKind::CustomWeapon(_))))),
                    _ => false,
                };
            }
            Some(HeapObj::Class(_)) => {
                if let PyVal::Handle(vid) = val
                    && let Some(HeapObj::Instance(inst)) = vm.heap.get(*vid)
                {
                    return is_class_subclass(vm, inst.class_id, *tid);
                }
            }
            _ => {}
        }
    }
    false
}

fn is_class_subclass(vm: &PyVm, sub: crate::value::ObjId, target: crate::value::ObjId) -> bool {
    if sub == target {
        return true;
    }
    if let Some(HeapObj::Class(c)) = vm.heap.get(sub) {
        return c.bases.iter().any(|&b| is_class_subclass(vm, b, target));
    }
    false
}

/// Formats a Python value according to a PEP 3101 format spec (e.g. `.2f`, `02d`, `>8`, `<10s`, `x`, `,`).
pub fn format_py_val(vm: &PyVm, val: &PyVal, spec: &str) -> Result<String, PyError> {
    if spec.is_empty() {
        return Ok(vm.val_to_str(val));
    }
    let chars: Vec<char> = spec.chars().collect();
    let mut pos = 0usize;
    let mut fill = ' ';
    let mut align: Option<char> = None;

    if chars.len() >= 2 && matches!(chars[1], '<' | '>' | '^' | '=') {
        fill = chars[0];
        align = Some(chars[1]);
        pos = 2;
    } else if !chars.is_empty() && matches!(chars[0], '<' | '>' | '^' | '=') {
        align = Some(chars[0]);
        pos = 1;
    }

    let mut sign = '\0';
    if pos < chars.len() && matches!(chars[pos], '+' | '-' | ' ') {
        sign = chars[pos];
        pos += 1;
    }

    let mut zero_pad = false;
    if pos < chars.len() && chars[pos] == '0' {
        zero_pad = true;
        pos += 1;
    }

    let mut width_str = String::new();
    while pos < chars.len() && chars[pos].is_ascii_digit() {
        width_str.push(chars[pos]);
        pos += 1;
    }
    let width: usize = width_str.parse().unwrap_or(0);

    let mut grouping = false;
    if pos < chars.len() && chars[pos] == ',' {
        grouping = true;
        pos += 1;
    }

    let mut prec: Option<usize> = None;
    if pos < chars.len() && chars[pos] == '.' {
        pos += 1;
        let mut p_str = String::new();
        while pos < chars.len() && chars[pos].is_ascii_digit() {
            p_str.push(chars[pos]);
            pos += 1;
        }
        prec = Some(p_str.parse().unwrap_or(0));
    }

    let ty = if pos < chars.len() {
        Some(chars[pos])
    } else {
        None
    };

    let mut body = match ty {
        Some('f' | 'F') => {
            let f = val
                .as_float()
                .ok_or_else(|| vm.runtime_err("ValueError", "format 'f' requires a number"))?;
            let p = prec.unwrap_or(6);
            let s = format!("{:.*}", p, f.abs());
            apply_sign(f < 0.0, sign, if grouping { add_commas(&s) } else { s })
        }
        Some('d') => {
            let n = val
                .as_int()
                .or_else(|| val.as_float().map(|f| f as i64))
                .ok_or_else(|| vm.runtime_err("ValueError", "format 'd' requires an integer"))?;
            let s = n.unsigned_abs().to_string();
            apply_sign(n < 0, sign, if grouping { add_commas(&s) } else { s })
        }
        Some('x') => {
            let n = val
                .as_int()
                .ok_or_else(|| vm.runtime_err("ValueError", "format 'x' requires an integer"))?;
            apply_sign(n < 0, sign, format!("{:x}", n.unsigned_abs()))
        }
        Some('X') => {
            let n = val
                .as_int()
                .ok_or_else(|| vm.runtime_err("ValueError", "format 'X' requires an integer"))?;
            apply_sign(n < 0, sign, format!("{:X}", n.unsigned_abs()))
        }
        Some('%') => {
            let f = val
                .as_float()
                .ok_or_else(|| vm.runtime_err("ValueError", "format '%' requires a number"))?;
            let p = prec.unwrap_or(6);
            format!("{:.*}%", p, f * 100.0)
        }
        Some('s') | None => {
            let mut s = vm.val_to_str(val);
            if let Some(p) = prec {
                if val.as_float().is_some() && !matches!(val, PyVal::Str(_)) {
                    let f = val.as_float().unwrap();
                    s = format!("{:.*}", p, f);
                } else {
                    s = s.chars().take(p).collect();
                }
            }
            s
        }
        Some(_) => vm.val_to_str(val),
    };

    let char_len = body.chars().count();
    if width > char_len {
        let diff = width - char_len;
        let is_num = val.as_float().is_some();
        let eff_align = align.unwrap_or(if is_num { '>' } else { '<' });
        let eff_fill = if zero_pad && align.is_none() { '0' } else { fill };
        body = match eff_align {
            '<' => format!("{body}{}", eff_fill.to_string().repeat(diff)),
            '>' => {
                if eff_fill == '0' && (body.starts_with('-') || body.starts_with('+')) {
                    let prefix = &body[..1];
                    let rest = &body[1..];
                    format!("{prefix}{}{rest}", "0".repeat(diff))
                } else {
                    format!("{}{body}", eff_fill.to_string().repeat(diff))
                }
            }
            '^' => {
                let left = diff / 2;
                let right = diff - left;
                format!(
                    "{}{body}{}",
                    eff_fill.to_string().repeat(left),
                    eff_fill.to_string().repeat(right)
                )
            }
            _ => format!("{}{body}", eff_fill.to_string().repeat(diff)),
        };
    }
    Ok(body)
}

fn apply_sign(neg: bool, sign: char, body: String) -> String {
    if neg {
        format!("-{body}")
    } else if sign == '+' {
        format!("+{body}")
    } else if sign == ' ' {
        format!(" {body}")
    } else {
        body
    }
}

fn add_commas(s: &str) -> String {
    let (int_part, frac_part) = match s.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (s, None),
    };
    let mut rev = String::new();
    for (idx, ch) in int_part.chars().rev().enumerate() {
        if idx > 0 && idx % 3 == 0 {
            rev.push(',');
        }
        rev.push(ch);
    }
    let formatted_int: String = rev.chars().rev().collect();
    match frac_part {
        Some(f) => format!("{formatted_int}.{f}"),
        None => formatted_int,
    }
}

/// Classic Python `%` string formatting (`"%s: %d" % (name, score)`).
pub fn printf_percent_format(vm: &PyVm, fmt: &str, rhs: &PyVal) -> Result<PyVal, PyError> {
    let args: Vec<PyVal> = match rhs {
        PyVal::Tuple(items) => items.clone(),
        other => vec![other.clone()],
    };
    let chars: Vec<char> = fmt.chars().collect();
    let mut out = String::new();
    let mut i = 0usize;
    let mut arg_idx = 0usize;
    while i < chars.len() {
        if chars[i] == '%' && i + 1 < chars.len() {
            if chars[i + 1] == '%' {
                out.push('%');
                i += 2;
                continue;
            }
            let mut j = i + 1;
            let mut spec = String::new();
            while j < chars.len()
                && (chars[j].is_ascii_digit() || matches!(chars[j], '.' | '-' | '+' | '0'))
            {
                spec.push(chars[j]);
                j += 1;
            }
            if j < chars.len() && matches!(chars[j], 's' | 'd' | 'i' | 'f' | 'r' | 'x' | 'X') {
                let Some(val) = args.get(arg_idx) else {
                    return Err(vm.runtime_err("TypeError", "not enough arguments for format string"));
                };
                arg_idx += 1;
                let ty = chars[j];
                if ty == 'r' {
                    out.push_str(&vm.val_to_repr(val));
                } else {
                    let norm_ty = if ty == 'i' { 'd' } else { ty };
                    spec.push(norm_ty);
                    out.push_str(&format_py_val(vm, val, &spec)?);
                }
                i = j + 1;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    Ok(PyVal::Str(out))
}

fn check_argc(
    vm: &PyVm,
    name: &str,
    args: &[PyVal],
    min: usize,
    max: usize,
) -> Result<(), PyError> {
    if args.len() < min || args.len() > max {
        Err(vm.runtime_err(
            "TypeError",
            format!("{name}() expected {min}..={max} arguments, got {}", args.len()),
        ))
    } else {
        Ok(())
    }
}

fn require_int(vm: &PyVm, val: &PyVal, name: &str) -> Result<i64, PyError> {
    val.as_int()
        .ok_or_else(|| vm.runtime_err("TypeError", format!("{name}() expected int")))
}

fn require_str<'a>(vm: &PyVm, val: &'a PyVal, name: &str) -> Result<&'a str, PyError> {
    val.as_str()
        .ok_or_else(|| vm.runtime_err("TypeError", format!("{name}() expected str")))
}
