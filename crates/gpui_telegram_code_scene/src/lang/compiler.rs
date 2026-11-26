//! Two-pass bytecode compiler for `CodeSceneScript` (`cs1`).
//!
//! Resolves top-level `let` globals and forward/mutually-recursive functions,
//! lowers control flow with loop `break`/`continue` backpatching, short-circuit
//! `&&`/`||`, single-evaluation compound L-value assignments, enforces
//! `max_bytecode_ops` and `max_constants`, and extracts static `feature_tokens`.

use std::collections::{BTreeMap, BTreeSet};
use std::f64::consts::{PI, TAU};
use std::sync::Arc;

use crate::lang::ast::{AssignOp, BinOp, Expr, FnDecl, LValue, Program, Stmt, TopItem, UnaryOp};
use crate::lang::limits::VmBudgets;
use crate::lang::stdlib::BuiltinId;
use crate::model::{CompileDiagnostic, DiagnosticStage, Span};

#[derive(Debug, Clone)]
pub enum ConstVal {
    Num(f64),
    Str(Arc<str>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Const(u16),
    Null,
    True,
    False,
    Pop,
    Dup,
    Dup2,
    GetLocal(u16),
    SetLocal(u16),
    GetGlobal(u16),
    SetGlobal(u16),
    MakeList(u16),
    MakeMap(u16),
    GetField(u16),
    SetField(u16),
    GetIndex,
    SetIndex,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Neg,
    Not,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Jump(u32),
    JumpIfFalse(u32),
    JumpIfFalsePeek(u32),
    JumpIfTruePeek(u32),
    CallFn { fn_idx: u16, argc: u8 },
    CallBuiltin { builtin: BuiltinId, argc: u8 },
    CallMethod { name_idx: u16, argc: u8 },
    Return,
}

#[derive(Debug, Clone)]
pub struct FunctionChunk {
    pub name: String,
    pub arity: u8,
    pub local_count: u16,
    pub ops: Vec<Op>,
}

#[derive(Debug, Clone)]
pub struct CompiledProgram {
    pub constants: Vec<ConstVal>,
    pub strings: Vec<Arc<str>>,
    pub globals: Vec<String>,
    pub init_chunk: Option<FunctionChunk>,
    pub functions: Vec<FunctionChunk>,
    pub fn_table: BTreeMap<String, u16>,
    pub feature_tokens: Vec<String>,
    pub total_bytecode_ops: usize,
}

impl CompiledProgram {
    pub fn has_function(&self, name: &str) -> bool {
        self.fn_table.contains_key(name)
    }

    pub fn function_arity(&self, name: &str) -> Option<u8> {
        let idx = *self.fn_table.get(name)?;
        self.functions.get(idx as usize).map(|f| f.arity)
    }

    pub fn called_builtins(&self) -> Vec<String> {
        self.feature_tokens.clone()
    }
}

pub fn compile(program: &Program, budgets: &VmBudgets) -> Result<CompiledProgram, CompileDiagnostic> {
    let compiler = Compiler {
        budgets: *budgets,
        constants: Vec::new(),
        num_const_map: BTreeMap::new(),
        str_const_map: BTreeMap::new(),
        strings: Vec::new(),
        string_map: BTreeMap::new(),
        globals: Vec::new(),
        global_map: BTreeMap::new(),
        fn_table: BTreeMap::new(),
        fn_arities: Vec::new(),
        feature_tokens: BTreeSet::new(),
        total_bytecode_ops: 0,
    };
    compiler.compile_program(program)
}

struct LoopPatch {
    break_jumps: Vec<usize>,
    continue_jumps: Vec<usize>,
}

struct FnBuilder {
    ops: Vec<Op>,
    scopes: Vec<BTreeMap<String, u16>>,
    next_local: u16,
    max_locals: u16,
    loops: Vec<LoopPatch>,
}

impl FnBuilder {
    fn new(params: &[String]) -> Self {
        let mut root = BTreeMap::new();
        for (i, p) in params.iter().enumerate() {
            root.insert(p.clone(), i as u16);
        }
        let n = params.len() as u16;
        Self {
            ops: Vec::with_capacity(64),
            scopes: vec![root],
            next_local: n,
            max_locals: n,
            loops: Vec::new(),
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(BTreeMap::new());
    }

    fn pop_scope(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }

    fn declare_local(&mut self, name: String) -> u16 {
        let slot = self.next_local;
        self.next_local = self.next_local.saturating_add(1);
        if self.next_local > self.max_locals {
            self.max_locals = self.next_local;
        }
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name, slot);
        }
        slot
    }

