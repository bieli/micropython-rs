//! Rich return values from the bridge (`HwFn::returning_values`,
//! `HardwareBridge::call_values`): floats, strings, bytes, tuples, lists and
//! dicts made by Rust code arrive in Python as the objects they should be.
#![cfg(all(feature = "format", feature = "float"))]

use micropython_rs::{Arg, Error, HardwareBridge, HwFn, Interpreter, Module, Output, Returns};

const FUNCS: &[HwFn] = &[
    HwFn::new("scalars", "", "").returning_values(),
    HwFn::new("nothing", "", "").returning_values(),
    HwFn::new("several", "", "").returning_values(),
    HwFn::with_args("echo", 1, 2, "s, [times]", "").returning_values(),
    HwFn::new("nested", "", "").returning_values(),
    HwFn::new("many", "n", "").returning_values(),
    HwFn::new("unclosed", "", "").returning_values(),
    HwFn::new("bad_key", "", "").returning_values(),
    HwFn::new("big", "n", "").returning_values(),
    HwFn::new("plain", "", ""),
];

const SENSOR: Module = Module {
    name: "sensor",
    doc: "",
    functions: &[HwFn::new("read", "", "").returning_values()],
    constants: &[],
};

struct Bridge;

impl HardwareBridge for Bridge {
    fn call_args(&mut self, name: &str, _args: &[Arg<'_>]) -> Result<Option<i64>, Error> {
        match name {
            "plain" => Ok(Some(7)),
            _ => Err(Error::UnknownIdentifier),
        }
    }

    fn call_values(
        &mut self,
        module: Option<&str>,
        name: &str,
        args: &[Arg<'_>],
        out: &mut Returns<'_>,
    ) -> Result<(), Error> {
        match (module, name) {
            (Some("sensor"), "read") => {
                // A BME280-style reading: (°C, hPa, %)
                out.begin_tuple()?;
                out.float(21.37)?;
                out.float(1013.2)?;
                out.float(45.8)?;
                out.end()
            }
            (None, "scalars") => {
                out.begin_list()?;
                out.none()?;
                out.bool(true)?;
                out.bool(false)?;
                out.int(-42)?;
                out.int(i64::MAX)?;
                out.float(0.5)?;
                out.str("żółć €")?;
                out.bytes(&[0, 1, 255])?;
                out.end()
            }
            (None, "nothing") => Ok(()),
            (None, "several") => {
                out.int(1)?;
                out.str("two")
            }
            (None, "echo") => {
                // The argument is a heap string: still valid while recording
                let s = args[0].str()?;
                let times = args.get(1).map_or(Ok(1), |a| a.int())?;
                out.begin_tuple()?;
                for _ in 0..times {
                    out.str(s)?;
                }
                out.int(s.len() as i64)?;
                out.end()
            }
            (None, "nested") => {
                // {'name': 'net', 'ip': (192, 168, 4, 1), 'tags': ['a', ['b', ()]], 'n': None}
                out.begin_dict()?;
                out.str("name")?;
                out.str("net")?;
                out.str("ip")?;
                out.begin_tuple()?;
                for b in [192, 168, 4, 1] {
                    out.int(b)?;
                }
                out.end()?;
                out.str("tags")?;
                out.begin_list()?;
                out.str("a")?;
                out.begin_list()?;
                out.str("b")?;
                out.begin_tuple()?;
                out.end()?;
                out.end()?;
                out.end()?;
                out.str("n")?;
                out.none()?;
                out.end()
            }
            (None, "many") => {
                // More items than the value stack holds: built in chunks
                let n = args[0].int()?;
                out.begin_list()?;
                for i in 0..n {
                    out.begin_tuple()?;
                    out.int(i)?;
                    out.str("ssid")?;
                    out.float(i as f64 / 2.0)?;
                    out.end()?;
                }
                out.end()
            }
            (None, "unclosed") => out.begin_list(),
            (None, "bad_key") => {
                out.begin_dict()?;
                out.begin_list()?;
                out.end()?;
                out.int(1)?;
                out.end()
            }
            (None, "big") => {
                let n = args[0].int()? as usize;
                out.str(&"x".repeat(n))
            }
            _ => Err(Error::UnknownIdentifier),
        }
    }

    fn functions(&self) -> &[HwFn] {
        FUNCS
    }

    fn modules(&self) -> &[Module] {
        &[SENSOR]
    }
}

struct Text(String);

impl Output for Text {
    fn write_str(&mut self, s: &str) {
        self.0.push_str(s);
    }
}

fn run_heap(script: &str, heap_size: usize) -> Result<String, String> {
    // An object table for big results (300 tuples with a string each)
    run_table(script, heap_size, 2048)
}

fn run_table(script: &str, heap_size: usize, objects: usize) -> Result<String, String> {
    let mut out = Text(String::new());
    let mut bridge = Bridge;
    let mut heap = vec![0u8; heap_size];
    let mut table = vec![micropython_rs::heap::HeapEntry::FREE; objects];
    let mut vm = Interpreter::with_hardware(&mut out, &mut bridge)
        .with_heap(&mut heap)
        .with_heap_table(&mut table);
    let result = vm.eval(script);
    let mut message = Text(String::new());
    if let Err(err) = result {
        vm.write_exception(err, &mut message);
    }
    let _ = vm;
    if message.0.is_empty() {
        Ok(out.0)
    } else {
        Err(message.0)
    }
}

fn run(script: &str) -> String {
    run_heap(script, 32 * 1024).unwrap_or_else(|e| panic!("{}", e))
}

#[test]
fn scalars_and_strings() {
    let out = run("v = scalars()\nprint(v)\nprint([type(x).__name__ for x in v])\nprint(len(v[6]), v[6][0])\n");
    let mut lines = out.lines();
    assert_eq!(
        lines.next(),
        Some("[None, True, False, -42, 9223372036854775807, 0.5, 'żółć €', b'\\x00\\x01\\xff']")
    );
    assert_eq!(
        lines.next(),
        Some("['NoneType', 'bool', 'bool', 'int', 'int', 'float', 'str', 'bytes']")
    );
    assert_eq!(lines.next(), Some("6 ż"));
}

#[test]
fn none_several_and_modules() {
    let out = run("print(nothing(), several(), plain())\n\
         import sensor\n\
         t, p, h = sensor.read()\n\
         print(round(t, 2), p, h, type(t).__name__)\n");
    assert_eq!(out, "None (1, 'two') 7\n21.37 1013.2 45.8 float\n");
}

#[test]
fn arguments_stay_valid() {
    // A heap string argument (built at runtime), echoed back three times
    let out =
        run("s = 'ab' * 20 + str(7)\nr = echo(s, 3)\nprint(r[0] == s, r[2] == s, r[3], len(r))\n");
    assert_eq!(out, "True True 41 4\n");
}

#[test]
fn nested_containers() {
    let out = run("d = nested()\nprint(d)\nprint(d['ip'][3], d['tags'][1][0], d['tags'][1][1])\n");
    assert_eq!(
        out,
        "{'name': 'net', 'ip': (192, 168, 4, 1), 'tags': ['a', ['b', ()]], 'n': None}\n1 b ()\n"
    );
}

#[test]
fn long_lists_are_built_in_chunks() {
    // 300 tuples: more values than the 128-entry value stack
    let out = run_heap(
        "r = many(300)\nprint(len(r), r[0], r[299], sum(x[0] for x in r))\n",
        64 * 1024,
    )
    .unwrap();
    assert_eq!(out, "300 (0, 'ssid', 0.0) (299, 'ssid', 149.5) 44850\n");
}

#[test]
fn errors() {
    // A container not ended: ValueError; a list as a dict key: TypeError
    assert!(run_heap("unclosed()", 8192)
        .unwrap_err()
        .starts_with("ValueError"));
    assert!(run_heap("bad_key()", 8192)
        .unwrap_err()
        .starts_with("TypeError"));
    // More than the heap's free memory: MemoryError, which a script can catch
    assert!(run_heap("big(10000)", 4096)
        .unwrap_err()
        .starts_with("MemoryError"));
    let out = run_heap(
        "try:\n    big(10000)\nexcept MemoryError:\n    print('caught')\nprint(len(big(100)))\n",
        4096,
    )
    .unwrap();
    assert_eq!(out, "caught\n100\n");
}

#[test]
fn collections_while_building() {
    // A small heap: making the objects collects garbage (the scratch area
    // and the half-built values must survive it)
    let out = run_heap(
        "junk = None\nfor k in range(20):\n    junk = [str(i) for i in range(20)]\n    r = many(40)\n\
         print(len(r), r[39], len(junk))\n",
        12 * 1024,
    )
    .unwrap();
    assert_eq!(out, "40 (39, 'ssid', 19.5) 20\n");
}

#[test]
fn full_object_table_or_little_free_memory_collects_first() {
    // Found on an ESP32-C3 (a small object table): a loop of calls fills the
    // table with garbage tuples; each call must collect, not fail
    let out = run_table(
        "for i in range(300):\n    r = several()\nprint(r)\n",
        16 * 1024,
        48,
    )
    .unwrap();
    assert_eq!(out, "(1, 'two')\n");
    // Garbage leaves only a few hundred bytes free: a 2000-byte result fits
    let out = run_heap(
        "junk = 'x' * 7600\njunk = None\nfor i in range(20):\n    s = big(2000)\nprint(len(s))\n",
        8 * 1024,
    )
    .unwrap();
    assert_eq!(out, "2000\n");
}
