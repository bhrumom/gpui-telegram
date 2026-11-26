//! Arena-indexed `VmHeap`, `Val`, deterministic `SplitMix64` RNG,
//! iterative mark-and-sweep GC, and cycle-safe equality/formatting.
//!
//! 100% `Send + Sync`, zero `Rc<RefCell<...>>`, zero `unsafe`.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use crate::lang::limits::VmBudgets;
use crate::model::{RuntimeError, SceneInputState};

const CANONICAL_INPUT_KEYS: [&str; 10] = [
    "left",
    "right",
    "up",
    "down",
    "action",
    "mouse_x",
    "mouse_y",
    "mouse_down",
    "clicked",
    "pointer_active",
];

/// Deterministic 64-bit PRNG (SplitMix64), stored inside `VmSnapshot` for bit-exact seek replay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        Self {
            state: seed ^ 0x9E37_79B9_7F4A_7C15,
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform float in `[0.0, 1.0)`.
    pub fn next_f64(&mut self) -> f64 {
        let bits = self.next_u64() >> 11;
        (bits as f64) * (1.0 / ((1u64 << 53) as f64))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HeapId(pub u32);

#[derive(Debug, Clone)]
pub enum Val {
    Null,
    Bool(bool),
    Num(f64),
    Str(Arc<str>),
    List(HeapId),
    Map(HeapId),
    Canvas,
}

impl Val {
    pub fn is_truthy(&self) -> bool {
        match self {
            Self::Null => false,
            Self::Bool(b) => *b,
            Self::Num(n) => *n != 0.0 && !n.is_nan(),
            Self::Str(s) => !s.is_empty(),
            Self::List(_) | Self::Map(_) | Self::Canvas => true,
        }
    }

    pub fn as_num(&self) -> f64 {
        match self {
            Self::Num(n) => *n,
            Self::Bool(true) => 1.0,
            Self::Bool(false) | Self::Null => 0.0,
            Self::Str(s) => s.trim().parse::<f64>().ok().filter(|v| v.is_finite()).unwrap_or(0.0),
            Self::List(_) | Self::Map(_) | Self::Canvas => 0.0,
        }
    }

    pub fn str_bytes(&self) -> usize {
        match self {
            Self::Str(s) => s.len(),
            _ => 0,
        }
    }
}

#[derive(Debug, Clone)]
pub enum HeapObj {
    List(Vec<Val>),
    Map(BTreeMap<String, Val>),
}

#[derive(Debug, Clone)]
enum HeapSlot {
    Free,
    Occupied(HeapObj),
}

#[derive(Debug, Clone)]
pub struct VmHeap {
    slots: Vec<HeapSlot>,
    free_list: Vec<u32>,
    live_values: usize,
    total_str_bytes: usize,
}

#[derive(Debug, Clone)]
pub struct VmSnapshot {
    pub heap: VmHeap,
    pub globals: Vec<Val>,
    pub rng: SplitMix64,
    pub state_id: HeapId,
    pub input_id: HeapId,
}

impl Default for VmHeap {
    fn default() -> Self {
        Self::new()
    }
}

impl VmHeap {
    pub fn new() -> Self {
        Self {
            slots: Vec::with_capacity(64),
            free_list: Vec::new(),
            live_values: 0,
            total_str_bytes: 0,
        }
    }

    pub fn live_values(&self) -> usize {
        self.live_values
    }

    pub fn total_str_bytes(&self) -> usize {
        self.total_str_bytes
    }

    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    pub fn can_allocate(&self, add_values: usize, add_str_bytes: usize, budgets: &VmBudgets) -> bool {
        self.live_values.saturating_add(add_values) <= budgets.max_heap_values
            && self.total_str_bytes.saturating_add(add_str_bytes) <= budgets.max_total_str_bytes
    }

    pub fn alloc_list(&mut self, items: Vec<Val>, budgets: &VmBudgets) -> Result<HeapId, RuntimeError> {
        if items.len() > budgets.max_list_len {
            return Err(RuntimeError::MemoryBudgetExceeded {
                detail: format!("list length {} exceeds max {}", items.len(), budgets.max_list_len),
            });
        }
        for v in &items {
            if v.str_bytes() > budgets.max_str_bytes {
                return Err(RuntimeError::MemoryBudgetExceeded {
                    detail: format!(
                        "list element string exceeds max {} bytes",
                        budgets.max_str_bytes
                    ),
                });
            }
        }
        let add_vals = 1 + items.len();
        let add_bytes: usize = items
            .iter()
            .map(Val::str_bytes)
            .fold(0usize, usize::saturating_add);
        if !self.can_allocate(add_vals, add_bytes, budgets) {
            return Err(RuntimeError::MemoryBudgetExceeded {
                detail: format!(
                    "heap budget exceeded allocating list (vals={}, str_bytes={})",
                    self.live_values + add_vals,
                    self.total_str_bytes + add_bytes
                ),
            });
        }
        self.live_values += add_vals;
        self.total_str_bytes += add_bytes;
        Ok(self.insert_slot(HeapObj::List(items)))
    }

    pub fn alloc_map(
        &mut self,
        entries: BTreeMap<String, Val>,
        budgets: &VmBudgets,
    ) -> Result<HeapId, RuntimeError> {
        if entries.len() > budgets.max_map_keys {
            return Err(RuntimeError::MemoryBudgetExceeded {
                detail: format!("map keys {} exceeds max {}", entries.len(), budgets.max_map_keys),
            });
        }
        for (k, v) in &entries {
            if k.len() > budgets.max_str_bytes {
                return Err(RuntimeError::MemoryBudgetExceeded {
                    detail: format!("map key exceeds max {} bytes", budgets.max_str_bytes),
                });
            }
            if v.str_bytes() > budgets.max_str_bytes {
                return Err(RuntimeError::MemoryBudgetExceeded {
                    detail: format!("map value string exceeds max {} bytes", budgets.max_str_bytes),
                });
            }
        }
        let add_vals = 1 + entries.len();
        let add_bytes: usize = entries
            .iter()
            .map(|(k, v)| k.len().saturating_add(v.str_bytes()))
            .fold(0usize, usize::saturating_add);
        if !self.can_allocate(add_vals, add_bytes, budgets) {
            return Err(RuntimeError::MemoryBudgetExceeded {
                detail: format!(
                    "heap budget exceeded allocating map (vals={}, str_bytes={})",
                    self.live_values + add_vals,
                    self.total_str_bytes + add_bytes
                ),
            });
        }
        self.live_values += add_vals;
        self.total_str_bytes += add_bytes;
        Ok(self.insert_slot(HeapObj::Map(entries)))
    }

    fn insert_slot(&mut self, obj: HeapObj) -> HeapId {
        if let Some(idx) = self.free_list.pop() {
            self.slots[idx as usize] = HeapSlot::Occupied(obj);
            HeapId(idx)
        } else {
            let idx = self.slots.len() as u32;
            self.slots.push(HeapSlot::Occupied(obj));
            HeapId(idx)
        }
    }
}

impl VmHeap {
    pub fn get(&self, id: HeapId) -> Option<&HeapObj> {
        match self.slots.get(id.0 as usize) {
            Some(HeapSlot::Occupied(obj)) => Some(obj),
            _ => None,
        }
    }

    pub fn list_len(&self, id: HeapId) -> usize {
        match self.get(id) {
            Some(HeapObj::List(v)) => v.len(),
            _ => 0,
        }
    }

    pub fn map_len(&self, id: HeapId) -> usize {
        match self.get(id) {
            Some(HeapObj::Map(m)) => m.len(),
            _ => 0,
        }
    }

    pub fn list_str_bytes(&self, id: HeapId) -> usize {
        match self.get(id) {
            Some(HeapObj::List(items)) => items
                .iter()
                .map(Val::str_bytes)
                .fold(0usize, usize::saturating_add),
            _ => 0,
        }
    }

    pub fn list_get(&self, id: HeapId, idx: f64) -> Val {
        if !idx.is_finite() || idx < 0.0 {
            return Val::Null;
        }
        let i = idx.floor() as usize;
        match self.get(id) {
            Some(HeapObj::List(v)) => v.get(i).cloned().unwrap_or(Val::Null),
            _ => Val::Null,
        }
    }

    pub fn list_set(
        &mut self,
        id: HeapId,
        idx: f64,
        val: Val,
        budgets: &VmBudgets,
    ) -> Result<(), RuntimeError> {
        let len = self.list_len(id);
        if !idx.is_finite() {
            return Err(RuntimeError::IndexOutOfBounds { index: -1, len });
        }
        let i_signed = idx.floor() as i64;
        if i_signed < 0 || (i_signed as usize) >= len {
            return Err(RuntimeError::IndexOutOfBounds {
                index: i_signed,
                len,
            });
        }
        let i = i_signed as usize;
        let new_bytes = val.str_bytes();
        if new_bytes > budgets.max_str_bytes {
            return Err(RuntimeError::MemoryBudgetExceeded {
                detail: format!(
                    "list element string exceeds max {} bytes",
                    budgets.max_str_bytes
                ),
            });
        }
        let old_bytes = match self.get(id) {
            Some(HeapObj::List(v)) => v[i].str_bytes(),
            _ => 0,
        };
        if new_bytes > old_bytes
            && !self.can_allocate(0, new_bytes - old_bytes, budgets)
        {
            return Err(RuntimeError::MemoryBudgetExceeded {
                detail: "total string memory budget exceeded on list set".into(),
            });
        }
        if let Some(HeapSlot::Occupied(HeapObj::List(v))) = self.slots.get_mut(id.0 as usize) {
            v[i] = val;
        }
        self.total_str_bytes = self.total_str_bytes.saturating_sub(old_bytes) + new_bytes;
        Ok(())
    }

    pub fn list_push(
        &mut self,
        id: HeapId,
        val: Val,
        budgets: &VmBudgets,
    ) -> Result<(), RuntimeError> {
        let cur_len = self.list_len(id);
        if cur_len >= budgets.max_list_len {
            return Err(RuntimeError::MemoryBudgetExceeded {
                detail: format!("list length exceeded max {}", budgets.max_list_len),
            });
        }
        let add_bytes = val.str_bytes();
        if add_bytes > budgets.max_str_bytes {
            return Err(RuntimeError::MemoryBudgetExceeded {
                detail: format!(
                    "list element string exceeds max {} bytes",
                    budgets.max_str_bytes
                ),
            });
        }
        if !self.can_allocate(1, add_bytes, budgets) {
            return Err(RuntimeError::MemoryBudgetExceeded {
                detail: "heap budget exceeded on list push".into(),
            });
        }
        if let Some(HeapSlot::Occupied(HeapObj::List(v))) = self.slots.get_mut(id.0 as usize) {
            v.push(val);
            self.live_values += 1;
            self.total_str_bytes += add_bytes;
            Ok(())
        } else {
            Err(RuntimeError::TypeError {
                detail: "push target is not a list".into(),
            })
        }
    }

    pub fn list_pop(&mut self, id: HeapId) -> Val {
        if let Some(HeapSlot::Occupied(HeapObj::List(v))) = self.slots.get_mut(id.0 as usize) {
            if let Some(popped) = v.pop() {
                self.live_values = self.live_values.saturating_sub(1);
                self.total_str_bytes = self.total_str_bytes.saturating_sub(popped.str_bytes());
                return popped;
            }
        }
        Val::Null
    }

    pub fn map_get(&self, id: HeapId, key: &str) -> Val {
        match self.get(id) {
            Some(HeapObj::Map(m)) => m.get(key).cloned().unwrap_or(Val::Null),
            _ => Val::Null,
        }
    }

    pub fn map_has_key(&self, id: HeapId, key: &str) -> bool {
        match self.get(id) {
            Some(HeapObj::Map(m)) => m.contains_key(key),
            _ => false,
        }
    }

    pub fn map_keys(&self, id: HeapId) -> Vec<Val> {
        match self.get(id) {
            Some(HeapObj::Map(m)) => m.keys().map(|k| Val::Str(Arc::from(k.as_str()))).collect(),
            _ => Vec::new(),
        }
    }

    pub fn map_set(
        &mut self,
        id: HeapId,
        key: String,
        val: Val,
        budgets: &VmBudgets,
    ) -> Result<(), RuntimeError> {
        self.map_set_tracked(id, key, val, budgets).map(|_| ())
    }

    /// Inserts or updates a map entry, returning the byte length of any newly inserted key
    /// (`0` if updating an existing key).
    pub(crate) fn map_set_tracked(
        &mut self,
        id: HeapId,
        key: String,
        val: Val,
        budgets: &VmBudgets,
    ) -> Result<usize, RuntimeError> {
        if key.len() > budgets.max_str_bytes {
            return Err(RuntimeError::MemoryBudgetExceeded {
                detail: format!("map key exceeds max {} bytes", budgets.max_str_bytes),
            });
        }
        if val.str_bytes() > budgets.max_str_bytes {
            return Err(RuntimeError::MemoryBudgetExceeded {
                detail: format!("map value string exceeds max {} bytes", budgets.max_str_bytes),
            });
        }
        let (is_new, old_val_bytes, cur_len) = match self.get(id) {
            Some(HeapObj::Map(m)) => match m.get(&key) {
                Some(old) => (false, old.str_bytes(), m.len()),
                None => (true, 0, m.len()),
            },
            _ => {
                return Err(RuntimeError::TypeError {
                    detail: "map set target is not a map".into(),
                });
            }
        };
        if is_new && cur_len >= budgets.max_map_keys {
            return Err(RuntimeError::MemoryBudgetExceeded {
                detail: format!("map key count exceeded max {}", budgets.max_map_keys),
            });
        }
        let add_vals = usize::from(is_new);
        let added_key_bytes = if is_new { key.len() } else { 0 };
        let new_bytes = added_key_bytes + val.str_bytes();
        let net_add_bytes = new_bytes.saturating_sub(old_val_bytes);
        if !self.can_allocate(add_vals, net_add_bytes, budgets) {
            return Err(RuntimeError::MemoryBudgetExceeded {
                detail: "heap budget exceeded on map insert/update".into(),
            });
        }
        if let Some(HeapSlot::Occupied(HeapObj::Map(m))) = self.slots.get_mut(id.0 as usize) {
            m.insert(key, val);
        }
        self.live_values += add_vals;
        self.total_str_bytes = self.total_str_bytes.saturating_sub(old_val_bytes) + new_bytes;
        Ok(added_key_bytes)
    }

    /// Updates the 10 fixed fields of the persistent `input` map in-place with zero
    /// per-frame `BTreeMap` or key `String` allocations.
    pub fn update_input_map(&mut self, input_id: HeapId, input: &SceneInputState) {
        let _ = self.update_input_map_tracked(input_id, input);
    }

    /// Updates the persistent `input` map in-place and returns the byte length of any
    /// canonical keys that had to be re-inserted (`0` in normal operation).
    pub(crate) fn update_input_map_tracked(
        &mut self,
        input_id: HeapId,
        input: &SceneInputState,
    ) -> usize {
        let s = input.sanitized();
        let Some(HeapSlot::Occupied(HeapObj::Map(map))) = self.slots.get_mut(input_id.0 as usize)
        else {
            return 0;
        };
        if map.len() > CANONICAL_INPUT_KEYS.len() {
            let mut removed_vals = 0usize;
            let mut removed_bytes = 0usize;
            map.retain(|k, v| {
                if CANONICAL_INPUT_KEYS.contains(&k.as_str()) {
                    true
                } else {
                    removed_vals += 1;
                    removed_bytes = removed_bytes.saturating_add(k.len() + v.str_bytes());
                    false
                }
            });
            self.live_values = self.live_values.saturating_sub(removed_vals);
            self.total_str_bytes = self.total_str_bytes.saturating_sub(removed_bytes);
        }
        let mut added_key_bytes = 0usize;
        let mut set_field = |key: &str, new_val: Val| {
            if let Some(slot) = map.get_mut(key) {
                let old_bytes = slot.str_bytes();
                if old_bytes > 0 {
                    self.total_str_bytes = self.total_str_bytes.saturating_sub(old_bytes);
                }
                *slot = new_val;
            } else {
                self.live_values += 1;
                self.total_str_bytes = self.total_str_bytes.saturating_add(key.len());
                added_key_bytes = added_key_bytes.saturating_add(key.len());
                map.insert(key.to_string(), new_val);
            }
        };
        set_field("left", Val::Bool(s.left));
        set_field("right", Val::Bool(s.right));
        set_field("up", Val::Bool(s.up));
        set_field("down", Val::Bool(s.down));
        set_field("action", Val::Bool(s.action));
        set_field("mouse_x", Val::Num(s.mouse_x));
        set_field("mouse_y", Val::Num(s.mouse_y));
        set_field("mouse_down", Val::Bool(s.mouse_down));
        set_field("clicked", Val::Bool(s.clicked));
        set_field("pointer_active", Val::Bool(s.pointer_active));
        added_key_bytes
    }

    /// Computes exact deduplicated live string bytes across occupied heap slots
    /// (map keys + unique `Arc<str>` allocations in lists/maps) plus any distinct
    /// `Arc<str>` allocations held in `root_slices` (`globals`, `stack`, `frames[*].locals`).
    /// Returns `(total_live_str_bytes, non_heap_live_str_bytes)`.
    pub fn exact_live_str_bytes(&self, root_slices: &[&[Val]]) -> (usize, usize) {
        let mut seen_ptrs: HashSet<usize> = HashSet::new();
        let mut heap_bytes = 0usize;
        for slot in &self.slots {
            if let HeapSlot::Occupied(obj) = slot {
                match obj {
                    HeapObj::List(items) => {
                        for item in items {
                            if let Val::Str(s) = item {
                                if !s.is_empty() && seen_ptrs.insert(s.as_ptr().addr()) {
                                    heap_bytes = heap_bytes.saturating_add(s.len());
                                }
                            }
                        }
                    }
                    HeapObj::Map(map) => {
                        for (k, v) in map {
                            heap_bytes = heap_bytes.saturating_add(k.len());
                            if let Val::Str(s) = v {
                                if !s.is_empty() && seen_ptrs.insert(s.as_ptr().addr()) {
                                    heap_bytes = heap_bytes.saturating_add(s.len());
                                }
                            }
                        }
                    }
                }
            }
        }
        let mut non_heap_bytes = 0usize;
        for slice in root_slices {
            for val in *slice {
                if let Val::Str(s) = val {
                    if !s.is_empty() && seen_ptrs.insert(s.as_ptr().addr()) {
                        non_heap_bytes = non_heap_bytes.saturating_add(s.len());
                    }
                }
            }
        }
        (heap_bytes.saturating_add(non_heap_bytes), non_heap_bytes)
    }

    /// Iterative, cycle-safe mark-and-sweep garbage collection over reachable `HeapId`s.
    pub fn collect_garbage(&mut self, root_vals: &[&[Val]], state_id: HeapId, input_id: HeapId) {
        let slot_len = self.slots.len();
        let mut marked = vec![false; slot_len];
        let mut worklist = Vec::with_capacity(64);

        let push_id = |id: HeapId, marked: &mut [bool], worklist: &mut Vec<HeapId>| {
            let idx = id.0 as usize;
            if idx < marked.len() && !marked[idx] {
                marked[idx] = true;
                worklist.push(id);
            }
        };

        push_id(state_id, &mut marked, &mut worklist);
        push_id(input_id, &mut marked, &mut worklist);
        for slice in root_vals {
            for val in *slice {
                match val {
                    Val::List(id) | Val::Map(id) => push_id(*id, &mut marked, &mut worklist),
                    _ => {}
                }
            }
        }

        while let Some(id) = worklist.pop() {
            if let Some(HeapSlot::Occupied(obj)) = self.slots.get(id.0 as usize) {
                match obj {
                    HeapObj::List(items) => {
                        for item in items {
                            if let Val::List(cid) | Val::Map(cid) = item {
                                push_id(*cid, &mut marked, &mut worklist);
                            }
                        }
                    }
                    HeapObj::Map(map) => {
                        for item in map.values() {
                            if let Val::List(cid) | Val::Map(cid) = item {
                                push_id(*cid, &mut marked, &mut worklist);
                            }
                        }
                    }
                }
            }
        }

        self.free_list.clear();
        self.live_values = 0;
        self.total_str_bytes = 0;
        for (idx, slot) in self.slots.iter_mut().enumerate() {
            if marked[idx] {
                if let HeapSlot::Occupied(obj) = slot {
                    match obj {
                        HeapObj::List(items) => {
                            self.live_values += 1 + items.len();
                            self.total_str_bytes += items.iter().map(Val::str_bytes).sum::<usize>();
                        }
                        HeapObj::Map(map) => {
                            self.live_values += 1 + map.len();
                            self.total_str_bytes += map
                                .iter()
                                .map(|(k, v)| k.len() + v.str_bytes())
                                .sum::<usize>();
                        }
                    }
                }
            } else {
                *slot = HeapSlot::Free;
                self.free_list.push(idx as u32);
            }
        }
    }
}

impl VmHeap {
    /// Cycle-safe, depth-bounded structural equality (fast-path by `HeapId`).
    pub fn val_eq(&self, a: &Val, b: &Val) -> bool {
        let mut budget = 256usize;
        self.val_eq_inner(a, b, 0, &mut budget)
    }

    fn val_eq_inner(&self, a: &Val, b: &Val, depth: usize, budget: &mut usize) -> bool {
        if depth > 16 || *budget == 0 {
            return false;
        }
        *budget = budget.saturating_sub(1);
        match (a, b) {
            (Val::Null, Val::Null) => true,
            (Val::Bool(x), Val::Bool(y)) => x == y,
            (Val::Num(x), Val::Num(y)) => x == y,
            (Val::Str(x), Val::Str(y)) => x == y,
            (Val::Canvas, Val::Canvas) => true,
            (Val::List(x), Val::List(y)) => {
                if x == y {
                    return true;
                }
                match (self.get(*x), self.get(*y)) {
                    (Some(HeapObj::List(la)), Some(HeapObj::List(lb))) if la.len() == lb.len() => {
                        la.iter()
                            .zip(lb.iter())
                            .all(|(va, vb)| self.val_eq_inner(va, vb, depth + 1, budget))
                    }
                    _ => false,
                }
            }
            (Val::Map(x), Val::Map(y)) => {
                if x == y {
                    return true;
                }
                match (self.get(*x), self.get(*y)) {
                    (Some(HeapObj::Map(ma)), Some(HeapObj::Map(mb))) if ma.len() == mb.len() => {
                        ma.iter().zip(mb.iter()).all(|((ka, va), (kb, vb))| {
                            ka == kb && self.val_eq_inner(va, vb, depth + 1, budget)
                        })
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    /// Cycle-safe, byte-bounded display formatting for `str(v)` and string concatenation.
    pub fn format_val(&self, v: &Val, max_bytes: usize) -> Result<String, RuntimeError> {
        let mut out = String::new();
        let mut active = HashSet::new();
        self.format_val_inner(v, 0, max_bytes, &mut active, &mut out)?;
        Ok(out)
    }

    fn format_val_inner(
        &self,
        v: &Val,
        depth: usize,
        max_bytes: usize,
        active: &mut HashSet<HeapId>,
        out: &mut String,
    ) -> Result<(), RuntimeError> {
        let push_checked = |out: &mut String, s: &str| -> Result<(), RuntimeError> {
            if out.len().saturating_add(s.len()) > max_bytes {
                return Err(RuntimeError::MemoryBudgetExceeded {
                    detail: format!("formatted string exceeds max {max_bytes} bytes"),
                });
            }
            out.push_str(s);
            Ok(())
        };

        match v {
            Val::Null => push_checked(out, "null"),
            Val::Bool(true) => push_checked(out, "true"),
            Val::Bool(false) => push_checked(out, "false"),
            Val::Num(n) => {
                let s = if !n.is_finite() {
                    "0".to_string()
                } else if n.fract() == 0.0 && n.abs() < 1e15 {
                    let rounded = if *n == 0.0 { 0i64 } else { *n as i64 };
                    rounded.to_string()
                } else {
                    format!("{n}")
                };
                push_checked(out, &s)
            }
            Val::Str(s) => push_checked(out, s),
            Val::Canvas => push_checked(out, "<canvas>"),
            Val::List(id) => {
                if depth >= 8 || !active.insert(*id) {
                    return push_checked(out, "[<...>]");
                }
                push_checked(out, "[")?;
                if let Some(HeapObj::List(items)) = self.get(*id) {
                    for (idx, item) in items.iter().enumerate() {
                        if idx > 0 {
                            push_checked(out, ", ")?;
                        }
                        self.format_val_inner(item, depth + 1, max_bytes, active, out)?;
                    }
                }
                active.remove(id);
                push_checked(out, "]")
            }
            Val::Map(id) => {
                if depth >= 8 || !active.insert(*id) {
                    return push_checked(out, "{<...>}");
                }
                push_checked(out, "{")?;
                if let Some(HeapObj::Map(map)) = self.get(*id) {
                    for (idx, (k, val)) in map.iter().enumerate() {
                        if idx > 0 {
                            push_checked(out, ", ")?;
                        }
                        push_checked(out, k)?;
                        push_checked(out, ": ")?;
                        self.format_val_inner(val, depth + 1, max_bytes, active, out)?;
                    }
                }
                active.remove(id);
                push_checked(out, "}")
            }
        }
    }
}