    fn alloc_hidden_local(&mut self) -> u16 {
        let slot = self.next_local;
        self.next_local = self.next_local.saturating_add(1);
        if self.next_local > self.max_locals {
            self.max_locals = self.next_local;
        }
        slot
    }

    fn resolve_local(&self, name: &str) -> Option<u16> {
        for scope in self.scopes.iter().rev() {
            if let Some(&slot) = scope.get(name) {
                return Some(slot);
            }
        }
        None
    }
}

struct Compiler {
    budgets: VmBudgets,
    constants: Vec<ConstVal>,
    num_const_map: BTreeMap<u64, u16>,
    str_const_map: BTreeMap<String, u16>,
    strings: Vec<Arc<str>>,
    string_map: BTreeMap<String, u16>,
    globals: Vec<String>,
    global_map: BTreeMap<String, u16>,
    fn_table: BTreeMap<String, u16>,
    fn_arities: Vec<u8>,
    feature_tokens: BTreeSet<String>,
    total_bytecode_ops: usize,
}

impl Compiler {
    fn check_const_budget(&self, span: Span) -> Result<(), CompileDiagnostic> {
        if self.constants.len() + self.strings.len() >= self.budgets.max_constants {
            return Err(CompileDiagnostic::new(
                DiagnosticStage::Compile,
                "E_CONST_BUDGET_EXCEEDED",
                format!(
                    "constant pool exceeds limit {}",
                    self.budgets.max_constants
                ),
                Some(span),
            ));
        }
        Ok(())
    }

    fn intern_num(&mut self, n: f64, span: Span) -> Result<u16, CompileDiagnostic> {
        let bits = n.to_bits();
        if let Some(&idx) = self.num_const_map.get(&bits) {
            return Ok(idx);
        }
        self.check_const_budget(span)?;
        let idx = self.constants.len() as u16;
        self.constants.push(ConstVal::Num(n));
        self.num_const_map.insert(bits, idx);
        Ok(idx)
    }

    fn intern_str_const(&mut self, s: Arc<str>, span: Span) -> Result<u16, CompileDiagnostic> {
        if s.len() > self.budgets.max_str_bytes {
            return Err(CompileDiagnostic::new(
                DiagnosticStage::Compile,
                "E_STR_TOO_LONG",
                format!(
                    "string constant exceeds max length {} bytes",
                    self.budgets.max_str_bytes
                ),
                Some(span),
            ));
        }
        if let Some(&idx) = self.str_const_map.get(s.as_ref()) {
            return Ok(idx);
        }
        self.check_const_budget(span)?;
        let idx = self.constants.len() as u16;
        self.str_const_map.insert(s.to_string(), idx);
        self.constants.push(ConstVal::Str(s));
        Ok(idx)
    }

    fn intern_ident(&mut self, name: &str, span: Span) -> Result<u16, CompileDiagnostic> {
        if name.len() > self.budgets.max_str_bytes {
            return Err(CompileDiagnostic::new(
                DiagnosticStage::Compile,
                "E_STR_TOO_LONG",
                format!(
                    "identifier exceeds max length {} bytes",
                    self.budgets.max_str_bytes
                ),
                Some(span),
            ));
        }
        if let Some(&idx) = self.string_map.get(name) {
            return Ok(idx);
        }
        self.check_const_budget(span)?;
        let idx = self.strings.len() as u16;
        self.string_map.insert(name.to_string(), idx);
        self.strings.push(Arc::from(name));
        Ok(idx)
    }

