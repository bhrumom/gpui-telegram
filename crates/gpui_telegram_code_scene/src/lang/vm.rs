//! Sandboxed bytecode Virtual Machine for `CodeSceneScript` (`cs1`).
//!
//! Enforces per-op instruction limits, 1,024-op wall-clock watchdogs, call stack
//! depth caps, automatic iterative mark-and-sweep GC before heap exhaustion,
//! and O(N) cycle-safe `VmSnapshot` checkpointing.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::lang::compiler::{CompiledProgram, ConstVal, FunctionChunk, Op};
use crate::lang::heap::{HeapId, SplitMix64, Val, VmHeap, VmSnapshot};
use crate::lang::limits::VmBudgets;
use crate::lang::shaders::ShaderWatchdog;
use crate::lang::stdlib::{BuiltinCtx, BuiltinId, VmHost, dispatch_builtin, dispatch_method};
use crate::model::{RuntimeError, SceneInputState, sanitize_f64};

/// Per-invocation execution quota (standard frame vs seek replay).
#[derive(Debug, Clone, Copy)]
pub struct ExecQuota {
    pub op_limit: u64,
    pub wall_limit: Duration,
}

impl ExecQuota {
    pub fn for_frame(budgets: &VmBudgets) -> Self {
        Self {
            op_limit: budgets.max_ops_per_frame,
            wall_limit: Duration::from_millis(budgets.max_frame_wall_ms.max(1)),
        }
    }

    pub fn for_seek(budgets: &VmBudgets) -> Self {
        Self {
            op_limit: budgets.max_seek_ops,
            wall_limit: Duration::from_millis(budgets.max_seek_wall_ms.max(1)),
        }
    }
}

#[derive(Debug, Clone)]
struct CallFrame {
    fn_idx: Option<u16>, // None => init_chunk
    pc: usize,
    locals: Vec<Val>,
}

/// Complete mutable VM state (100% `Send + Sync`, zero `Rc<RefCell>`).
#[derive(Debug, Clone)]
pub struct VmState {
    program: Arc<CompiledProgram>,
    budgets: VmBudgets,
    heap: VmHeap,
    globals: Vec<Val>,
    rng: SplitMix64,
    state_id: HeapId,
    input_id: HeapId,
    used_builtins_mask: u128,
    audited_live_str_bytes: usize,
    str_alloc_bytes_since_audit: usize,
    const_str_bytes: usize,
}

impl VmState {
    fn make_initial_input_map() -> BTreeMap<String, Val> {
        let mut map = BTreeMap::new();
        map.insert("left".to_string(), Val::Bool(false));
        map.insert("right".to_string(), Val::Bool(false));
        map.insert("up".to_string(), Val::Bool(false));
        map.insert("down".to_string(), Val::Bool(false));
        map.insert("action".to_string(), Val::Bool(false));
        map.insert("mouse_x".to_string(), Val::Num(0.0));
        map.insert("mouse_y".to_string(), Val::Num(0.0));
        map.insert("mouse_down".to_string(), Val::Bool(false));
        map.insert("clicked".to_string(), Val::Bool(false));
        map.insert("pointer_active".to_string(), Val::Bool(false));
        map
    }

    pub fn new(
        program: Arc<CompiledProgram>,
        budgets: VmBudgets,
        seed: u64,
    ) -> Result<Self, RuntimeError> {
        let mut heap = VmHeap::new();
        let state_id = heap.alloc_map(BTreeMap::new(), &budgets)?;
        let input_id = heap.alloc_map(Self::make_initial_input_map(), &budgets)?;
        let globals = vec![Val::Null; program.globals.len()];
        let const_str_bytes = program
            .constants
            .iter()
            .map(|c| match c {
                ConstVal::Str(s) => s.len(),
                ConstVal::Num(_) => 0,
            })
            .fold(0usize, usize::saturating_add)
            .saturating_add(
                program
                    .strings
                    .iter()
                    .map(|s| s.len())
                    .fold(0usize, usize::saturating_add),
            );
        let audited_live_str_bytes = heap.exact_live_str_bytes(&[&globals]).0;
        Ok(Self {
            program,
            budgets,
            heap,
            globals,
            rng: SplitMix64::new(seed),
            state_id,
            input_id,
            used_builtins_mask: 0,
            audited_live_str_bytes,
            str_alloc_bytes_since_audit: 0,
            const_str_bytes,
        })
    }

    pub fn program(&self) -> &CompiledProgram {
        &self.program
    }

    pub fn budgets(&self) -> &VmBudgets {
        &self.budgets
    }

