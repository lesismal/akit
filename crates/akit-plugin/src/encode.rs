//! A WebAssembly encoder for tests' own small modules, written out instruction by instruction:
//! plugins that misbehave in ways a compiler would not let them, built without a toolchain
//! (fit's `plugin/wasm/encode_test.go`). Not an API: for akit-plugin's tests.

/// The instructions the tests' modules use.
pub const OP_UNREACHABLE: u8 = 0x00;
pub const OP_LOOP: u8 = 0x03;
pub const OP_IF: u8 = 0x04;
pub const OP_BR: u8 = 0x0c;
pub const OP_END: u8 = 0x0b;
pub const OP_CALL: u8 = 0x10;
pub const OP_LOCAL_GET: u8 = 0x20;
pub const OP_I32_CONST: u8 = 0x41;
pub const OP_DROP: u8 = 0x1a;
pub const OP_I32_LOAD: u8 = 0x28;
pub const OP_I32_EQZ: u8 = 0x45;
pub const OP_I64_EXTEND_I32_S: u8 = 0xac;
pub const BLOCK_EMPTY: u8 = 0x40;
pub const TYPE_I32: u8 = 0x7f;
pub const TYPE_I64: u8 = 0x7e;

/// A function a module imports, of i32 params and results (or the params `param_types` says).
#[derive(Debug, Clone, Default)]
pub struct Import {
    pub module: String,
    pub name: String,
    pub params: usize,
    pub results: usize,
    pub param_types: Vec<u8>,
}

impl Import {
    /// `module`.`name`, of `params` i32 params and `results` i32 results.
    #[must_use]
    pub fn new(module: &str, name: &str, params: usize, results: usize) -> Import {
        Import {
            module: module.to_owned(),
            name: name.to_owned(),
            params,
            results,
            param_types: Vec::new(),
        }
    }
}

/// A function a module defines, exported as `export` unless that is empty, of i32 params and
/// results; `body` leaves out the final `end`.
#[derive(Debug, Clone, Default)]
pub struct Func {
    pub export: String,
    pub params: usize,
    pub results: usize,
    pub body: Vec<u8>,
}

impl Func {
    /// An exported function.
    #[must_use]
    pub fn export(name: &str, params: usize, results: usize, body: Vec<u8>) -> Func {
        Func {
            export: name.to_owned(),
            params,
            results,
            body,
        }
    }
}

/// A module: its imports and functions, a memory of `pages` exported as `memory` unless `pages`
/// is 0, and the bytes `data` writes into it at its offsets.
#[derive(Debug, Clone, Default)]
pub struct Module {
    pub imports: Vec<Import>,
    pub funcs: Vec<Func>,
    pub pages: u32,
    pub data: Vec<(u32, Vec<u8>)>,
}

fn uleb(b: &mut Vec<u8>, mut v: u64) {
    loop {
        let c = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            b.push(c);
            return;
        }
        b.push(c | 0x80);
    }
}

fn sleb(b: &mut Vec<u8>, mut v: i64) {
    loop {
        let c = (v & 0x7f) as u8;
        v >>= 7;
        if (v == 0 && c & 0x40 == 0) || (v == -1 && c & 0x40 != 0) {
            b.push(c);
            return;
        }
        b.push(c | 0x80);
    }
}

fn name(b: &mut Vec<u8>, s: &str) {
    uleb(b, s.len() as u64);
    b.extend_from_slice(s.as_bytes());
}

fn section(b: &mut Vec<u8>, id: u8, count: usize, items: &[u8]) {
    let mut content = Vec::new();
    uleb(&mut content, count as u64);
    content.extend_from_slice(items);
    b.push(id);
    uleb(b, content.len() as u64);
    b.extend_from_slice(&content);
}

fn func_type(b: &mut Vec<u8>, params: usize, results: usize) {
    func_type_of(b, &vec![TYPE_I32; params], results);
}

fn func_type_of(b: &mut Vec<u8>, params: &[u8], results: usize) {
    b.push(0x60);
    uleb(b, params.len() as u64);
    b.extend_from_slice(params);
    uleb(b, results as u64);
    b.extend(std::iter::repeat_n(TYPE_I32, results));
}