    fn emit(&mut self, fb: &mut FnBuilder, op: Op, span: Span) -> Result<usize, CompileDiagnostic> {
        self.total_bytecode_ops += 1;
        if self.total_bytecode_ops > self.budgets.max_bytecode_ops {
            return Err(CompileDiagnostic::new(
                DiagnosticStage::Compile,
                "E_BYTECODE_BUDGET_EXCEEDED",
                format!(
                    "bytecode size exceeds limit {} ops",
                    self.budgets.max_bytecode_ops
                ),
                Some(span),
            ));
        }
        let pc = fb.ops.len();
        fb.ops.push(op);
        Ok(pc)
    }

    fn patch_jump(fb: &mut FnBuilder, jump_pc: usize, target: usize) {
        let t = target as u32;
        match &mut fb.ops[jump_pc] {
            Op::Jump(dst)
            | Op::JumpIfFalse(dst)
            | Op::JumpIfFalsePeek(dst)
            | Op::JumpIfTruePeek(dst) => *dst = t,
            _ => unreachable!("attempted to patch non-jump op"),
        }
    }

    fn compile_program(mut self, program: &Program) -> Result<CompiledProgram, CompileDiagnostic> {
        // Pass 1: register top-level globals and function signatures
        let mut fn_decls: Vec<&FnDecl> = Vec::new();
        let mut global_inits: Vec<(u16, &Expr, Span)> = Vec::new();

        for item in &program.items {
            match item {
                TopItem::GlobalLet { name, value, span } => {
                    if self.global_map.contains_key(name) {
                        return Err(CompileDiagnostic::new(
                            DiagnosticStage::Compile,
                            "E_DUPLICATE_GLOBAL",
                            format!("duplicate top-level `let {name}`"),
                            Some(*span),
                        ));
                    }
                    let slot = self.globals.len() as u16;
                    self.globals.push(name.clone());
                    self.global_map.insert(name.clone(), slot);
                    global_inits.push((slot, value, *span));
                }
                TopItem::Fn(decl) => {
                    if self.fn_table.contains_key(&decl.name) {
                        return Err(CompileDiagnostic::new(
                            DiagnosticStage::Compile,
                            "E_DUPLICATE_FN",
                            format!("duplicate function definition `{}`", decl.name),
                            Some(decl.span),
                        ));
                    }
                    if decl.params.len() > 32 {
                        return Err(CompileDiagnostic::new(
                            DiagnosticStage::Compile,
                            "E_TOO_MANY_PARAMS",
                            format!("function `{}` exceeds 32 parameters", decl.name),
                            Some(decl.span),
                        ));
                    }
                    let mut seen_params = BTreeSet::new();
                    for param in &decl.params {
                        if !seen_params.insert(param.as_str()) {
                            return Err(CompileDiagnostic::new(
                                DiagnosticStage::Compile,
                                "E_DUPLICATE_PARAM",
                                format!("duplicate parameter `{param}` in function `{}`", decl.name),
                                Some(decl.span),
                            ));
                        }
                    }
                    let idx = fn_decls.len() as u16;
                    self.fn_table.insert(decl.name.clone(), idx);
                    self.fn_arities.push(decl.params.len() as u8);
                    fn_decls.push(decl);
                }
            }
        }

        // Compile optional top-level global initializer chunk
        let init_chunk = if global_inits.is_empty() {
            None
        } else {
            let mut fb = FnBuilder::new(&[]);
            for (slot, expr, span) in global_inits {
                self.compile_expr(&mut fb, expr)?;
                self.emit(&mut fb, Op::SetGlobal(slot), span)?;
            }
            self.emit(&mut fb, Op::Null, Span { line: 1, col: 1 })?;
            self.emit(&mut fb, Op::Return, Span { line: 1, col: 1 })?;
            Some(FunctionChunk {
                name: "__init_globals__".to_string(),
                arity: 0,
                local_count: fb.max_locals,
                ops: fb.ops,
            })
        };

        // Pass 2: compile all function bodies
        let mut functions = Vec::with_capacity(fn_decls.len());
        for decl in fn_decls {
            let mut fb = FnBuilder::new(&decl.params);
            for stmt in &decl.body {
                self.compile_stmt(&mut fb, stmt)?;
            }
            self.emit(&mut fb, Op::Null, decl.span)?;
            self.emit(&mut fb, Op::Return, decl.span)?;
            functions.push(FunctionChunk {
                name: decl.name.clone(),
                arity: decl.params.len() as u8,
                local_count: fb.max_locals,
                ops: fb.ops,
            });
        }

        Ok(CompiledProgram {
            constants: self.constants,
            strings: self.strings,
            globals: self.globals,
            init_chunk,
            functions,
            fn_table: self.fn_table,
            feature_tokens: self.feature_tokens.into_iter().collect(),
            total_bytecode_ops: self.total_bytecode_ops,
        })
    }
}