    pub fn heap(&self) -> &VmHeap {
        &self.heap
    }

    pub fn state_id(&self) -> HeapId {
        self.state_id
    }

    pub fn state_val(&self) -> Val {
        Val::Map(self.state_id)
    }

    pub fn input_val(&self) -> Val {
        Val::Map(self.input_id)
    }

    pub fn update_input_state(&mut self, input: &SceneInputState) {
        let added_key_bytes = self.heap.update_input_map_tracked(self.input_id, input);
        self.str_alloc_bytes_since_audit = self
            .str_alloc_bytes_since_audit
            .saturating_add(added_key_bytes);
    }

    pub fn collect_garbage_idle(&mut self) {
        self.heap
            .collect_garbage(&[&self.globals], self.state_id, self.input_id);
        self.audited_live_str_bytes = self.heap.exact_live_str_bytes(&[&self.globals]).0;
        self.str_alloc_bytes_since_audit = 0;
    }

    pub fn used_builtins(&self) -> Vec<String> {
        let mut out = Vec::new();
        for b in BuiltinId::ALL {
            if (self.used_builtins_mask & (1u128 << (b as u8))) != 0 {
                out.push(b.canonical_name().to_string());
            }
        }
        out
    }

    pub fn alloc_map(&mut self, entries: BTreeMap<String, Val>) -> Result<HeapId, RuntimeError> {
        let added_bytes = entries
            .iter()
            .map(|(k, v)| k.len().saturating_add(v.str_bytes()))
            .fold(0usize, usize::saturating_add);
        let id = match self.heap.alloc_map(entries.clone(), &self.budgets) {
            Ok(id) => id,
            Err(RuntimeError::MemoryBudgetExceeded { .. }) => {
                let extra: Vec<Val> = entries.values().cloned().collect();
                self.heap
                    .collect_garbage(&[&self.globals, &extra], self.state_id, self.input_id);
                self.audited_live_str_bytes =
                    self.heap.exact_live_str_bytes(&[&self.globals, &extra]).0;
                self.str_alloc_bytes_since_audit = 0;
                self.heap.alloc_map(entries, &self.budgets)?
            }
            Err(e) => return Err(e),
        };
        self.str_alloc_bytes_since_audit = self
            .str_alloc_bytes_since_audit
            .saturating_add(added_bytes);
        if let Err(err) = self.check_host_aggregate_str_budget(&[Val::Map(id)]) {
            self.collect_garbage_idle();
            return Err(err);
        }
        Ok(id)
    }

    pub fn set_state_field(&mut self, key: &str, val: Val) -> Result<(), RuntimeError> {
        let val_str_bytes = val.str_bytes();
        let added_key_bytes = match self.heap.map_set_tracked(
            self.state_id,
            key.to_string(),
            val.clone(),
            &self.budgets,
        ) {
            Ok(kbytes) => kbytes,
            Err(RuntimeError::MemoryBudgetExceeded { .. }) => {
                let extra = [val.clone()];
                self.heap
                    .collect_garbage(&[&self.globals, &extra], self.state_id, self.input_id);
                self.audited_live_str_bytes =
                    self.heap.exact_live_str_bytes(&[&self.globals, &extra]).0;
                self.str_alloc_bytes_since_audit = 0;
                self.heap
                    .map_set_tracked(self.state_id, key.to_string(), val, &self.budgets)?
            }
            Err(e) => return Err(e),
        };
        self.str_alloc_bytes_since_audit = self
            .str_alloc_bytes_since_audit
            .saturating_add(added_key_bytes)
            .saturating_add(val_str_bytes);
        self.check_host_aggregate_str_budget(&[])
    }

    pub fn get_state_field(&self, key: &str) -> Val {
        self.heap.map_get(self.state_id, key)
    }

    pub fn snapshot(&self) -> VmSnapshot {
        VmSnapshot {
            heap: self.heap.clone(),
            globals: self.globals.clone(),
            rng: self.rng,
            state_id: self.state_id,
            input_id: self.input_id,
        }
    }

    pub fn restore(&mut self, snap: &VmSnapshot) {
        self.heap = snap.heap.clone();
        self.globals = snap.globals.clone();
        self.rng = snap.rng;
        self.state_id = snap.state_id;
        self.input_id = snap.input_id;
        self.audited_live_str_bytes = self.heap.exact_live_str_bytes(&[&self.globals]).0;
        self.str_alloc_bytes_since_audit = 0;
    }

