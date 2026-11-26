//! `CodeSceneScript` (`cs1`) language frontend, compiler, heap, stdlib, and sandboxed VM.

pub mod ast;
pub mod compiler;
pub mod heap;
pub mod lexer;
pub mod limits;
pub mod parser;
pub mod shaders;
pub mod stdlib;
pub mod vm;

pub use compiler::CompiledProgram;
pub use heap::{HeapId, HeapObj, SplitMix64, Val, VmHeap, VmSnapshot};
pub use limits::VmBudgets;
pub use stdlib::{BuiltinId, VmHost};
pub use vm::{ExecQuota, VmState};

use crate::model::CompileDiagnostic;

/// Lexes, parses, and compiles `CodeSceneScript` (`cs1`) source under `budgets`.
pub fn compile_source(
    source: &str,
    budgets: &VmBudgets,
) -> Result<CompiledProgram, CompileDiagnostic> {
    let tokens = lexer::tokenize(source, budgets)?;
    let program = parser::parse(&tokens, budgets)?;
    compiler::compile(&program, budgets)
}