impl Compiler {
    fn compile_stmt(&mut self, fb: &mut FnBuilder, stmt: &Stmt) -> Result<(), CompileDiagnostic> {
        match stmt {
            Stmt::Let { name, value, span } => {
                self.compile_expr(fb, value)?;
                let slot = fb.declare_local(name.clone());
                self.emit(fb, Op::SetLocal(slot), *span)?;
                Ok(())
            }
            Stmt::Assign { target, op, value, span } => self.compile_assign(fb, target, *op, value, *span),
            Stmt::If { cond, then_block, else_block, span } => {
                self.compile_expr(fb, cond)?;
                let jump_else = self.emit(fb, Op::JumpIfFalse(0), *span)?;
                fb.push_scope();
                for s in then_block {
                    self.compile_stmt(fb, s)?;
                }
                fb.pop_scope();
                if let Some(eb) = else_block {
                    let jump_end = self.emit(fb, Op::Jump(0), *span)?;
                    let else_pc = fb.ops.len();
                    Self::patch_jump(fb, jump_else, else_pc);
                    fb.push_scope();
                    for s in eb {
                        self.compile_stmt(fb, s)?;
                    }
                    fb.pop_scope();
                    let end_pc = fb.ops.len();
                    Self::patch_jump(fb, jump_end, end_pc);
                } else {
                    let end_pc = fb.ops.len();
                    Self::patch_jump(fb, jump_else, end_pc);
                }
                Ok(())
            }
            Stmt::While { cond, body, span } => {
                let head_pc = fb.ops.len();
                self.compile_expr(fb, cond)?;
                let exit_jump = self.emit(fb, Op::JumpIfFalse(0), *span)?;
                fb.loops.push(LoopPatch { break_jumps: Vec::new(), continue_jumps: Vec::new() });
                fb.push_scope();
                for s in body {
                    self.compile_stmt(fb, s)?;
                }
                fb.pop_scope();
                let patch = fb.loops.pop().expect("loop stack balanced");
                for cj in patch.continue_jumps {
                    Self::patch_jump(fb, cj, head_pc);
                }
                self.emit(fb, Op::Jump(head_pc as u32), *span)?;
                let end_pc = fb.ops.len();
                Self::patch_jump(fb, exit_jump, end_pc);
                for bj in patch.break_jumps {
                    Self::patch_jump(fb, bj, end_pc);
                }
                Ok(())
            }
            Stmt::ForRange { var, start, end, inclusive, body, span } => {
                self.compile_for_range(fb, var, start, end, *inclusive, body, *span)
            }
            Stmt::ForEach { var, iter, body, span } => {
                self.compile_for_each(fb, var, iter, body, *span)
            }
            Stmt::Break { span } => {
                if fb.loops.is_empty() {
                    return Err(CompileDiagnostic::new(
                        DiagnosticStage::Compile,
                        "E_INVALID_CONTROL_FLOW",
                        "`break` used outside of a loop",
                        Some(*span),
                    ));
                }
                let pc = self.emit(fb, Op::Jump(0), *span)?;
                fb.loops.last_mut().unwrap().break_jumps.push(pc);
                Ok(())
            }
            Stmt::Continue { span } => {
                if fb.loops.is_empty() {
                    return Err(CompileDiagnostic::new(
                        DiagnosticStage::Compile,
                        "E_INVALID_CONTROL_FLOW",
                        "`continue` used outside of a loop",
                        Some(*span),
                    ));
                }
                let pc = self.emit(fb, Op::Jump(0), *span)?;
                fb.loops.last_mut().unwrap().continue_jumps.push(pc);
                Ok(())
            }
            Stmt::Return { value, span } => {
                if let Some(expr) = value {
                    self.compile_expr(fb, expr)?;
                } else {
                    self.emit(fb, Op::Null, *span)?;
                }
                self.emit(fb, Op::Return, *span)?;
                Ok(())
            }
            Stmt::Expr { expr, span } => {
                self.compile_expr(fb, expr)?;
                self.emit(fb, Op::Pop, *span)?;
                Ok(())
            }
            Stmt::Block { stmts, .. } => {
                fb.push_scope();
                for s in stmts {
                    self.compile_stmt(fb, s)?;
                }
                fb.pop_scope();
                Ok(())
            }
        }
    }