    /// Runs top-level `let` initializers (if any).
    pub fn run_global_inits(
        &mut self,
        host: &mut dyn VmHost,
        ops_used: &mut u64,
        started_at: Instant,
        quota: ExecQuota,
    ) -> Result<(), RuntimeError> {
        if self.program.init_chunk.is_some() {
            let local_count = self
                .program
                .init_chunk
                .as_ref()
                .map_or(0, |c| c.local_count as usize);
            let frame = CallFrame {
                fn_idx: None,
                pc: 0,
                locals: vec![Val::Null; local_count],
            };
            self.run_frame(frame, host, ops_used, started_at, quota)?;
        }
        Ok(())
    }

    /// Calls a compiled function by name, padding missing args with `Null` or truncating extras
    /// for lifecycle hooks (`on_init`/`on_update`/`on_render`).
    pub fn call_by_name(
        &mut self,
        name: &str,
        args: &[Val],
        host: &mut dyn VmHost,
        ops_used: &mut u64,
        started_at: Instant,
        quota: ExecQuota,
    ) -> Result<Val, RuntimeError> {
        let Some(&fn_idx) = self.program.fn_table.get(name) else {
            return Err(RuntimeError::UndefinedFunction {
                name: name.to_string(),
            });
        };
        let chunk = &self.program.functions[fn_idx as usize];
        let mut locals = vec![Val::Null; chunk.local_count.max(chunk.arity as u16) as usize];
        for (i, v) in args.iter().take(chunk.arity as usize).enumerate() {
            locals[i] = v.clone();
        }
        let arg_str_bytes = args
            .iter()
            .take(chunk.arity as usize)
            .map(Val::str_bytes)
            .fold(0usize, usize::saturating_add);
        self.str_alloc_bytes_since_audit = self
            .str_alloc_bytes_since_audit
            .saturating_add(arg_str_bytes);
        let frame = CallFrame {
            fn_idx: Some(fn_idx),
            pc: 0,
            locals,
        };
        self.run_frame(frame, host, ops_used, started_at, quota)
    }

    fn maybe_gc(
        &mut self,
        stack: &[Val],
        frames: &[CallFrame],
        ops_used: &mut u64,
        started_at: Instant,
        quota: ExecQuota,
    ) -> Result<(), RuntimeError> {
        let gc_cost = ((self.heap.slot_count() / 8).max(16)) as u64;
        *ops_used = ops_used.saturating_add(gc_cost);
        if *ops_used > quota.op_limit {
            return Err(RuntimeError::InstructionBudgetExceeded {
                used: *ops_used,
                limit: quota.op_limit,
            });
        }
        let elapsed = started_at.elapsed();
        if elapsed >= quota.wall_limit {
            return Err(RuntimeError::FrameTimeout {
                elapsed_ms: elapsed.as_millis() as u64,
                limit_ms: quota.wall_limit.as_millis() as u64,
            });
        }
        let mut roots: Vec<&[Val]> = Vec::with_capacity(2 + frames.len());
        roots.push(&self.globals);
        roots.push(stack);
        for f in frames {
            roots.push(&f.locals);
        }
        self.heap.collect_garbage(&roots, self.state_id, self.input_id);
        Ok(())
    }

    fn check_host_aggregate_str_budget(
        &mut self,
        extra_roots: &[Val],
    ) -> Result<(), RuntimeError> {
        let upper = self
            .audited_live_str_bytes
            .saturating_add(self.str_alloc_bytes_since_audit)
            .saturating_add(self.const_str_bytes);
        if upper <= self.budgets.max_total_str_bytes {
            return Ok(());
        }
        self.heap.collect_garbage(
            &[&self.globals, extra_roots],
            self.state_id,
            self.input_id,
        );
        let (total_live, _) = self
            .heap
            .exact_live_str_bytes(&[&self.globals, extra_roots]);
        self.audited_live_str_bytes = total_live;
        self.str_alloc_bytes_since_audit = 0;
        if total_live > self.budgets.max_total_str_bytes {
            return Err(RuntimeError::MemoryBudgetExceeded {
                detail: format!(
                    "aggregate live string memory {total_live} bytes exceeds max {}",
                    self.budgets.max_total_str_bytes
                ),
            });
        }
        Ok(())
    }

    fn record_str_alloc_and_check(
        &mut self,
        added_bytes: usize,
        stack: &[Val],
        frames: &[CallFrame],
        ops_used: &mut u64,
        started_at: Instant,
        quota: ExecQuota,
    ) -> Result<(), RuntimeError> {
        self.str_alloc_bytes_since_audit = self
            .str_alloc_bytes_since_audit
            .saturating_add(added_bytes);
        self.check_aggregate_str_budget(stack, frames, ops_used, started_at, quota)
    }

