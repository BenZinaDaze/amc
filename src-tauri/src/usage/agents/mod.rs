// One module per usage source; each exposes its adapter and the Agents
// overview status probe. `omp` is additionally consumed by operations
// (state()) for the Agents-page version display.
pub(super) mod claude;
pub(super) mod codex;
pub(crate) mod omp;