    fn compile_for_range(
        &mut self,
        fb: &mut FnBuilder,
        var: &str,
        start: &Expr,
        end: &Expr,
        inclusive: bool,
        body: &[Stmt],
        span: Span,
    ) -> Result<(), CompileDiagnostic> {
        fb.push_scope();
        self.compile_expr(fb, start)?;
        let var_slot = fb.declare_local(var.to_string());
        self.emit(fb, Op::SetLocal(var_slot), span)?;

        self.compile_expr(fb, end)?;
        let end_slot = fb.alloc_hidden_local();
        self.emit(fb, Op::SetLocal(end_slot), span)?;

        let head_pc = fb.ops.len();
        self.emit(fb, Op::GetLocal(var_slot), span)?;
        self.emit(fb, Op::GetLocal(end_slot), span)?;
        self.emit(fb, if inclusive { Op::Le } else { Op::Lt }, span)?;
        let exit_jump = self.emit(fb, Op::JumpIfFalse(0), span)?;

        fb.loops.push(LoopPatch { break_jumps: Vec::new(), continue_jumps: Vec::new() });
        fb.push_scope();
        for s in body {
            self.compile_stmt(fb, s)?;
        }
        fb.pop_scope();
        let patch = fb.loops.pop().expect("loop stack balanced");

        // `continue` MUST jump here so `var` advances!
        let step_pc = fb.ops.len();
        for cj in patch.continue_jumps {
            Self::patch_jump(fb, cj, step_pc);
        }
        let one_idx = self.intern_num(1.0, span)?;
        self.emit(fb, Op::GetLocal(var_slot), span)?;
        self.emit(fb, Op::Const(one_idx), span)?;
        self.emit(fb, Op::Add, span)?;
        self.emit(fb, Op::SetLocal(var_slot), span)?;
        self.emit(fb, Op::Jump(head_pc as u32), span)?;

        let end_pc = fb.ops.len();
        Self::patch_jump(fb, exit_jump, end_pc);
        for bj in patch.break_jumps {
            Self::patch_jump(fb, bj, end_pc);
        }
        fb.pop_scope();
        Ok(())
    }

    fn compile_for_each(
        &mut self,
        fb: &mut FnBuilder,
        var: &str,
        iter: &Expr,
        body: &[Stmt],
        span: Span,
    ) -> Result<(), CompileDiagnostic> {
        fb.push_scope();
        self.compile_expr(fb, iter)?;
        let list_slot = fb.alloc_hidden_local();
        self.emit(fb, Op::SetLocal(list_slot), span)?;

        let zero_idx = self.intern_num(0.0, span)?;
        let idx_slot = fb.alloc_hidden_local();
        self.emit(fb, Op::Const(zero_idx), span)?;
        self.emit(fb, Op::SetLocal(idx_slot), span)?;

        let var_slot = fb.declare_local(var.to_string());

        let head_pc = fb.ops.len();
        self.emit(fb, Op::GetLocal(idx_slot), span)?;
        self.emit(fb, Op::GetLocal(list_slot), span)?;
        self.emit(fb, Op::CallBuiltin { builtin: BuiltinId::Len, argc: 1 }, span)?;
        self.emit(fb, Op::Lt, span)?;
        let exit_jump = self.emit(fb, Op::JumpIfFalse(0), span)?;

        self.emit(fb, Op::GetLocal(list_slot), span)?;
        self.emit(fb, Op::GetLocal(idx_slot), span)?;
        self.emit(fb, Op::GetIndex, span)?;
        self.emit(fb, Op::SetLocal(var_slot), span)?;

        fb.loops.push(LoopPatch { break_jumps: Vec::new(), continue_jumps: Vec::new() });
        fb.push_scope();
        for s in body {
            self.compile_stmt(fb, s)?;
        }
        fb.pop_scope();
        let patch = fb.loops.pop().expect("loop stack balanced");

        let step_pc = fb.ops.len();
        for cj in patch.continue_jumps {
            Self::patch_jump(fb, cj, step_pc);
        }
        let one_idx = self.intern_num(1.0, span)?;
        self.emit(fb, Op::GetLocal(idx_slot), span)?;
        self.emit(fb, Op::Const(one_idx), span)?;
        self.emit(fb, Op::Add, span)?;
        self.emit(fb, Op::SetLocal(idx_slot), span)?;
        self.emit(fb, Op::Jump(head_pc as u32), span)?;

        let end_pc = fb.ops.len();
        Self::patch_jump(fb, exit_jump, end_pc);
        for bj in patch.break_jumps {
            Self::patch_jump(fb, bj, end_pc);
        }
        fb.pop_scope();
        Ok(())
    }
}