    fn check_aggregate_str_budget(
        &mut self,
        stack: &[Val],
        frames: &[CallFrame],
        ops_used: &mut u64,
        started_at: Instant,
        quota: ExecQuota,
    ) -> Result<(), RuntimeError> {
        let upper = self
            .audited_live_str_bytes
            .saturating_add(self.str_alloc_bytes_since_audit)
            .saturating_add(self.const_str_bytes);
        if upper <= self.budgets.max_total_str_bytes {
            return Ok(());
        }
        self.maybe_gc(stack, frames, ops_used, started_at, quota)?;
        let mut root_slices: Vec<&[Val]> = Vec::with_capacity(2 + frames.len());
        root_slices.push(&self.globals);
        root_slices.push(stack);
        for f in frames {
            root_slices.push(&f.locals);
        }
        let (total_live, _) = self.heap.exact_live_str_bytes(&root_slices);
        self.audited_live_str_bytes = total_live;
        self.str_alloc_bytes_since_audit = 0;
        if total_live > self.budgets.max_total_str_bytes {
            return Err(RuntimeError::MemoryBudgetExceeded {
                detail: format!(
                    "aggregate live string memory {total_live} bytes exceeds max {}",
                    self.budgets.max_total_str_bytes
                ),
            });
        }
        Ok(())
    }
}

impl VmState {
    fn chunk_of(&self, fn_idx: Option<u16>) -> &FunctionChunk {
        match fn_idx {
            Some(idx) => &self.program.functions[idx as usize],
            None => self
                .program
                .init_chunk
                .as_ref()
                .expect("init_chunk present"),
        }
    }

