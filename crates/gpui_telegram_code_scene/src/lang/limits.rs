//! Hard fail-closed compile & runtime budgets for `CodeSceneScript` (`cs1`).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VmBudgets {
    pub max_source_bytes: usize,
    pub max_ast_depth: usize,
    pub max_bytecode_ops: usize,
    pub max_constants: usize,
    pub max_ops_per_frame: u64,
    pub max_frame_wall_ms: u64,
    pub max_call_depth: usize,
    pub max_stack_values: usize,
    pub max_heap_values: usize,
    pub max_list_len: usize,
    pub max_map_keys: usize,
    pub max_str_bytes: usize,
    pub max_total_str_bytes: usize,
    pub max_draw_cmds: usize,
    pub max_audio_pulses: usize,
    pub max_pixel_width: u32,
    pub max_pixel_height: u32,
    pub max_pixel_area: usize,
    pub max_seek_ops: u64,
    pub max_seek_wall_ms: u64,
}

impl Default for VmBudgets {
    fn default() -> Self {
        Self {
            max_source_bytes: 65_536,
            max_ast_depth: 64,
            max_bytecode_ops: 16_384,
            max_constants: 2_048,
            max_ops_per_frame: 100_000,
            max_frame_wall_ms: 10,
            max_call_depth: 48,
            max_stack_values: 4_096,
            max_heap_values: 32_768,
            max_list_len: 4_096,
            max_map_keys: 1_024,
            max_str_bytes: 8_192,
            max_total_str_bytes: 2 * 1024 * 1024,
            max_draw_cmds: 2_048,
            max_audio_pulses: 32,
            max_pixel_width: 240,
            max_pixel_height: 320,
            max_pixel_area: 240 * 320,
            max_seek_ops: 2_000_000,
            max_seek_wall_ms: 100,
        }
    }
}