impl Compiler {
    fn assign_bin_op(op: AssignOp) -> Option<Op> {
        match op {
            AssignOp::Assign => None,
            AssignOp::AddAssign => Some(Op::Add),
            AssignOp::SubAssign => Some(Op::Sub),
            AssignOp::MulAssign => Some(Op::Mul),
            AssignOp::DivAssign => Some(Op::Div),
            AssignOp::ModAssign => Some(Op::Mod),
        }
    }

    fn compile_assign(
        &mut self,
        fb: &mut FnBuilder,
        target: &LValue,
        op: AssignOp,
        value: &Expr,
        span: Span,
    ) -> Result<(), CompileDiagnostic> {
        let bin = Self::assign_bin_op(op);
        match target {
            LValue::Var { name, span: vspan } => {
                if let Some(slot) = fb.resolve_local(name) {
                    if let Some(bop) = bin {
                        self.emit(fb, Op::GetLocal(slot), *vspan)?;
                        self.compile_expr(fb, value)?;
                        self.emit(fb, bop, span)?;
                    } else {
                        self.compile_expr(fb, value)?;
                    }
                    self.emit(fb, Op::SetLocal(slot), span)?;
                    Ok(())
                } else if let Some(&slot) = self.global_map.get(name) {
                    if let Some(bop) = bin {
                        self.emit(fb, Op::GetGlobal(slot), *vspan)?;
                        self.compile_expr(fb, value)?;
                        self.emit(fb, bop, span)?;
                    } else {
                        self.compile_expr(fb, value)?;
                    }
                    self.emit(fb, Op::SetGlobal(slot), span)?;
                    Ok(())
                } else {
                    Err(CompileDiagnostic::new(
                        DiagnosticStage::Compile,
                        "E_UNDEFINED_VAR",
                        format!("assignment to undeclared variable `{name}`"),
                        Some(*vspan),
                    ))
                }
            }
            LValue::Field { base, field, span: fspan } => {
                let name_idx = self.intern_ident(field, *fspan)?;
                self.compile_expr(fb, base)?;
                if let Some(bop) = bin {
                    // Single evaluation of `base`: [obj] -> [obj, obj] -> [obj, old] -> [obj, new]
                    self.emit(fb, Op::Dup, *fspan)?;
                    self.emit(fb, Op::GetField(name_idx), *fspan)?;
                    self.compile_expr(fb, value)?;
                    self.emit(fb, bop, span)?;
                } else {
                    self.compile_expr(fb, value)?;
                }
                self.emit(fb, Op::SetField(name_idx), span)?;
                Ok(())
            }
            LValue::Index { base, index, span: ispan } => {
                self.compile_expr(fb, base)?;
                self.compile_expr(fb, index)?;
                if let Some(bop) = bin {
                    // Single evaluation of `base` & `index`: [arr, idx] -> [arr, idx, arr, idx] -> [arr, idx, old] -> [arr, idx, new]
                    self.emit(fb, Op::Dup2, *ispan)?;
                    self.emit(fb, Op::GetIndex, *ispan)?;
                    self.compile_expr(fb, value)?;
                    self.emit(fb, bop, span)?;
                } else {
                    self.compile_expr(fb, value)?;
                }
                self.emit(fb, Op::SetIndex, span)?;
                Ok(())
            }
        }
    }