    fn run_frame(
        &mut self,
        initial_frame: CallFrame,
        host: &mut dyn VmHost,
        ops_used: &mut u64,
        started_at: Instant,
        quota: ExecQuota,
    ) -> Result<Val, RuntimeError> {
        let mut frames = Vec::with_capacity(16);
        frames.push(initial_frame);
        let mut stack: Vec<Val> = Vec::with_capacity(64);
        let budgets = self.budgets;

        loop {
            let Some(frame) = frames.last_mut() else {
                return Ok(stack.pop().unwrap_or(Val::Null));
            };
            let op = {
                let chunk = match frame.fn_idx {
                    Some(idx) => &self.program.functions[idx as usize],
                    None => self.program.init_chunk.as_ref().expect("init_chunk"),
                };
                if frame.pc >= chunk.ops.len() {
                    Op::Return
                } else {
                    let op = chunk.ops[frame.pc];
                    frame.pc += 1;
                    op
                }
            };

            *ops_used = ops_used.saturating_add(1);
            if *ops_used > quota.op_limit {
                return Err(RuntimeError::InstructionBudgetExceeded {
                    used: *ops_used,
                    limit: quota.op_limit,
                });
            }
            if (*ops_used & 0x3FF) == 0 {
                let elapsed = started_at.elapsed();
                if elapsed >= quota.wall_limit {
                    return Err(RuntimeError::FrameTimeout {
                        elapsed_ms: elapsed.as_millis() as u64,
                        limit_ms: quota.wall_limit.as_millis() as u64,
                    });
                }
            }

            if stack.len() > budgets.max_stack_values {
                return Err(RuntimeError::MemoryBudgetExceeded {
                    detail: format!("operand stack exceeded max {}", budgets.max_stack_values),
                });
            }

            match op {
                Op::Const(idx) => {
                    let val = match &self.program.constants[idx as usize] {
                        ConstVal::Num(n) => Val::Num(*n),
                        ConstVal::Str(s) => Val::Str(Arc::clone(s)),
                    };
                    stack.push(val);
                }
                Op::Null => stack.push(Val::Null),
                Op::True => stack.push(Val::Bool(true)),
                Op::False => stack.push(Val::Bool(false)),
                Op::Pop => { stack.pop(); }
                Op::Dup => {
                    if let Some(v) = stack.last().cloned() {
                        stack.push(v);
                    }
                }
                Op::Dup2 => {
                    if stack.len() >= 2 {
                        let a = stack[stack.len() - 2].clone();
                        let b = stack[stack.len() - 1].clone();
                        stack.push(a);
                        stack.push(b);
                    }
                }
                Op::GetLocal(slot) => {
                    let v = frames
                        .last()
                        .and_then(|f| f.locals.get(slot as usize))
                        .cloned()
                        .unwrap_or(Val::Null);
                    stack.push(v);
                }
                Op::SetLocal(slot) => {
                    let val = stack.pop().unwrap_or(Val::Null);
                    if let Some(f) = frames.last_mut() {
                        if let Some(target) = f.locals.get_mut(slot as usize) {
                            *target = val;
                        }
                    }
                }
                Op::GetGlobal(slot) => {
                    let v = self.globals.get(slot as usize).cloned().unwrap_or(Val::Null);
                    stack.push(v);
                }
                Op::SetGlobal(slot) => {
                    let val = stack.pop().unwrap_or(Val::Null);
                    if let Some(target) = self.globals.get_mut(slot as usize) {
                        *target = val;
                    }
                }
                Op::MakeList(count) => {
                    let n = count as usize;
                    let start = stack.len().saturating_sub(n);
                    let id = match self.heap.alloc_list(stack[start..].to_vec(), &budgets) {
                        Ok(id) => id,
                        Err(RuntimeError::MemoryBudgetExceeded { .. }) => {
                            self.maybe_gc(&stack, &frames, ops_used, started_at, quota)?;
                            self.heap.alloc_list(stack[start..].to_vec(), &budgets)?
                        }
                        Err(e) => return Err(e),
                    };
                    stack.truncate(start);
                    stack.push(Val::List(id));
                }
                Op::MakeMap(pairs) => {
                    let n = (pairs as usize) * 2;
                    let start = stack.len().saturating_sub(n);
                    let mut map = BTreeMap::new();
                    let mut i = start;
                    while i + 1 < stack.len() {
                        let key = match &stack[i] {
                            Val::Str(s) => s.to_string(),
                            other => self.heap.format_val(other, budgets.max_str_bytes)?,
                        };
                        map.insert(key, stack[i + 1].clone());
                        i += 2;
                    }
                    let map_key_bytes = map
                        .keys()
                        .map(String::len)
                        .fold(0usize, usize::saturating_add);
                    let id = match self.heap.alloc_map(map.clone(), &budgets) {
                        Ok(id) => id,
                        Err(RuntimeError::MemoryBudgetExceeded { .. }) => {
                            self.maybe_gc(&stack, &frames, ops_used, started_at, quota)?;
                            self.heap.alloc_map(map, &budgets)?
                        }
                        Err(e) => return Err(e),
                    };
                    stack.truncate(start);
                    stack.push(Val::Map(id));
                    self.record_str_alloc_and_check(
                        map_key_bytes,
                        &stack,
                        &frames,
                        ops_used,
                        started_at,
                        quota,
                    )?;
                }
                Op::GetField(name_idx) => {
                    let base = stack.pop().unwrap_or(Val::Null);
                    let field = self.program.strings[name_idx as usize].as_ref();
                    let val = match base {
                        Val::Map(id) => self.heap.map_get(id, field),
                        Val::Canvas => match field {
                            "width" | "w" => Val::Num(host.canvas_size().0 as f64),
                            "height" | "h" => Val::Num(host.canvas_size().1 as f64),
                            "fps" => Val::Num(host.canvas_fps() as f64),
                            "duration" => host.canvas_duration().map_or(Val::Null, Val::Num),
                            _ => Val::Null,
                        },
                        _ => Val::Null,
                    };
                    stack.push(val);
                }
                Op::SetField(name_idx) => {
                    let len = stack.len();
                    let val = if len >= 1 { stack[len - 1].clone() } else { Val::Null };
                    let base = if len >= 2 { stack[len - 2].clone() } else { Val::Null };
                    let field = self.program.strings[name_idx as usize].to_string();
                    let added_key_bytes = match base {
                        Val::Map(id) => {
                            match self.heap.map_set_tracked(id, field.clone(), val.clone(), &budgets) {
                                Ok(kbytes) => kbytes,
                                Err(RuntimeError::MemoryBudgetExceeded { .. }) => {
                                    self.maybe_gc(&stack, &frames, ops_used, started_at, quota)?;
                                    self.heap.map_set_tracked(id, field, val, &budgets)?
                                }
                                Err(e) => return Err(e),
                            }
                        }
                        _ => {
                            return Err(RuntimeError::TypeError {
                                detail: format!("cannot set field `.{field}` on non-map value"),
                            });
                        }
                    };
                    stack.truncate(len.saturating_sub(2));
                    self.record_str_alloc_and_check(
                        added_key_bytes,
                        &stack,
                        &frames,
                        ops_used,
                        started_at,
                        quota,
                    )?;
                }
                _ => {
                    if let Some(ret) = self.exec_op_ext(
                        op,
                        &mut stack,
                        &mut frames,
                        host,
                        ops_used,
                        started_at,
                        quota,
                    )? {
                        return Ok(ret);
                    }
                }
            }
        }
    }
}

