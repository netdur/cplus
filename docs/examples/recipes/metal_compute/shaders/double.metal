// double.metal — minimal compute kernel: in[i] * 2.0 → out[i].
//
// Compile to a .metallib via the recipe's build.sh, which the C+ source
// embeds at compile time via `include_bytes!`. The host program loads
// this blob through ObjC interop, never touches the .metal source at
// runtime, and dispatches the kernel over a 1D threadgroup grid.

#include <metal_stdlib>
using namespace metal;

kernel void double_each(
    device const float* in   [[ buffer(0) ]],
    device       float* out  [[ buffer(1) ]],
    uint                 idx [[ thread_position_in_grid ]]
) {
    out[idx] = in[idx] * 2.0f;
}