    fn compile_expr(&mut self, fb: &mut FnBuilder, expr: &Expr) -> Result<(), CompileDiagnostic> {
        match expr {
            Expr::Null(span) => { self.emit(fb, Op::Null, *span)?; Ok(()) }
            Expr::Bool(true, span) => { self.emit(fb, Op::True, *span)?; Ok(()) }
            Expr::Bool(false, span) => { self.emit(fb, Op::False, *span)?; Ok(()) }
            Expr::Num(n, span) => {
                let idx = self.intern_num(*n, *span)?;
                self.emit(fb, Op::Const(idx), *span)?;
                Ok(())
            }
            Expr::Str(s, span) => {
                let idx = self.intern_str_const(Arc::clone(s), *span)?;
                self.emit(fb, Op::Const(idx), *span)?;
                Ok(())
            }
            Expr::Ident(name, span) => {
                if let Some(slot) = fb.resolve_local(name) {
                    self.emit(fb, Op::GetLocal(slot), *span)?;
                    return Ok(());
                }
                if let Some(&slot) = self.global_map.get(name) {
                    self.emit(fb, Op::GetGlobal(slot), *span)?;
                    return Ok(());
                }
                if name == "PI" {
                    let idx = self.intern_num(PI, *span)?;
                    self.emit(fb, Op::Const(idx), *span)?;
                    return Ok(());
                }
                if name == "TAU" {
                    let idx = self.intern_num(TAU, *span)?;
                    self.emit(fb, Op::Const(idx), *span)?;
                    return Ok(());
                }
                Err(CompileDiagnostic::new(
                    DiagnosticStage::Compile,
                    "E_UNDEFINED_VAR",
                    format!("undefined variable `{name}`"),
                    Some(*span),
                ))
            }
            Expr::List(items, span) => {
                if items.len() > self.budgets.max_list_len {
                    return Err(CompileDiagnostic::new(
                        DiagnosticStage::Compile,
                        "E_LIST_TOO_LONG",
                        format!("list literal exceeds max length {}", self.budgets.max_list_len),
                        Some(*span),
                    ));
                }
                for item in items {
                    self.compile_expr(fb, item)?;
                }
                self.emit(fb, Op::MakeList(items.len() as u16), *span)?;
                Ok(())
            }
            Expr::Map(pairs, span) => {
                if pairs.len() > self.budgets.max_map_keys {
                    return Err(CompileDiagnostic::new(
                        DiagnosticStage::Compile,
                        "E_MAP_TOO_LARGE",
                        format!("map literal exceeds max keys {}", self.budgets.max_map_keys),
                        Some(*span),
                    ));
                }
                for (k, v) in pairs {
                    if k.len() > self.budgets.max_str_bytes {
                        return Err(CompileDiagnostic::new(
                            DiagnosticStage::Compile,
                            "E_STR_TOO_LONG",
                            format!(
                                "map literal key exceeds max length {} bytes",
                                self.budgets.max_str_bytes
                            ),
                            Some(*span),
                        ));
                    }
                    let k_idx = self.intern_str_const(Arc::from(k.as_str()), *span)?;
                    self.emit(fb, Op::Const(k_idx), *span)?;
                    self.compile_expr(fb, v)?;
                }
                self.emit(fb, Op::MakeMap(pairs.len() as u16), *span)?;
                Ok(())
            }
            Expr::Unary { op, operand, span } => {
                self.compile_expr(fb, operand)?;
                let bytecode = match op {
                    UnaryOp::Neg => Op::Neg,
                    UnaryOp::Not => Op::Not,
                };
                self.emit(fb, bytecode, *span)?;
                Ok(())
            }
            Expr::Binary { op: BinOp::And, left, right, span } => {
                self.compile_expr(fb, left)?;
                let j = self.emit(fb, Op::JumpIfFalsePeek(0), *span)?;
                self.emit(fb, Op::Pop, *span)?;
                self.compile_expr(fb, right)?;
                let end = fb.ops.len();
                Self::patch_jump(fb, j, end);
                Ok(())
            }
            Expr::Binary { op: BinOp::Or, left, right, span } => {
                self.compile_expr(fb, left)?;
                let j = self.emit(fb, Op::JumpIfTruePeek(0), *span)?;
                self.emit(fb, Op::Pop, *span)?;
                self.compile_expr(fb, right)?;
                let end = fb.ops.len();
                Self::patch_jump(fb, j, end);
                Ok(())
            }
            Expr::Binary { op, left, right, span } => {
                self.compile_expr(fb, left)?;
                self.compile_expr(fb, right)?;
                let bytecode = match op {
                    BinOp::Add => Op::Add, BinOp::Sub => Op::Sub, BinOp::Mul => Op::Mul,
                    BinOp::Div => Op::Div, BinOp::Mod => Op::Mod, BinOp::Eq => Op::Eq,
                    BinOp::Ne => Op::Ne, BinOp::Lt => Op::Lt, BinOp::Le => Op::Le,
                    BinOp::Gt => Op::Gt, BinOp::Ge => Op::Ge,
                    BinOp::And | BinOp::Or => unreachable!(),
                };
                self.emit(fb, bytecode, *span)?;
                Ok(())
            }
            Expr::Call { callee, args, span } => {
                if args.len() > 32 {
                    return Err(CompileDiagnostic::new(
                        DiagnosticStage::Compile,
                        "E_TOO_MANY_ARGS",
                        format!("call to `{callee}` exceeds 32 arguments"),
                        Some(*span),
                    ));
                }
                for a in args {
                    self.compile_expr(fb, a)?;
                }
                if let Some(&fn_idx) = self.fn_table.get(callee) {
                    let expected = self.fn_arities[fn_idx as usize] as usize;
                    if args.len() != expected {
                        return Err(CompileDiagnostic::new(
                            DiagnosticStage::Compile,
                            "E_ARITY_MISMATCH",
                            format!("function `{callee}` expects {expected} args, got {}", args.len()),
                            Some(*span),
                        ));
                    }
                    self.emit(fb, Op::CallFn { fn_idx, argc: args.len() as u8 }, *span)?;
                    Ok(())
                } else if let Some(builtin) = BuiltinId::from_name(callee) {
                    self.feature_tokens.insert(builtin.canonical_name().to_string());
                    self.emit(fb, Op::CallBuiltin { builtin, argc: args.len() as u8 }, *span)?;
                    Ok(())
                } else {
                    Err(CompileDiagnostic::new(
                        DiagnosticStage::Compile,
                        "E_UNDEFINED_FN",
                        format!("undefined function `{callee}`"),
                        Some(*span),
                    ))
                }
            }
            Expr::MethodCall { receiver, method, args, span } => {
                if args.len() > 32 {
                    return Err(CompileDiagnostic::new(
                        DiagnosticStage::Compile,
                        "E_TOO_MANY_ARGS",
                        format!("method call `.{method}()` exceeds 32 arguments"),
                        Some(*span),
                    ));
                }
                if let Some(b) = BuiltinId::from_name(method) {
                    self.feature_tokens.insert(b.canonical_name().to_string());
                }
                self.compile_expr(fb, receiver)?;
                for a in args {
                    self.compile_expr(fb, a)?;
                }
                let name_idx = self.intern_ident(method, *span)?;
                self.emit(fb, Op::CallMethod { name_idx, argc: args.len() as u8 }, *span)?;
                Ok(())
            }
            Expr::Field { base, field, span } => {
                self.compile_expr(fb, base)?;
                let name_idx = self.intern_ident(field, *span)?;
                self.emit(fb, Op::GetField(name_idx), *span)?;
                Ok(())
            }
            Expr::Index { base, index, span } => {
                self.compile_expr(fb, base)?;
                self.compile_expr(fb, index)?;
                self.emit(fb, Op::GetIndex, *span)?;
                Ok(())
            }
        }
    }
}