impl VmState {
    #[allow(clippy::too_many_arguments)]
    fn exec_op_ext(
        &mut self,
        op: Op,
        stack: &mut Vec<Val>,
        frames: &mut Vec<CallFrame>,
        host: &mut dyn VmHost,
        ops_used: &mut u64,
        started_at: Instant,
        quota: ExecQuota,
    ) -> Result<Option<Val>, RuntimeError> {
        let budgets = self.budgets;
        match op {
            Op::GetIndex => {
                let idx = stack.pop().unwrap_or(Val::Null);
                let base = stack.pop().unwrap_or(Val::Null);
                let val = match base {
                    Val::List(id) => self.heap.list_get(id, idx.as_num()),
                    Val::Map(id) => {
                        let key = match idx {
                            Val::Str(s) => s.to_string(),
                            other => self.heap.format_val(&other, budgets.max_str_bytes)?,
                        };
                        self.heap.map_get(id, &key)
                    }
                    _ => Val::Null,
                };
                stack.push(val);
            }
            Op::SetIndex => {
                let len = stack.len();
                let val = if len >= 1 { stack[len - 1].clone() } else { Val::Null };
                let idx = if len >= 2 { stack[len - 2].clone() } else { Val::Null };
                let base = if len >= 3 { stack[len - 3].clone() } else { Val::Null };
                let added_key_bytes = match base {
                    Val::List(id) => {
                        let idx_num = idx.as_num();
                        match self.heap.list_set(id, idx_num, val.clone(), &budgets) {
                            Ok(()) => 0,
                            Err(RuntimeError::MemoryBudgetExceeded { .. }) => {
                                self.maybe_gc(stack, frames, ops_used, started_at, quota)?;
                                self.heap.list_set(id, idx_num, val, &budgets)?;
                                0
                            }
                            Err(e) => return Err(e),
                        }
                    }
                    Val::Map(id) => {
                        let key = match &idx {
                            Val::Str(s) => s.to_string(),
                            other => self.heap.format_val(other, budgets.max_str_bytes)?,
                        };
                        match self.heap.map_set_tracked(id, key.clone(), val.clone(), &budgets) {
                            Ok(kbytes) => kbytes,
                            Err(RuntimeError::MemoryBudgetExceeded { .. }) => {
                                self.maybe_gc(stack, frames, ops_used, started_at, quota)?;
                                self.heap.map_set_tracked(id, key, val, &budgets)?
                            }
                            Err(e) => return Err(e),
                        }
                    }
                    _ => {
                        return Err(RuntimeError::TypeError {
                            detail: "cannot index-assign non-container value".into(),
                        });
                    }
                };
                stack.truncate(len.saturating_sub(3));
                self.record_str_alloc_and_check(
                    added_key_bytes,
                    stack,
                    frames,
                    ops_used,
                    started_at,
                    quota,
                )?;
            }
            Op::Add => {
                let b = stack.pop().unwrap_or(Val::Null);
                let a = stack.pop().unwrap_or(Val::Null);
                let (res, is_str_concat, added_str_bytes) =
                    if matches!(a, Val::Str(_)) || matches!(b, Val::Str(_)) {
                        let sa = self.heap.format_val(&a, budgets.max_str_bytes)?;
                        let sb = self.heap.format_val(&b, budgets.max_str_bytes)?;
                        if sa.len().saturating_add(sb.len()) > budgets.max_str_bytes {
                            return Err(RuntimeError::MemoryBudgetExceeded {
                                detail: format!(
                                    "string concatenation exceeds max {} bytes",
                                    budgets.max_str_bytes
                                ),
                            });
                        }
                        let mut combined = String::with_capacity(sa.len() + sb.len());
                        combined.push_str(&sa);
                        combined.push_str(&sb);
                        let len = combined.len();
                        (Val::Str(Arc::from(combined)), true, len)
                    } else {
                        (Val::Num(a.as_num() + b.as_num()), false, 0)
                    };
                stack.push(res);
                if is_str_concat {
                    self.record_str_alloc_and_check(
                        added_str_bytes,
                        stack,
                        frames,
                        ops_used,
                        started_at,
                        quota,
                    )?;
                }
            }
            Op::Sub => {
                let b = stack.pop().unwrap_or(Val::Null);
                let a = stack.pop().unwrap_or(Val::Null);
                stack.push(Val::Num(a.as_num() - b.as_num()));
            }
            Op::Mul => {
                let b = stack.pop().unwrap_or(Val::Null);
                let a = stack.pop().unwrap_or(Val::Null);
                stack.push(Val::Num(a.as_num() * b.as_num()));
            }
            Op::Div => {
                let b = stack.pop().unwrap_or(Val::Null).as_num();
                let a = stack.pop().unwrap_or(Val::Null).as_num();
                let out = if b == 0.0 || !b.is_finite() { 0.0 } else { sanitize_f64(a / b) };
                stack.push(Val::Num(out));
            }
            Op::Mod => {
                let b = stack.pop().unwrap_or(Val::Null).as_num();
                let a = stack.pop().unwrap_or(Val::Null).as_num();
                let out = if b == 0.0 || !b.is_finite() { 0.0 } else { sanitize_f64(a % b) };
                stack.push(Val::Num(out));
            }
            Op::Neg => {
                let a = stack.pop().unwrap_or(Val::Null).as_num();
                stack.push(Val::Num(-a));
            }
            Op::Not => {
                let a = stack.pop().unwrap_or(Val::Null);
                stack.push(Val::Bool(!a.is_truthy()));
            }
            Op::Eq => {
                let b = stack.pop().unwrap_or(Val::Null);
                let a = stack.pop().unwrap_or(Val::Null);
                stack.push(Val::Bool(self.heap.val_eq(&a, &b)));
            }
            Op::Ne => {
                let b = stack.pop().unwrap_or(Val::Null);
                let a = stack.pop().unwrap_or(Val::Null);
                stack.push(Val::Bool(!self.heap.val_eq(&a, &b)));
            }
            Op::Lt | Op::Le | Op::Gt | Op::Ge => {
                let b = stack.pop().unwrap_or(Val::Null).as_num();
                let a = stack.pop().unwrap_or(Val::Null).as_num();
                let cmp = match op {
                    Op::Lt => a < b,
                    Op::Le => a <= b,
                    Op::Gt => a > b,
                    Op::Ge => a >= b,
                    _ => false,
                };
                stack.push(Val::Bool(cmp));
            }
            Op::Jump(target) => {
                if let Some(f) = frames.last_mut() {
                    f.pc = target as usize;
                }
            }
            Op::JumpIfFalse(target) => {
                let cond = stack.pop().unwrap_or(Val::Null);
                if !cond.is_truthy() {
                    if let Some(f) = frames.last_mut() {
                        f.pc = target as usize;
                    }
                }
            }
            Op::JumpIfFalsePeek(target) => {
                let cond = stack.last().map_or(false, Val::is_truthy);
                if !cond {
                    if let Some(f) = frames.last_mut() {
                        f.pc = target as usize;
                    }
                }
            }
            Op::JumpIfTruePeek(target) => {
                let cond = stack.last().map_or(false, Val::is_truthy);
                if cond {
                    if let Some(f) = frames.last_mut() {
                        f.pc = target as usize;
                    }
                }
            }
            Op::CallFn { fn_idx, argc } => {
                if frames.len() >= budgets.max_call_depth {
                    return Err(RuntimeError::StackOverflow {
                        max_depth: budgets.max_call_depth,
                    });
                }
                let n = argc as usize;
                let start = stack.len().saturating_sub(n);
                let chunk = self.chunk_of(Some(fn_idx));
                let mut locals = vec![Val::Null; chunk.local_count.max(chunk.arity as u16) as usize];
                let copy_n = n.min(chunk.arity as usize);
                for i in 0..copy_n {
                    locals[i] = stack[start + i].clone();
                }
                stack.truncate(start);
                frames.push(CallFrame {
                    fn_idx: Some(fn_idx),
                    pc: 0,
                    locals,
                });
            }
            Op::CallBuiltin { builtin, argc } => {
                let n = argc as usize;
                let start = stack.len().saturating_sub(n);
                self.used_builtins_mask |= 1u128 << (builtin as u8);
                let res = self.invoke_builtin_with_gc(
                    builtin,
                    start,
                    stack,
                    frames,
                    host,
                    ops_used,
                    started_at,
                    quota,
                )?;
                let added_str_bytes = match (builtin, &res) {
                    (BuiltinId::Str, _) => res.str_bytes(),
                    (BuiltinId::Keys, Val::List(list_id)) => self.heap.list_str_bytes(*list_id),
                    _ => 0,
                };
                stack.truncate(start);
                stack.push(res);
                if matches!(builtin, BuiltinId::Str | BuiltinId::Keys) {
                    self.record_str_alloc_and_check(
                        added_str_bytes,
                        stack,
                        frames,
                        ops_used,
                        started_at,
                        quota,
                    )?;
                }
            }
            Op::CallMethod { name_idx, argc } => {
                let n = argc as usize;
                let args_start = stack.len().saturating_sub(n);
                let recv_idx = args_start.saturating_sub(1);
                let receiver = stack.get(recv_idx).cloned().unwrap_or(Val::Null);
                let method = self.program.strings[name_idx as usize].clone();
                let (res, used) = self.invoke_method_with_gc(
                    receiver,
                    method.as_ref(),
                    args_start,
                    stack,
                    frames,
                    host,
                    ops_used,
                    started_at,
                    quota,
                )?;
                if let Some(b) = used {
                    self.used_builtins_mask |= 1u128 << (b as u8);
                }
                let added_str_bytes = match (used, &res) {
                    (Some(BuiltinId::Keys), Val::List(list_id)) => self.heap.list_str_bytes(*list_id),
                    _ => 0,
                };
                stack.truncate(recv_idx);
                stack.push(res);
                if used == Some(BuiltinId::Keys) {
                    self.record_str_alloc_and_check(
                        added_str_bytes,
                        stack,
                        frames,
                        ops_used,
                        started_at,
                        quota,
                    )?;
                }
            }
            Op::Return => {
                let ret = stack.pop().unwrap_or(Val::Null);
                frames.pop();
                if frames.is_empty() {
                    return Ok(Some(ret));
                }
                stack.push(ret);
            }
            _ => unreachable!("handled in run_frame"),
        }
        Ok(None)
    }