/// A body that calls function `f` with the i32 constants `args`, dropping what it returns
/// (`results` of them), then returns `ret`.
#[must_use]
pub fn call_with(f: u32, args: &[i32], results: usize, ret: Option<i32>) -> Vec<u8> {
    let mut b = Vec::new();
    for &a in args {
        b.extend(i32_const(a));
    }
    b.push(OP_CALL);
    uleb(&mut b, u64::from(f));
    b.extend(std::iter::repeat_n(OP_DROP, results));
    if let Some(r) = ret {
        b.extend(i32_const(r));
    }
    b
}

/// The instruction `i32.const v`.
#[must_use]
pub fn i32_const(v: i32) -> Vec<u8> {
    let mut b = vec![OP_I32_CONST];
    sleb(&mut b, i64::from(v));
    b
}

/// A body that calls function `f` with its first `params` params, and returns what it returns.
#[must_use]
pub fn forward(f: u32, params: usize) -> Vec<u8> {
    let mut b = Vec::new();
    for i in 0..params {
        b.push(OP_LOCAL_GET);
        uleb(&mut b, i as u64);
    }
    b.push(OP_CALL);
    uleb(&mut b, u64::from(f));
    b
}

/// A body that loops for ever.
#[must_use]
pub fn spin() -> Vec<u8> {
    vec![OP_LOOP, BLOCK_EMPTY, OP_BR, 0, OP_END, OP_UNREACHABLE]
}

impl Module {
    /// The module's binary.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut b = vec![0x00, b'a', b's', b'm', 0x01, 0x00, 0x00, 0x00];
        // One type for each import and function, in that order.
        let mut types = Vec::new();
        for im in &self.imports {
            if im.param_types.is_empty() {
                func_type(&mut types, im.params, im.results);
            } else {
                func_type_of(&mut types, &im.param_types, im.results);
            }
        }
        for f in &self.funcs {
            func_type(&mut types, f.params, f.results);
        }
        section(&mut b, 1, self.imports.len() + self.funcs.len(), &types);
        if !self.imports.is_empty() {
            let mut imports = Vec::new();
            for (i, im) in self.imports.iter().enumerate() {
                name(&mut imports, &im.module);
                name(&mut imports, &im.name);
                imports.push(0x00);
                uleb(&mut imports, i as u64);
            }
            section(&mut b, 2, self.imports.len(), &imports);
        }
        let mut funcs = Vec::new();
        for i in 0..self.funcs.len() {
            uleb(&mut funcs, (self.imports.len() + i) as u64);
        }
        section(&mut b, 3, self.funcs.len(), &funcs);
        if self.pages > 0 {
            let mut mem = vec![0x00];
            uleb(&mut mem, u64::from(self.pages));
            section(&mut b, 5, 1, &mem);
        }
        let mut exports = Vec::new();
        let mut n = 0;
        if self.pages > 0 {
            name(&mut exports, "memory");
            exports.extend_from_slice(&[0x02, 0x00]);
            n += 1;
        }
        for (i, f) in self.funcs.iter().enumerate() {
            if !f.export.is_empty() {
                name(&mut exports, &f.export);
                exports.push(0x00);
                uleb(&mut exports, (self.imports.len() + i) as u64);
                n += 1;
            }
        }
        section(&mut b, 7, n, &exports);
        let mut code = Vec::new();
        for f in &self.funcs {
            let mut body = vec![0x00]; // no locals
            body.extend_from_slice(&f.body);
            body.push(OP_END);
            uleb(&mut code, body.len() as u64);
            code.extend_from_slice(&body);
        }
        section(&mut b, 10, self.funcs.len(), &code);
        if !self.data.is_empty() {
            let mut data = Vec::new();
            for (at, bytes) in &self.data {
                data.push(0x00);
                data.push(OP_I32_CONST);
                sleb(&mut data, i64::from(*at as i32));
                data.push(OP_END);
                uleb(&mut data, bytes.len() as u64);
                data.extend_from_slice(bytes);
            }
            section(&mut b, 11, self.data.len(), &data);
        }
        b
    }
}