    #[allow(clippy::too_many_arguments)]
    fn invoke_builtin_with_gc(
        &mut self,
        builtin: BuiltinId,
        args_start: usize,
        stack: &[Val],
        frames: &[CallFrame],
        host: &mut dyn VmHost,
        ops_used: &mut u64,
        started_at: Instant,
        quota: ExecQuota,
    ) -> Result<Val, RuntimeError> {
        let budgets = self.budgets;
        let args = &stack[args_start..];
        let attempt = {
            let mut wd = ShaderWatchdog {
                ops_used,
                op_limit: quota.op_limit,
                started_at,
                wall_limit: quota.wall_limit,
            };
            let mut ctx = BuiltinCtx {
                heap: &mut self.heap,
                rng: &mut self.rng,
                budgets: &budgets,
                host,
                watchdog: &mut wd,
            };
            dispatch_builtin(builtin, args, &mut ctx)
        };
        match attempt {
            Ok(v) => Ok(v),
            Err(RuntimeError::MemoryBudgetExceeded { .. }) => {
                self.maybe_gc(stack, frames, ops_used, started_at, quota)?;
                let mut wd = ShaderWatchdog {
                    ops_used,
                    op_limit: quota.op_limit,
                    started_at,
                    wall_limit: quota.wall_limit,
                };
                let mut ctx = BuiltinCtx {
                    heap: &mut self.heap,
                    rng: &mut self.rng,
                    budgets: &budgets,
                    host,
                    watchdog: &mut wd,
                };
                dispatch_builtin(builtin, args, &mut ctx)
            }
            Err(e) => Err(e),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn invoke_method_with_gc(
        &mut self,
        receiver: Val,
        method: &str,
        args_start: usize,
        stack: &[Val],
        frames: &[CallFrame],
        host: &mut dyn VmHost,
        ops_used: &mut u64,
        started_at: Instant,
        quota: ExecQuota,
    ) -> Result<(Val, Option<BuiltinId>), RuntimeError> {
        let budgets = self.budgets;
        let args = &stack[args_start..];
        let attempt = {
            let mut wd = ShaderWatchdog {
                ops_used,
                op_limit: quota.op_limit,
                started_at,
                wall_limit: quota.wall_limit,
            };
            let mut ctx = BuiltinCtx {
                heap: &mut self.heap,
                rng: &mut self.rng,
                budgets: &budgets,
                host,
                watchdog: &mut wd,
            };
            dispatch_method(receiver.clone(), method, args, &mut ctx)
        };
        match attempt {
            Ok(v) => Ok(v),
            Err(RuntimeError::MemoryBudgetExceeded { .. }) => {
                self.maybe_gc(stack, frames, ops_used, started_at, quota)?;
                let mut wd = ShaderWatchdog {
                    ops_used,
                    op_limit: quota.op_limit,
                    started_at,
                    wall_limit: quota.wall_limit,
                };
                let mut ctx = BuiltinCtx {
                    heap: &mut self.heap,
                    rng: &mut self.rng,
                    budgets: &budgets,
                    host,
                    watchdog: &mut wd,
                };
                dispatch_method(receiver, method, args, &mut ctx)
            }
            Err(e) => Err(e),
        }
    }
}
